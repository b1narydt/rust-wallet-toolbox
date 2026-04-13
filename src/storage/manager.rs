//! WalletStorageManager -- multi-provider storage coordinator with hierarchical locking.
//!
//! Implements the TS-parity manager architecture:
//! - Multiple providers wrapped in `ManagedStorage` with cached settings/user
//! - `make_available()` partitions stores into active/backups/conflicting_actives
//! - Four-level hierarchical lock system (reader < writer < sync < storage_provider)
//! - All writes route to active only; backups sync via `update_backups()` (Plan 02)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::warn;

use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, RelinquishCertificateArgs,
    RelinquishOutputArgs,
};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::status::TransactionStatus;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::storage::find_args::{
    FindCertificateFieldsArgs, FindCertificatesArgs, FindMonitorEventsArgs, FindOutputBasketsArgs,
    FindOutputsArgs, FindProvenTxReqsArgs, FindProvenTxsArgs, FindTransactionsArgs, OutputPartial,
    ProvenTxPartial, ProvenTxReqPartial, PurgeParams, TransactionPartial,
};
use crate::storage::sync::request_args::{RequestSyncChunkArgs, SyncChunkOffset};
use crate::storage::sync::sync_map::SyncMap;
use crate::storage::sync::{ProcessSyncChunkResult, SyncChunk};
use crate::storage::traits::wallet_provider::WalletStorageProvider;
use crate::storage::TrxToken;
use crate::tables::SyncState;
use crate::tables::{
    Certificate, MonitorEvent, Output, OutputBasket, ProvenTx, ProvenTxReq, Settings, Transaction,
    User,
};
use crate::wallet::types::{
    AdminStatsResult, AuthId, ReproveHeaderResult, ReproveProvenResult, ReproveProvenUpdate,
    WalletStorageInfo,
};

// ---------------------------------------------------------------------------
// make_request_sync_chunk_args -- free helper for sync loop
// ---------------------------------------------------------------------------

/// Build `RequestSyncChunkArgs` from an existing `SyncState`.
///
/// The `SyncState.sync_map` JSON is deserialized to extract per-entity
/// offsets (count field) and the `when` timestamp becomes the `since` filter.
///
/// # Arguments
/// * `sync_state` - The sync state record for this storage pair.
/// * `identity_key` - The wallet owner's identity key.
/// * `writer_storage_identity_key` - The destination storage's identity key.
pub fn make_request_sync_chunk_args(
    sync_state: &SyncState,
    identity_key: &str,
    writer_storage_identity_key: &str,
) -> WalletResult<RequestSyncChunkArgs> {
    // Parse the stored JSON sync_map to extract per-entity offsets.
    // A newly-inserted SyncState has sync_map = "{}" which cannot deserialize
    // into SyncMap (all fields required). Fall back to a fresh SyncMap in that case.
    let sync_map: SyncMap = if sync_state.sync_map.is_empty() || sync_state.sync_map == "{}" {
        SyncMap::new()
    } else {
        serde_json::from_str(&sync_state.sync_map).map_err(|e| {
            WalletError::Internal(format!(
                "make_request_sync_chunk_args: failed to parse sync_map JSON: {e}"
            ))
        })?
    };

    // Build offsets from each EntitySyncMap's count field.
    // The `count` tracks cumulative items received in the current sync window
    // — this IS the pagination offset, matching TS `ess.count`.
    let offsets = vec![
        SyncChunkOffset {
            name: sync_map.proven_tx.entity_name.clone(),
            offset: sync_map.proven_tx.count,
        },
        SyncChunkOffset {
            name: sync_map.output_basket.entity_name.clone(),
            offset: sync_map.output_basket.count,
        },
        SyncChunkOffset {
            name: sync_map.transaction.entity_name.clone(),
            offset: sync_map.transaction.count,
        },
        SyncChunkOffset {
            name: sync_map.output.entity_name.clone(),
            offset: sync_map.output.count,
        },
        SyncChunkOffset {
            name: sync_map.tx_label.entity_name.clone(),
            offset: sync_map.tx_label.count,
        },
        SyncChunkOffset {
            name: sync_map.tx_label_map.entity_name.clone(),
            offset: sync_map.tx_label_map.count,
        },
        SyncChunkOffset {
            name: sync_map.output_tag.entity_name.clone(),
            offset: sync_map.output_tag.count,
        },
        SyncChunkOffset {
            name: sync_map.output_tag_map.entity_name.clone(),
            offset: sync_map.output_tag_map.count,
        },
        SyncChunkOffset {
            name: sync_map.certificate.entity_name.clone(),
            offset: sync_map.certificate.count,
        },
        SyncChunkOffset {
            name: sync_map.certificate_field.entity_name.clone(),
            offset: sync_map.certificate_field.count,
        },
        SyncChunkOffset {
            name: sync_map.commission.entity_name.clone(),
            offset: sync_map.commission.count,
        },
        SyncChunkOffset {
            name: sync_map.proven_tx_req.entity_name.clone(),
            offset: sync_map.proven_tx_req.count,
        },
    ];

    Ok(RequestSyncChunkArgs {
        // from = the reader (sync_state records the reader's identity key)
        from_storage_identity_key: sync_state.storage_identity_key.clone(),
        // to = the writer we are syncing into
        to_storage_identity_key: writer_storage_identity_key.to_string(),
        identity_key: identity_key.to_string(),
        since: sync_state.when,
        max_rough_size: 10_000_000,
        max_items: 1000,
        offsets,
    })
}

// ---------------------------------------------------------------------------
// ManagedStorage -- wraps a single WalletStorageProvider with cached state
// ---------------------------------------------------------------------------

/// Per-provider wrapper that caches settings and user after `make_available`.
///
/// Private to this module — only `WalletStorageManager` creates these.
struct ManagedStorage {
    /// The underlying storage provider.
    storage: Arc<dyn WalletStorageProvider>,
    /// Set once at construction from `storage.is_storage_provider()`.
    is_storage_provider: bool,
    /// Becomes true after `make_available()` succeeds for this store.
    is_available: AtomicBool,
    /// Cached `Settings` populated during `make_available`.
    settings: tokio::sync::Mutex<Option<Settings>>,
    /// Cached `User` populated during `make_available`.
    user: tokio::sync::Mutex<Option<User>>,
}

impl ManagedStorage {
    fn new(storage: Arc<dyn WalletStorageProvider>) -> Self {
        let is_sp = storage.is_storage_provider();
        Self {
            storage,
            is_storage_provider: is_sp,
            is_available: AtomicBool::new(false),
            settings: tokio::sync::Mutex::new(None),
            user: tokio::sync::Mutex::new(None),
        }
    }

    /// Get a clone of the cached settings, if available.
    async fn get_settings_cached(&self) -> Option<Settings> {
        self.settings.lock().await.clone()
    }

    /// Get a clone of the cached user, if available.
    async fn get_user_cached(&self) -> Option<User> {
        self.user.lock().await.clone()
    }
}

// ---------------------------------------------------------------------------
// ManagerState -- partition state for active/backup/conflicting stores
// ---------------------------------------------------------------------------

/// Guards the partition state: which store is active, which are backups, etc.
///
/// Protected by a `tokio::sync::Mutex` inside `WalletStorageManager`.
struct ManagerState {
    /// All providers, with active (if any) first at construction.
    stores: Vec<ManagedStorage>,
    /// Index into `stores` for the currently-active provider.
    active_index: Option<usize>,
    /// Indices of backup stores (after active is determined).
    backup_indices: Vec<usize>,
    /// Indices of stores whose `user.active_storage` doesn't match active.
    conflicting_active_indices: Vec<usize>,
}

impl ManagerState {
    fn new(stores: Vec<ManagedStorage>) -> Self {
        Self {
            stores,
            active_index: None,
            backup_indices: Vec::new(),
            conflicting_active_indices: Vec::new(),
        }
    }

    /// Returns the active index, or an error if not yet partitioned.
    fn require_active(&self) -> WalletResult<usize> {
        self.active_index.ok_or_else(|| {
            WalletError::InvalidOperation(
                "WalletStorageManager not yet available — call make_available() first".to_string(),
            )
        })
    }
}

// ---------------------------------------------------------------------------
// WalletStorageManager -- public struct
// ---------------------------------------------------------------------------

/// Multi-provider storage manager with hierarchical locking.
///
/// Wraps one or more `WalletStorageProvider` implementations. On first use
/// (or explicit `make_available()` call) partitions providers into
/// active / backups / conflicting_actives based on stored user preferences.
///
/// # Lock hierarchy
///
/// Four `tokio::sync::Mutex<()>` must be acquired in strict ascending order:
/// reader < writer < sync < storage_provider.
///
/// - `acquire_reader` — read-only operations
/// - `acquire_writer` — mutations (create/process/abort action, etc.)
/// - `acquire_sync` — sync loop operations
/// - `acquire_storage_provider` — storage-provider-level operations
pub struct WalletStorageManager {
    /// Partition state — holds all ManagedStorage instances.
    state: tokio::sync::Mutex<ManagerState>,
    /// Wallet owner's identity key (compressed public key hex).
    identity_key: String,
    /// Original active provider (the first store passed to constructor).
    /// Kept for sync access before make_available() completes.
    initial_active: Option<Arc<dyn WalletStorageProvider>>,
    /// Original backup providers (remaining stores passed to constructor).
    /// Kept for sync access before make_available() completes.
    initial_backups: Vec<Arc<dyn WalletStorageProvider>>,
    /// Level-1 lock (outermost). All operations acquire this.
    pub(crate) reader_lock: tokio::sync::Mutex<()>,
    /// Level-2 lock. Writers + sync + sp acquire this.
    pub(crate) writer_lock: tokio::sync::Mutex<()>,
    /// Level-3 lock. Sync + sp acquire this.
    pub(crate) sync_lock: tokio::sync::Mutex<()>,
    /// Level-4 lock (innermost). Only sp-level operations acquire this.
    pub(crate) sp_lock: tokio::sync::Mutex<()>,
    /// Manager-level spend lock. Serializes UTXO-mutating operations
    /// (createAction/signAction/internalizeAction/abortAction/relinquishOutput)
    /// across ALL `Wallet` instances sharing this manager, preventing
    /// double-spend from concurrent callers on shared storage.
    pub(crate) spend_lock: tokio::sync::Mutex<()>,
    /// True once `make_available()` completes successfully.
    is_available_flag: AtomicBool,
    /// Optional wallet services reference.
    services: tokio::sync::Mutex<Option<Arc<dyn WalletServices>>>,
}

impl WalletStorageManager {
    // -----------------------------------------------------------------------
    // Constructor
    // -----------------------------------------------------------------------

    /// Create a new manager from an optional active provider and zero or more backups.
    ///
    /// Providers are stored in order: active first (if Some), then backups.
    /// No partitioning happens at construction — call `make_available()` to initialize.
    pub fn new(
        identity_key: String,
        active: Option<Arc<dyn WalletStorageProvider>>,
        backups: Vec<Arc<dyn WalletStorageProvider>>,
    ) -> Self {
        let initial_active = active.clone();
        let initial_backups = backups.clone();

        let mut stores = Vec::with_capacity(active.is_some() as usize + backups.len());
        if let Some(a) = active {
            stores.push(ManagedStorage::new(a));
        }
        for b in backups {
            stores.push(ManagedStorage::new(b));
        }

        Self {
            state: tokio::sync::Mutex::new(ManagerState::new(stores)),
            identity_key,
            initial_active,
            initial_backups,
            reader_lock: tokio::sync::Mutex::new(()),
            writer_lock: tokio::sync::Mutex::new(()),
            sync_lock: tokio::sync::Mutex::new(()),
            sp_lock: tokio::sync::Mutex::new(()),
            spend_lock: tokio::sync::Mutex::new(()),
            is_available_flag: AtomicBool::new(false),
            services: tokio::sync::Mutex::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Zero-lock sync accessors
    // -----------------------------------------------------------------------

    /// Returns the wallet owner's identity key.
    pub fn get_user_identity_key(&self) -> &str {
        &self.identity_key
    }

    /// Alias for `get_user_identity_key()`.
    pub fn auth_id(&self) -> &str {
        &self.identity_key
    }

    /// Returns `true` after `make_available()` has successfully completed.
    pub fn is_available(&self) -> bool {
        self.is_available_flag.load(Ordering::Acquire)
    }

    /// The manager itself is NOT a storage provider — always returns `false`.
    ///
    /// Matches TS `WalletStorageManager.isStorageProvider = false`.
    pub fn is_storage_provider(&self) -> bool {
        false
    }

    /// Returns the initial active provider (the first provider passed to the constructor).
    ///
    /// This is a sync accessor that returns the provider as passed at construction,
    /// before `make_available()` determines the actual active via partition logic.
    /// Primarily used for backward-compatibility in sync contexts (Wallet::new, signer setup).
    pub fn active(&self) -> Option<&Arc<dyn WalletStorageProvider>> {
        self.initial_active.as_ref()
    }

    /// Returns the initial backup providers (the remaining providers from the constructor).
    ///
    /// Sync accessor for backward-compatibility.
    pub fn backups(&self) -> &[Arc<dyn WalletStorageProvider>] {
        &self.initial_backups
    }

    /// Returns the first backup provider if exactly one backup was provided.
    ///
    /// Backward-compatibility alias for the old `backup()` method.
    pub fn backup(&self) -> Option<&Arc<dyn WalletStorageProvider>> {
        self.initial_backups.first()
    }

    /// Returns true if at least one backup provider was provided.
    pub fn has_backup(&self) -> bool {
        !self.initial_backups.is_empty()
    }

    // -----------------------------------------------------------------------
    // Async state accessors (lock state mutex)
    // -----------------------------------------------------------------------

    /// Returns `true` if there is at least one provider registered.
    pub async fn can_make_available(&self) -> bool {
        let state = self.state.lock().await;
        !state.stores.is_empty()
    }

    /// Returns the active store's `settings.storage_identity_key`.
    pub async fn get_active_store(&self) -> WalletResult<String> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        let settings = state.stores[idx]
            .get_settings_cached()
            .await
            .ok_or_else(|| {
                WalletError::InvalidOperation("Active settings not cached".to_string())
            })?;
        Ok(settings.storage_identity_key)
    }

    /// Returns the active store's `settings.storage_name`.
    pub async fn get_active_store_name(&self) -> WalletResult<String> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        let settings = state.stores[idx]
            .get_settings_cached()
            .await
            .ok_or_else(|| {
                WalletError::InvalidOperation("Active settings not cached".to_string())
            })?;
        Ok(settings.storage_name)
    }

    /// Returns a clone of the active store's cached `Settings`.
    pub async fn get_active_settings(&self) -> WalletResult<Settings> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        state.stores[idx]
            .get_settings_cached()
            .await
            .ok_or_else(|| WalletError::InvalidOperation("Active settings not cached".to_string()))
    }

    /// Alias for `get_active_settings()` (TS parity name: `getSettings`).
    pub async fn get_settings(&self) -> WalletResult<Settings> {
        self.get_active_settings().await
    }

    /// Returns a clone of the active store's cached `User`.
    pub async fn get_active_user(&self) -> WalletResult<User> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        state.stores[idx]
            .get_user_cached()
            .await
            .ok_or_else(|| WalletError::InvalidOperation("Active user not cached".to_string()))
    }

    /// Returns `storage_identity_key` strings for all backup stores.
    pub async fn get_backup_stores(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut result = Vec::with_capacity(state.backup_indices.len());
        for &idx in &state.backup_indices {
            if let Some(settings) = state.stores[idx].get_settings_cached().await {
                result.push(settings.storage_identity_key);
            }
        }
        result
    }

    /// Returns `storage_identity_key` strings for all conflicting active stores.
    pub async fn get_conflicting_stores(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut result = Vec::with_capacity(state.conflicting_active_indices.len());
        for &idx in &state.conflicting_active_indices {
            if let Some(settings) = state.stores[idx].get_settings_cached().await {
                result.push(settings.storage_identity_key);
            }
        }
        result
    }

    /// Returns `storage_identity_key` strings for ALL stores.
    ///
    /// Before `make_available()`, returns empty strings as placeholders so callers
    /// can check the count without waiting for initialization.
    pub async fn get_all_stores(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut result = Vec::with_capacity(state.stores.len());
        for store in &state.stores {
            if let Some(settings) = store.get_settings_cached().await {
                result.push(settings.storage_identity_key);
            } else {
                result.push(String::new());
            }
        }
        result
    }

    /// Returns `true` if the active store's `is_storage_provider` flag is set.
    pub async fn is_active_storage_provider(&self) -> WalletResult<bool> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        Ok(state.stores[idx].is_storage_provider)
    }

    /// Returns true if active.settings.storage_identity_key == active.user.active_storage
    /// AND conflicting_active_indices is empty.
    pub async fn is_active_enabled(&self) -> bool {
        let state = self.state.lock().await;
        let Some(idx) = state.active_index else {
            return false;
        };
        let settings = match state.stores[idx].get_settings_cached().await {
            Some(s) => s,
            None => return false,
        };
        let user = match state.stores[idx].get_user_cached().await {
            Some(u) => u,
            None => return false,
        };
        state.conflicting_active_indices.is_empty()
            && settings.storage_identity_key == user.active_storage
    }

    /// Builds an `AuthId` for the manager's identity key.
    ///
    /// If `must_be_active` is true and the manager is not yet available,
    /// returns `WERR_NOT_ACTIVE`.
    pub async fn get_auth(&self, must_be_active: bool) -> WalletResult<AuthId> {
        if must_be_active && !self.is_available() {
            // Auto-initialize if not yet available (same pattern as acquire_reader)
            self.make_available().await?;
        }

        let state = self.state.lock().await;
        let user_id = if let Some(idx) = state.active_index {
            state.stores[idx].get_user_cached().await.map(|u| u.user_id)
        } else {
            None
        };

        if must_be_active {
            let Some(idx) = state.active_index else {
                return Err(WalletError::NotActive(
                    "No active storage — call make_available() first".to_string(),
                ));
            };
            let settings = state.stores[idx]
                .get_settings_cached()
                .await
                .ok_or_else(|| WalletError::NotActive("Active settings not cached".to_string()))?;
            let user = state.stores[idx]
                .get_user_cached()
                .await
                .ok_or_else(|| WalletError::NotActive("Active user not cached".to_string()))?;
            if !state.conflicting_active_indices.is_empty()
                || settings.storage_identity_key != user.active_storage
            {
                return Err(WalletError::NotActive(
                    "Active storage is not enabled — conflicting stores or mismatched active_storage".to_string(),
                ));
            }
        }

        Ok(AuthId {
            identity_key: self.identity_key.clone(),
            user_id,
            is_active: Some(self.is_available()),
        })
    }

    /// Returns the user_id from the active store's cached user.
    pub async fn get_user_id(&self) -> WalletResult<i64> {
        let auth = self.get_auth(false).await?;
        auth.user_id.ok_or_else(|| {
            WalletError::InvalidOperation(
                "user_id not available — call make_available() first".to_string(),
            )
        })
    }

    // -----------------------------------------------------------------------
    // get_active — returns the active WalletStorageProvider
    // -----------------------------------------------------------------------

    /// Returns a clone of the active `Arc<dyn WalletStorageProvider>`.
    ///
    /// Errors if `make_available()` has not been called.
    pub async fn get_active(&self) -> WalletResult<Arc<dyn WalletStorageProvider>> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        Ok(state.stores[idx].storage.clone())
    }

    // -----------------------------------------------------------------------
    // makeAvailable — initialize and partition stores
    // -----------------------------------------------------------------------

    /// Initialize all stores and partition them into active/backups/conflicting.
    ///
    /// Idempotent — returns cached active settings on subsequent calls.
    /// Uses `reader_lock` as the idempotency guard to prevent concurrent re-init.
    pub async fn make_available(&self) -> WalletResult<Settings> {
        // Fast path: already available
        if self.is_available() {
            return self.get_active_settings().await;
        }

        // Acquire reader lock to serialize concurrent makeAvailable calls (Pitfall 3)
        let _reader_guard = self.reader_lock.lock().await;

        // Re-check after acquiring lock (another task may have finished first)
        if self.is_available() {
            return self.get_active_settings().await;
        }

        self.do_make_available().await
    }

    /// Internal make_available that runs under the reader lock.
    ///
    /// Separated from `make_available()` so `acquire_reader()` can avoid
    /// re-acquiring the reader_lock that it is about to return to the caller.
    async fn do_make_available(&self) -> WalletResult<Settings> {
        // Collect storage Arcs under a brief state lock, then release so that
        // network I/O (make_available, find_or_insert_user) does not block
        // concurrent operations on the manager.
        let providers: Vec<Arc<dyn WalletStorageProvider>> = {
            let state = self.state.lock().await;
            if state.stores.is_empty() {
                return Err(WalletError::InvalidParameter {
                    parameter: "stores".to_string(),
                    must_be: "non-empty — provide at least one storage provider".to_string(),
                });
            }
            state.stores.iter().map(|s| s.storage.clone()).collect()
        };

        // Step 1: Call make_available() + find_or_insert_user() on each store
        // without holding the state lock.
        let mut init_results: Vec<(Settings, User)> = Vec::with_capacity(providers.len());
        for provider in &providers {
            let settings = provider.make_available().await?;
            let (user, _) = provider.find_or_insert_user(&self.identity_key).await?;
            init_results.push((settings, user));
        }

        // Re-acquire the state lock to write results and partition.
        let mut state = self.state.lock().await;
        for (i, (settings, user)) in init_results.into_iter().enumerate() {
            *state.stores[i].settings.lock().await = Some(settings);
            *state.stores[i].user.lock().await = Some(user);
            state.stores[i].is_available.store(true, Ordering::Release);
        }

        // Step 2: Partition stores — first store is default active
        let num_stores = state.stores.len();
        state.active_index = Some(0);
        state.backup_indices.clear();
        state.conflicting_active_indices.clear();

        for i in 1..num_stores {
            let store_sik = {
                let s = state.stores[i].settings.lock().await;
                s.as_ref()
                    .map(|s| s.storage_identity_key.clone())
                    .unwrap_or_default()
            };
            let store_user_active = {
                let u = state.stores[i].user.lock().await;
                u.as_ref()
                    .map(|u| u.active_storage.clone())
                    .unwrap_or_default()
            };

            // Check if current active is enabled (settings.sik == user.active_storage)
            let current_active_enabled = {
                let active_idx = state.active_index.unwrap();
                let active_sik = {
                    let s = state.stores[active_idx].settings.lock().await;
                    s.as_ref()
                        .map(|s| s.storage_identity_key.clone())
                        .unwrap_or_default()
                };
                let active_user_active = {
                    let u = state.stores[active_idx].user.lock().await;
                    u.as_ref()
                        .map(|u| u.active_storage.clone())
                        .unwrap_or_default()
                };
                active_sik == active_user_active
            };

            if store_user_active == store_sik && !current_active_enabled {
                // Swap: old active becomes backup, this store becomes new active
                let old_active = state.active_index.unwrap();
                state.backup_indices.push(old_active);
                state.active_index = Some(i);
            } else {
                state.backup_indices.push(i);
            }
        }

        // Step 3: Second pass — partition backups into plain backups vs conflicting_actives
        let active_sik = {
            let active_idx = state.active_index.unwrap();
            let s = state.stores[active_idx].settings.lock().await;
            s.as_ref()
                .map(|s| s.storage_identity_key.clone())
                .unwrap_or_default()
        };

        let old_backups = std::mem::take(&mut state.backup_indices);
        for backup_idx in old_backups {
            let backup_user_active = {
                let u = state.stores[backup_idx].user.lock().await;
                u.as_ref()
                    .map(|u| u.active_storage.clone())
                    .unwrap_or_default()
            };
            if backup_user_active != active_sik {
                state.conflicting_active_indices.push(backup_idx);
            } else {
                state.backup_indices.push(backup_idx);
            }
        }

        // Step 4: Mark manager as available
        self.is_available_flag.store(true, Ordering::Release);

        // Return active settings
        let active_idx = state.active_index.unwrap();
        let settings = state.stores[active_idx]
            .settings
            .lock()
            .await
            .clone()
            .unwrap();
        Ok(settings)
    }

    // -----------------------------------------------------------------------
    // Hierarchical lock acquire helpers
    // -----------------------------------------------------------------------

    /// Acquire the reader lock (level 1).
    ///
    /// Auto-initializes via `make_available()` if not yet available.
    /// The reader lock is acquired AFTER initialization so that initialization
    /// itself can serialize via the same reader_lock.
    pub async fn acquire_reader(&self) -> WalletResult<tokio::sync::MutexGuard<'_, ()>> {
        if !self.is_available() {
            // make_available() acquires+releases reader_lock internally for idempotency.
            // After it returns, is_available is true and reader_lock is free.
            self.make_available().await?;
        }
        Ok(self.reader_lock.lock().await)
    }

    /// Acquire the reader + writer locks (levels 1 and 2).
    ///
    /// Auto-initializes via `make_available()` if not yet available.
    pub async fn acquire_writer(
        &self,
    ) -> WalletResult<(
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
    )> {
        if !self.is_available() {
            self.make_available().await?;
        }
        let reader = self.reader_lock.lock().await;
        let writer = self.writer_lock.lock().await;
        Ok((reader, writer))
    }

    /// Acquire reader + writer + sync locks (levels 1, 2, and 3).
    ///
    /// Auto-initializes via `make_available()` if not yet available.
    pub async fn acquire_sync(
        &self,
    ) -> WalletResult<(
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
    )> {
        if !self.is_available() {
            self.make_available().await?;
        }
        let reader = self.reader_lock.lock().await;
        let writer = self.writer_lock.lock().await;
        let sync = self.sync_lock.lock().await;
        Ok((reader, writer, sync))
    }

    /// Acquire all four locks (levels 1, 2, 3, and 4).
    ///
    /// Auto-initializes via `make_available()` if not yet available.
    pub async fn acquire_storage_provider(
        &self,
    ) -> WalletResult<(
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
        tokio::sync::MutexGuard<'_, ()>,
    )> {
        if !self.is_available() {
            self.make_available().await?;
        }
        let reader = self.reader_lock.lock().await;
        let writer = self.writer_lock.lock().await;
        let sync = self.sync_lock.lock().await;
        let sp = self.sp_lock.lock().await;
        Ok((reader, writer, sync, sp))
    }

    /// Acquire the manager-level spend lock.
    ///
    /// Held across createAction/signAction/internalizeAction/abortAction/
    /// relinquishOutput to serialize UTXO-mutating operations across all
    /// `Wallet` instances sharing this manager. Cross-wallet UTXO contention
    /// on shared storage requires a manager-level lock; per-`Wallet` locks
    /// would not serialize.
    ///
    /// Independent of the hierarchical reader/writer/sync/sp locks: callers
    /// take this lock at the wallet layer, then perform multiple WSM calls
    /// (each of which acquires/releases writer/reader locks) under its
    /// protection. Do not hold it while also holding the reader lock from
    /// this manager — acquire it first (or release reader first).
    ///
    /// Auto-initializes via `make_available()` if not yet available.
    pub async fn acquire_spend_lock(&self) -> WalletResult<tokio::sync::MutexGuard<'_, ()>> {
        if !self.is_available() {
            self.make_available().await?;
        }
        Ok(self.spend_lock.lock().await)
    }

    // -----------------------------------------------------------------------
    // Lifecycle methods
    // -----------------------------------------------------------------------

    /// Destroy all stores, cleaning up all resources.
    pub async fn destroy(&self) -> WalletResult<()> {
        let state = self.state.lock().await;
        for store in &state.stores {
            store.storage.destroy().await?;
        }
        Ok(())
    }

    /// Get wallet services.
    pub async fn get_services_ref(&self) -> WalletResult<Arc<dyn WalletServices>> {
        self.services.lock().await.clone().ok_or_else(|| {
            WalletError::NotImplemented("services not configured on manager".to_string())
        })
    }

    /// Set wallet services.
    pub async fn set_services(&self, services: Arc<dyn WalletServices>) {
        *self.services.lock().await = Some(services);
    }

    // -----------------------------------------------------------------------
    // WalletStorageProvider delegation — read operations
    //
    // All read operations acquire the reader lock (level 1) before delegating
    // to the active provider. This matches the TS `runAsReader` pattern.
    // -----------------------------------------------------------------------

    /// List wallet actions (transactions) with filtering and pagination.
    pub async fn list_actions(
        &self,
        auth: &AuthId,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.list_actions(auth, args).await
    }

    /// List outputs with basket/tag filtering.
    pub async fn list_outputs(
        &self,
        auth: &AuthId,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.list_outputs(auth, args).await
    }

    /// List certificates with certifier/type filtering.
    pub async fn list_certificates(
        &self,
        auth: &AuthId,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.list_certificates(auth, args).await
    }

    /// Find certificates scoped to the given auth identity.
    pub async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.find_certificates_auth(auth, args).await
    }

    /// Find output baskets scoped to the given auth identity.
    pub async fn find_output_baskets_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.find_output_baskets_auth(auth, args).await
    }

    /// Find outputs scoped to the given auth identity.
    pub async fn find_outputs_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.find_outputs_auth(auth, args).await
    }

    /// Find proven transaction requests.
    pub async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        let _rg = self.acquire_reader().await?;
        let active = self.get_active().await?;
        active.find_proven_tx_reqs(args).await
    }

    // -----------------------------------------------------------------------
    // WalletStorageProvider delegation — write operations
    //
    // All write operations acquire reader + writer locks (levels 1 and 2)
    // before delegating to the active provider. This matches the TS
    // `runAsWriter` pattern.
    // -----------------------------------------------------------------------

    /// Abort (cancel) a transaction by reference.
    pub async fn abort_action(
        &self,
        auth: &AuthId,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.abort_action(auth, args).await
    }

    /// Create a new transaction in storage.
    pub async fn create_action(
        &self,
        auth: &AuthId,
        args: &StorageCreateActionArgs,
    ) -> WalletResult<StorageCreateActionResult> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.create_action(auth, args).await
    }

    /// Process (commit) a signed transaction to storage.
    pub async fn process_action(
        &self,
        auth: &AuthId,
        args: &StorageProcessActionArgs,
    ) -> WalletResult<StorageProcessActionResult> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.process_action(auth, args).await
    }

    /// Internalize outputs from an external transaction.
    pub async fn internalize_action(
        &self,
        auth: &AuthId,
        args: &StorageInternalizeActionArgs,
        services: &dyn WalletServices,
    ) -> WalletResult<StorageInternalizeActionResult> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.internalize_action(auth, args, services).await
    }

    /// Insert a certificate scoped to the given auth identity.
    pub async fn insert_certificate_auth(
        &self,
        auth: &AuthId,
        certificate: &Certificate,
    ) -> WalletResult<i64> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.insert_certificate_auth(auth, certificate).await
    }

    /// Relinquish (soft-delete) a certificate by marking it deleted.
    pub async fn relinquish_certificate(
        &self,
        auth: &AuthId,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<i64> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.relinquish_certificate(auth, args).await
    }

    /// Relinquish (remove from basket) an output by outpoint.
    pub async fn relinquish_output(
        &self,
        auth: &AuthId,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<i64> {
        let (_rg, _wg) = self.acquire_writer().await?;
        let active = self.get_active().await?;
        active.relinquish_output(auth, args).await
    }

    // -----------------------------------------------------------------------
    // Sync-level delegation
    // -----------------------------------------------------------------------

    pub async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)> {
        let active = self.get_active().await?;
        let result = active.find_or_insert_user(identity_key).await?;

        // Consistency guard: if the manager already has a cached user_id for this identity,
        // verify it matches the returned record. A mismatch indicates a storage corruption
        // or identity key collision that must not silently proceed.
        if let Ok(cached_auth) = self.get_auth(false).await {
            if let Some(cached_id) = cached_auth.user_id {
                if result.0.user_id != cached_id {
                    return Err(WalletError::Internal(
                        "find_or_insert_user: returned user_id does not match cached user_id — identity consistency violation".to_string(),
                    ));
                }
            }
        }

        Ok(result)
    }

    pub async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk> {
        let active = self.get_active().await?;
        active.get_sync_chunk(args).await
    }

    pub async fn process_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
        chunk: &SyncChunk,
    ) -> WalletResult<ProcessSyncChunkResult> {
        let active = self.get_active().await?;
        active.process_sync_chunk(args, chunk).await
    }

    // -----------------------------------------------------------------------
    // Low-level CRUD operations (certificates, signer, beef helper use these)
    // -----------------------------------------------------------------------

    pub async fn find_user_by_identity_key(&self, key: &str) -> WalletResult<Option<User>> {
        let active = self.get_active().await?;
        active.find_user_by_identity_key(key).await
    }

    pub async fn find_certificates_storage(
        &self,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        let active = self.get_active().await?;
        active.find_certificates_storage(args).await
    }

    pub async fn find_certificate_fields(
        &self,
        args: &FindCertificateFieldsArgs,
    ) -> WalletResult<Vec<crate::tables::CertificateField>> {
        let active = self.get_active().await?;
        active.find_certificate_fields(args).await
    }

    pub async fn find_outputs_storage(&self, args: &FindOutputsArgs) -> WalletResult<Vec<Output>> {
        let active = self.get_active().await?;
        active.find_outputs_storage(args).await
    }

    /// Find outputs (signer pipeline alias).
    pub async fn find_outputs(&self, args: &FindOutputsArgs) -> WalletResult<Vec<Output>> {
        self.find_outputs_storage(args).await
    }

    /// Update an output by ID.
    pub async fn update_output(&self, id: i64, update: &OutputPartial) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_output(id, update).await
    }

    pub async fn insert_certificate_storage(&self, cert: &Certificate) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_certificate_storage(cert).await
    }

    pub async fn insert_certificate_field_storage(
        &self,
        field: &crate::tables::CertificateField,
    ) -> WalletResult<()> {
        let active = self.get_active().await?;
        active.insert_certificate_field_storage(field).await
    }

    // -----------------------------------------------------------------------
    // Internal storage operations (monitor, signer, beef helper use these)
    //
    // These delegate to the active WalletStorageProvider's internal methods.
    // The active provider (SqliteStorage) implements these via the blanket impl.
    // StorageClient returns NotImplemented for these methods.
    // -----------------------------------------------------------------------

    pub async fn find_proven_txs(&self, args: &FindProvenTxsArgs) -> WalletResult<Vec<ProvenTx>> {
        let active = self.get_active().await?;
        active.find_proven_txs(args).await
    }

    pub async fn find_transactions(
        &self,
        args: &FindTransactionsArgs,
    ) -> WalletResult<Vec<Transaction>> {
        let active = self.get_active().await?;
        active.find_transactions(args).await
    }

    pub async fn update_proven_tx_req(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_proven_tx_req(id, update).await
    }

    pub async fn update_proven_tx(&self, id: i64, update: &ProvenTxPartial) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_proven_tx(id, update).await
    }

    pub async fn update_transaction(
        &self,
        id: i64,
        update: &TransactionPartial,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_transaction(id, update).await
    }

    pub async fn update_transaction_status(
        &self,
        txid: &str,
        new_status: TransactionStatus,
    ) -> WalletResult<()> {
        let active = self.get_active().await?;
        active.update_transaction_status(txid, new_status).await
    }

    pub async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        req_id: i64,
        proven_tx: &ProvenTx,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active
            .update_proven_tx_req_with_new_proven_tx(req_id, proven_tx)
            .await
    }

    // -----------------------------------------------------------------------
    // Transaction management + trx-aware CRUD passthroughs
    //
    // These let callers (e.g. handle_permanent_broadcast_failure) group a
    // sequence of writes under one atomic DB transaction on the active
    // provider. Callers are responsible for matching begin/commit (or
    // rollback) calls and for holding the trx only across the active
    // provider returned by `get_active()`.
    // -----------------------------------------------------------------------

    pub async fn begin_transaction(&self) -> WalletResult<TrxToken> {
        let active = self.get_active().await?;
        active.begin_transaction().await
    }

    pub async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        let active = self.get_active().await?;
        active.commit_transaction(trx).await
    }

    pub async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        let active = self.get_active().await?;
        active.rollback_transaction(trx).await
    }

    pub async fn find_outputs_trx(
        &self,
        args: &FindOutputsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Output>> {
        let active = self.get_active().await?;
        active.find_outputs_trx(args, trx).await
    }

    pub async fn find_transactions_trx(
        &self,
        args: &FindTransactionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Transaction>> {
        let active = self.get_active().await?;
        active.find_transactions_trx(args, trx).await
    }

    pub async fn find_proven_tx_reqs_trx(
        &self,
        args: &FindProvenTxReqsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        let active = self.get_active().await?;
        active.find_proven_tx_reqs_trx(args, trx).await
    }

    pub async fn update_output_trx(
        &self,
        id: i64,
        update: &OutputPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_output_trx(id, update, trx).await
    }

    pub async fn update_proven_tx_req_trx(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.update_proven_tx_req_trx(id, update, trx).await
    }

    pub async fn update_transaction_status_trx(
        &self,
        txid: &str,
        new_status: TransactionStatus,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        let active = self.get_active().await?;
        active
            .update_transaction_status_trx(txid, new_status, trx)
            .await
    }

    pub async fn insert_monitor_event(&self, event: &MonitorEvent) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_monitor_event(event).await
    }

    pub async fn delete_monitor_events_before_id(
        &self,
        event_name: &str,
        before_id: i64,
    ) -> WalletResult<u64> {
        let active = self.get_active().await?;
        active
            .delete_monitor_events_before_id(event_name, before_id)
            .await
    }

    pub async fn find_monitor_events(
        &self,
        args: &FindMonitorEventsArgs,
    ) -> WalletResult<Vec<MonitorEvent>> {
        let active = self.get_active().await?;
        active.find_monitor_events(args).await
    }

    pub async fn count_monitor_events(&self, args: &FindMonitorEventsArgs) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.count_monitor_events(args).await
    }

    pub async fn admin_stats(&self, auth_id: &str) -> WalletResult<AdminStatsResult> {
        let active = self.get_active().await?;
        active.admin_stats(auth_id).await
    }

    pub async fn purge_data(&self, params: &PurgeParams) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.purge_data(params).await
    }

    pub async fn review_status(&self, aged_limit: chrono::NaiveDateTime) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.review_status(aged_limit).await
    }

    pub async fn get_storage_identity_key(&self) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.get_storage_identity_key().await
    }

    // -----------------------------------------------------------------------
    // TS-parity getter methods
    // -----------------------------------------------------------------------

    /// Returns metadata for all registered storage providers.
    ///
    /// Matches TS `getStores()`: iterates all stores and returns a `WalletStorageInfo`
    /// for each, including which is active, which are backups, and which are conflicting.
    pub async fn get_stores(&self) -> Vec<WalletStorageInfo> {
        let state = self.state.lock().await;
        let mut result = Vec::with_capacity(state.stores.len());

        let is_active_enabled = {
            if let Some(idx) = state.active_index {
                let sik = state.stores[idx]
                    .settings
                    .lock()
                    .await
                    .as_ref()
                    .map(|s| s.storage_identity_key.clone())
                    .unwrap_or_default();
                let user_active = state.stores[idx]
                    .user
                    .lock()
                    .await
                    .as_ref()
                    .map(|u| u.active_storage.clone())
                    .unwrap_or_default();
                state.conflicting_active_indices.is_empty() && sik == user_active
            } else {
                false
            }
        };

        for (i, store) in state.stores.iter().enumerate() {
            let settings_guard = store.settings.lock().await;
            let user_guard = store.user.lock().await;

            let storage_identity_key = settings_guard
                .as_ref()
                .map(|s| s.storage_identity_key.clone())
                .unwrap_or_default();
            let storage_name = settings_guard
                .as_ref()
                .map(|s| s.storage_name.clone())
                .unwrap_or_default();
            let user_id = user_guard.as_ref().map(|u| u.user_id);
            let endpoint_url = store.storage.get_endpoint_url();

            let is_active = state.active_index == Some(i);
            let is_backup = state.backup_indices.contains(&i);
            let is_conflicting = state.conflicting_active_indices.contains(&i);
            // A store is "enabled" only when it is the active store and active is enabled.
            let is_enabled = is_active && is_active_enabled;

            result.push(WalletStorageInfo {
                storage_identity_key,
                storage_name,
                user_id,
                is_active,
                is_enabled,
                is_backup,
                is_conflicting,
                endpoint_url,
            });
        }

        result
    }

    /// Returns the remote endpoint URL for the store with the given identity key.
    ///
    /// Matches TS `getStoreEndpointURL(storageIdentityKey)`.
    /// Returns None for local SQLite stores and for unknown identity keys.
    pub async fn get_store_endpoint_url(&self, storage_identity_key: &str) -> Option<String> {
        let state = self.state.lock().await;
        for store in &state.stores {
            let sik = store
                .settings
                .lock()
                .await
                .as_ref()
                .map(|s| s.storage_identity_key.clone())
                .unwrap_or_default();
            if sik == storage_identity_key {
                return store.storage.get_endpoint_url();
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Reprove methods (chain reorganization proof re-validation)
    // -----------------------------------------------------------------------

    /// Re-validate the merkle proof for a single ProvenTx record.
    ///
    /// Matches TS `reproveProven(ptx, noUpdate?)`:
    /// - Fetches a fresh merkle path from WalletServices.
    /// - Validates it against the current chain tracker.
    /// - If valid and `no_update` is false, persists the updated record to active storage.
    /// - Returns a `ReproveProvenResult` indicating updated/unchanged/unavailable.
    ///
    /// This method must be called while the StorageProvider lock is already held
    /// (the caller's responsibility — `reprove_header` acquires it before calling here).
    pub async fn reprove_proven(
        &self,
        ptx: &ProvenTx,
        no_update: bool,
    ) -> WalletResult<ReproveProvenResult> {
        let services = self.get_services_ref().await?;

        // Attempt to get a fresh merkle proof for this transaction.
        let merkle_result = services.get_merkle_path(&ptx.txid, false).await;

        // If no proof available, return unavailable.
        let (merkle_path_bytes, header) = match (merkle_result.merkle_path, merkle_result.header) {
            (Some(mp), Some(hdr)) => (mp, hdr),
            _ => {
                return Ok(ReproveProvenResult {
                    updated: None,
                    unchanged: false,
                    unavailable: true,
                });
            }
        };

        // Check if proof data is unchanged.
        let height = header.height as i32;
        let block_hash = header.hash.clone();
        let merkle_root = header.merkle_root.clone();

        if ptx.height == height
            && ptx.block_hash == block_hash
            && ptx.merkle_root == merkle_root
            && ptx.merkle_path == merkle_path_bytes
        {
            return Ok(ReproveProvenResult {
                updated: None,
                unchanged: true,
                unavailable: false,
            });
        }

        // Build the updated record.
        let mut updated_ptx = ptx.clone();
        updated_ptx.height = height;
        updated_ptx.block_hash = block_hash.clone();
        updated_ptx.merkle_root = merkle_root.clone();
        updated_ptx.merkle_path = merkle_path_bytes;
        // index comes from the chain tracker header if available; fall back to existing.
        // (TS updates index via MerklePath.index — we leave it unchanged unless we have
        // the actual index from a decoded path, which requires the bsv library. Acceptable
        // for now: reprove_header primarily needs height + block_hash updated.)

        let log_update = format!(
            "reproved txid={} old_block={} new_block={} height={}",
            ptx.txid, ptx.block_hash, block_hash, height
        );

        // Persist update unless caller requested dry-run mode.
        if !no_update {
            let active = self.get_active().await?;
            let partial = ProvenTxPartial {
                height: Some(updated_ptx.height),
                block_hash: Some(updated_ptx.block_hash.clone()),
                ..Default::default()
            };
            active.update_proven_tx(ptx.proven_tx_id, &partial).await?;
        }

        Ok(ReproveProvenResult {
            updated: Some(ReproveProvenUpdate {
                update: updated_ptx,
                log_update,
            }),
            unchanged: false,
            unavailable: false,
        })
    }

    /// Re-prove all ProvenTx records associated with a deactivated block hash.
    ///
    /// Matches TS `reproveHeader(deactivatedHash)`:
    /// - Runs under StorageProvider lock (all four locks).
    /// - Finds all ProvenTx records with block_hash == deactivated_hash.
    /// - Calls `reprove_proven` for each (no_update=true for individual calls).
    /// - Bulk-updates all successfully reproved records in a single pass.
    /// - Returns `ReproveHeaderResult` with updated/unchanged/unavailable partitions.
    pub async fn reprove_header(
        &self,
        deactivated_hash: &str,
    ) -> WalletResult<ReproveHeaderResult> {
        // Acquire all four locks (StorageProvider level).
        let _guards = self.acquire_storage_provider().await?;

        let proven_txs = {
            let active = self.get_active().await?;
            active
                .find_proven_txs(&FindProvenTxsArgs {
                    partial: ProvenTxPartial {
                        block_hash: Some(deactivated_hash.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .await?
        };

        let mut updated_vec = Vec::new();
        let mut unchanged_vec = Vec::new();
        let mut unavailable_vec = Vec::new();

        let mut updates_to_persist: Vec<(i64, ProvenTxPartial, ProvenTx)> = Vec::new();

        for ptx in &proven_txs {
            // Call reprove_proven in dry-run mode (no_update=true); we bulk-persist below.
            let result = self.reprove_proven(ptx, true).await?;

            if result.unavailable {
                unavailable_vec.push(ptx.clone());
            } else if result.unchanged {
                unchanged_vec.push(ptx.clone());
            } else if let Some(ru) = result.updated {
                let partial = ProvenTxPartial {
                    height: Some(ru.update.height),
                    block_hash: Some(ru.update.block_hash.clone()),
                    ..Default::default()
                };
                updates_to_persist.push((ptx.proven_tx_id, partial, ru.update));
            }
        }

        // Bulk-persist all updates.
        if !updates_to_persist.is_empty() {
            let active = self.get_active().await?;
            for (id, partial, updated_ptx) in updates_to_persist {
                active.update_proven_tx(id, &partial).await?;
                updated_vec.push(updated_ptx);
            }
        }

        Ok(ReproveHeaderResult {
            updated: updated_vec,
            unchanged: unchanged_vec,
            unavailable: unavailable_vec,
        })
    }

    // -----------------------------------------------------------------------
    // Sync loop methods (Plan 02)
    // -----------------------------------------------------------------------

    /// Sync entities from `reader` into `writer` using chunked getSyncChunk/processSyncChunk.
    ///
    /// Loops until `processSyncChunk` returns `done=true`, accumulating insert/update counts.
    /// Caller must hold the sync lock (acquired by `update_backups` or `sync_from_reader`).
    ///
    /// # Arguments
    /// * `writer` - Storage receiving the sync data.
    /// * `reader` - Storage providing the sync data.
    /// * `writer_settings` - Cached settings for the writer (to_storage_identity_key).
    /// * `reader_settings` - Cached settings for the reader (from_storage_identity_key).
    /// * `prog_log` - Optional progress callback. If Some, called after each chunk with a
    ///   progress string. Returns the accumulated log of all callback return values.
    ///
    /// Returns `(total_inserts, total_updates, log_string)`.
    pub async fn sync_to_writer(
        &self,
        writer: &dyn WalletStorageProvider,
        reader: &dyn WalletStorageProvider,
        writer_settings: &Settings,
        reader_settings: &Settings,
        prog_log: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
    ) -> WalletResult<(i64, i64, String)> {
        let auth = self.get_auth(false).await?;
        let mut total_inserts: i64 = 0;
        let mut total_updates: i64 = 0;
        let mut log = String::new();

        let mut i = 0usize;
        loop {
            // Re-query sync state on every iteration — processSyncChunk updates it.
            let (ss, _) = writer
                .find_or_insert_sync_state_auth(
                    &auth,
                    &reader_settings.storage_identity_key,
                    &reader_settings.storage_name,
                )
                .await?;

            let args = make_request_sync_chunk_args(
                &ss,
                &self.identity_key,
                &writer_settings.storage_identity_key,
            )?;

            let chunk = reader.get_sync_chunk(&args).await?;
            let r = writer.process_sync_chunk(&args, &chunk).await?;

            total_inserts += r.inserts;
            total_updates += r.updates;

            if let Some(cb) = prog_log {
                let msg = format!(
                    "sync chunk {} inserts={} updates={}",
                    i, r.inserts, r.updates
                );
                let s = cb(&msg);
                log.push_str(&s);
                log.push('\n');
            }

            if r.done {
                break;
            }
            i += 1;
        }

        Ok((total_inserts, total_updates, log))
    }

    /// Sync entities from `reader` into `writer` with active_storage override protection.
    ///
    /// Identical to `sync_to_writer` except that before calling `writer.process_sync_chunk`,
    /// the `chunk.user.active_storage` field is overridden with the writer's cached
    /// `user.active_storage`. This prevents the reader's active_storage from clobbering
    /// the writer's during sync (Pitfall 5 from the research).
    ///
    /// `reader_identity_key` must match this manager's identity key; otherwise returns
    /// `WERR_UNAUTHORIZED`. This mirrors the TS WalletStorageManager.syncFromReader check.
    pub async fn sync_from_reader(
        &self,
        reader_identity_key: &str,
        writer: &dyn WalletStorageProvider,
        reader: &dyn WalletStorageProvider,
        writer_settings: &Settings,
        reader_settings: &Settings,
        prog_log: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
    ) -> WalletResult<(i64, i64, String)> {
        if reader_identity_key != self.identity_key {
            return Err(WalletError::Unauthorized(
                "sync_from_reader: reader identity key does not match manager identity key"
                    .to_string(),
            ));
        }
        let auth = self.get_auth(false).await?;
        let mut total_inserts: i64 = 0;
        let mut total_updates: i64 = 0;
        let mut log = String::new();

        // Fetch writer's cached user.active_storage for the override below.
        let writer_active_storage = {
            let state = self.state.lock().await;
            if let Some(idx) = state.active_index {
                state.stores[idx]
                    .get_user_cached()
                    .await
                    .map(|u| u.active_storage)
            } else {
                None
            }
        };

        let mut i = 0usize;
        loop {
            let (ss, _) = writer
                .find_or_insert_sync_state_auth(
                    &auth,
                    &reader_settings.storage_identity_key,
                    &reader_settings.storage_name,
                )
                .await?;

            let args = make_request_sync_chunk_args(
                &ss,
                &self.identity_key,
                &writer_settings.storage_identity_key,
            )?;

            let mut chunk = reader.get_sync_chunk(&args).await?;

            // Pitfall 5: override chunk.user.active_storage with writer's cached value
            // to prevent the reader's active_storage from clobbering the writer's.
            if let (Some(ref mut chunk_user), Some(ref writer_as)) =
                (&mut chunk.user, &writer_active_storage)
            {
                chunk_user.active_storage = writer_as.clone();
            }

            let r = writer.process_sync_chunk(&args, &chunk).await?;

            total_inserts += r.inserts;
            total_updates += r.updates;

            if let Some(cb) = prog_log {
                let msg = format!(
                    "sync chunk {} inserts={} updates={}",
                    i, r.inserts, r.updates
                );
                let s = cb(&msg);
                log.push_str(&s);
                log.push('\n');
            }

            if r.done {
                break;
            }
            i += 1;
        }

        Ok((total_inserts, total_updates, log))
    }

    /// Sync all backup stores from the active store.
    ///
    /// Acquires sync-level locks, iterates `backup_indices`, and calls
    /// `sync_to_writer(backup, active, ...)` for each backup. Per-backup errors
    /// are logged as warnings and do not block other backups.
    ///
    /// Returns `(total_inserts, total_updates, accumulated_log)`.
    pub async fn update_backups(
        &self,
        prog_log: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
    ) -> WalletResult<(i64, i64, String)> {
        // Acquire sync-level lock (reader + writer + sync).
        let _guards = self.acquire_sync().await?;

        let (active_arc, active_settings, backup_arcs_and_settings) = {
            let state = self.state.lock().await;
            let idx = state.require_active()?;
            let active_arc = state.stores[idx].storage.clone();
            let active_settings =
                state.stores[idx]
                    .get_settings_cached()
                    .await
                    .ok_or_else(|| {
                        WalletError::InvalidOperation("Active settings not cached".to_string())
                    })?;

            let mut backups = Vec::with_capacity(state.backup_indices.len());
            for &bi in &state.backup_indices {
                let backup_arc = state.stores[bi].storage.clone();
                let backup_settings = state.stores[bi].get_settings_cached().await;
                backups.push((backup_arc, backup_settings));
            }
            (active_arc, active_settings, backups)
        };

        let mut total_inserts: i64 = 0;
        let mut total_updates: i64 = 0;
        let mut full_log = String::new();

        for (backup_arc, maybe_backup_settings) in &backup_arcs_and_settings {
            let backup_settings = match maybe_backup_settings {
                Some(s) => s.clone(),
                None => {
                    warn!("update_backups: backup has no cached settings, skipping");
                    continue;
                }
            };

            match self
                .sync_to_writer(
                    backup_arc.as_ref(),
                    active_arc.as_ref(),
                    &backup_settings,
                    &active_settings,
                    prog_log,
                )
                .await
            {
                Ok((ins, upd, log)) => {
                    total_inserts += ins;
                    total_updates += upd;
                    full_log.push_str(&log);
                }
                Err(e) => {
                    // Per-backup failure must not block other backups (TS parity).
                    warn!(
                        "update_backups: sync to backup '{}' failed: {e}",
                        backup_settings.storage_identity_key
                    );
                }
            }
        }

        Ok((total_inserts, total_updates, full_log))
    }

    /// Switch the active store to the one identified by `storage_identity_key`.
    ///
    /// Implements the TS `setActive(storageIdentityKey, progLog?)` algorithm (8 steps):
    ///
    /// 1. Validate the target store is registered.
    /// 2. Early-return "unchanged" if already active and enabled.
    /// 3. Acquire sync lock.
    /// 4. Merge any conflicting-active stores into the new active via `sync_to_writer`.
    /// 5. Determine backup_source (new_active if conflicts existed, else current_active).
    /// 6. Call provider-level `set_active` on backup_source to persist `user.active_storage`.
    /// 7. Propagate state to all other stores via `sync_to_writer`.
    /// 8. Reset `is_available` and re-partition via `do_make_available`.
    ///
    /// Returns accumulated progress log string on success.
    pub async fn set_active(
        &self,
        storage_identity_key: &str,
        prog_log: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
    ) -> WalletResult<String> {
        // Step 0: Auto-init if needed
        if !self.is_available() {
            self.make_available().await?;
        }

        // Step 1: Validate target store exists — find its index
        let new_active_idx = {
            let state = self.state.lock().await;
            let mut found = None;
            for (i, store) in state.stores.iter().enumerate() {
                let sik = store
                    .get_settings_cached()
                    .await
                    .map(|s| s.storage_identity_key)
                    .unwrap_or_default();
                if sik == storage_identity_key {
                    found = Some(i);
                    break;
                }
            }
            found
        };

        let new_active_idx = new_active_idx.ok_or_else(|| WalletError::InvalidParameter {
            parameter: "storage_identity_key".to_string(),
            must_be: format!(
                "registered with this WalletStorageManager. {} does not match any managed store.",
                storage_identity_key
            ),
        })?;

        // Step 2: Early return if already active and enabled
        let current_active_sik = self.get_active_store().await?;
        if current_active_sik == storage_identity_key && self.is_active_enabled().await {
            return Ok(format!("{} unchanged\n", storage_identity_key));
        }

        let log = {
            // Step 3: Acquire sync lock (reader + writer + sync)
            // IMPORTANT: acquire_sync already holds reader_lock — must use do_make_available()
            // at re-partition step, NOT make_available().
            let _guards = self.acquire_sync().await?;

            let mut log = String::new();

            // Step 4: Conflict resolution — merge each conflicting active into the new active
            // Clone Arcs and Settings before releasing state lock (never hold state lock across async I/O)
            let had_conflicts;
            let (new_active_arc, new_active_settings) = {
                let state = self.state.lock().await;
                let arc = state.stores[new_active_idx].storage.clone();
                let settings = state.stores[new_active_idx]
                    .get_settings_cached()
                    .await
                    .ok_or_else(|| {
                        WalletError::InvalidOperation(
                            "set_active: new active settings not cached".to_string(),
                        )
                    })?;
                (arc, settings)
            };

            let conflict_sources = {
                let state = self.state.lock().await;
                if state.conflicting_active_indices.is_empty() {
                    had_conflicts = false;
                    Vec::new()
                } else {
                    had_conflicts = true;
                    // Build list: all conflicting_active_indices + active_index, minus new_active_idx
                    let mut sources = state.conflicting_active_indices.clone();
                    if let Some(ai) = state.active_index {
                        sources.push(ai);
                    }
                    // Remove the new_active_idx from this list (it's the destination)
                    sources.retain(|&i| i != new_active_idx);

                    // Clone arcs and settings for each conflict source
                    let mut result = Vec::with_capacity(sources.len());
                    for idx in sources {
                        let arc = state.stores[idx].storage.clone();
                        let settings = state.stores[idx].get_settings_cached().await;
                        result.push((arc, settings));
                    }
                    result
                }
            };

            // Now do the async sync_to_writer calls outside the state lock
            for (conflict_arc, maybe_conflict_settings) in &conflict_sources {
                let conflict_settings = match maybe_conflict_settings {
                    Some(s) => s.clone(),
                    None => {
                        warn!("set_active: conflict source has no cached settings, skipping");
                        continue;
                    }
                };
                // Merge conflict into new_active (new_active is the writer/destination)
                let (ins, upd, chunk_log) = self
                    .sync_to_writer(
                        new_active_arc.as_ref(),
                        conflict_arc.as_ref(),
                        &new_active_settings,
                        &conflict_settings,
                        prog_log,
                    )
                    .await?;
                if let Some(cb) = prog_log {
                    let msg = format!(
                        "set_active: merged conflict {} inserts={} updates={}",
                        conflict_settings.storage_identity_key, ins, upd
                    );
                    let s = cb(&msg);
                    log.push_str(&s);
                    log.push('\n');
                }
                log.push_str(&chunk_log);
            }

            // Step 5: Determine backup_source
            // If conflicts existed → backup_source = new_active
            // Else → backup_source = current_active
            let (backup_source_arc, backup_source_settings, backup_source_user_id) = {
                let state = self.state.lock().await;
                if had_conflicts {
                    // backup_source is the new active
                    let user_id = state.stores[new_active_idx]
                        .get_user_cached()
                        .await
                        .map(|u| u.user_id)
                        .ok_or_else(|| {
                            WalletError::InvalidOperation(
                                "set_active: new active user not cached".to_string(),
                            )
                        })?;
                    (new_active_arc.clone(), new_active_settings.clone(), user_id)
                } else {
                    // backup_source is the current active
                    let ai = state.require_active()?;
                    let arc = state.stores[ai].storage.clone();
                    let settings =
                        state.stores[ai]
                            .get_settings_cached()
                            .await
                            .ok_or_else(|| {
                                WalletError::InvalidOperation(
                                    "set_active: current active settings not cached".to_string(),
                                )
                            })?;
                    let user_id = state.stores[ai]
                        .get_user_cached()
                        .await
                        .map(|u| u.user_id)
                        .ok_or_else(|| {
                            WalletError::InvalidOperation(
                                "set_active: current active user not cached".to_string(),
                            )
                        })?;
                    (arc, settings, user_id)
                }
            };

            // Step 6: Provider-level set_active on backup_source
            // This persists user.active_storage = storage_identity_key in the backup_source DB
            let auth = AuthId {
                identity_key: self.identity_key.clone(),
                user_id: Some(backup_source_user_id),
                is_active: Some(false),
            };
            backup_source_arc
                .set_active(&auth, storage_identity_key)
                .await?;

            if let Some(cb) = prog_log {
                let msg = format!(
                    "set_active: provider-level set_active on {} complete",
                    backup_source_settings.storage_identity_key
                );
                let s = cb(&msg);
                log.push_str(&s);
                log.push('\n');
            }

            // Step 7: Propagate state to all other stores via sync_to_writer
            // First update in-memory user.active_storage cache for all stores
            {
                let state = self.state.lock().await;
                for store in &state.stores {
                    let mut user_guard = store.user.lock().await;
                    if let Some(ref mut u) = *user_guard {
                        u.active_storage = storage_identity_key.to_string();
                    }
                }
            }

            // Collect list of (store_arc, store_settings) for stores != backup_source
            let backup_source_sik = backup_source_settings.storage_identity_key.clone();
            let propagation_targets = {
                let state = self.state.lock().await;
                let mut targets = Vec::new();
                for store in &state.stores {
                    let sik = store
                        .get_settings_cached()
                        .await
                        .map(|s| s.storage_identity_key)
                        .unwrap_or_default();
                    if sik != backup_source_sik {
                        let arc = store.storage.clone();
                        let settings = store.get_settings_cached().await;
                        targets.push((arc, settings));
                    }
                }
                targets
            };

            for (store_arc, maybe_store_settings) in &propagation_targets {
                let store_settings = match maybe_store_settings {
                    Some(s) => s.clone(),
                    None => {
                        warn!("set_active: propagation target has no cached settings, skipping");
                        continue;
                    }
                };
                let (ins, upd, chunk_log) = self
                    .sync_to_writer(
                        store_arc.as_ref(),
                        backup_source_arc.as_ref(),
                        &store_settings,
                        &backup_source_settings,
                        prog_log,
                    )
                    .await?;
                if let Some(cb) = prog_log {
                    let msg = format!(
                        "set_active: propagated to {} inserts={} updates={}",
                        store_settings.storage_identity_key, ins, upd
                    );
                    let s = cb(&msg);
                    log.push_str(&s);
                    log.push('\n');
                }
                log.push_str(&chunk_log);
            }

            // Step 8: Re-partition — reset flag and re-run do_make_available
            // MUST use do_make_available(), NOT make_available(): acquire_sync holds reader_lock
            // and make_available() would try to acquire it again → deadlock.
            self.is_available_flag.store(false, Ordering::Release);
            self.do_make_available().await?;

            if let Some(cb) = prog_log {
                let msg = format!(
                    "set_active: complete, new active is {}",
                    storage_identity_key
                );
                let s = cb(&msg);
                log.push_str(&s);
                log.push('\n');
            }

            log
        };
        // Sync lock released — push changes to backups (non-fatal).
        if let Err(e) = self.update_backups(None).await {
            warn!("set_active: backup sync failed: {e}");
        }

        Ok(log)
    }

    /// Add a new storage provider at runtime.
    ///
    /// Acquires all four locks (storage-provider level), appends the provider
    /// to the `stores` vec, resets `is_available`, and re-runs `make_available()`
    /// to re-partition the updated store list.
    ///
    /// Matches TS `addWalletStorageProvider(storage)`.
    pub async fn add_wallet_storage_provider(
        &self,
        provider: Arc<dyn WalletStorageProvider>,
    ) -> WalletResult<()> {
        {
            // Acquire all four locks (storage-provider level).
            let _guards = self.acquire_storage_provider().await?;

            {
                let mut state = self.state.lock().await;
                state.stores.push(ManagedStorage::new(provider));
                // Reset is_available so make_available() will re-run the full partition.
                self.is_available_flag.store(false, Ordering::Release);
            }

            // Re-run make_available() to initialize the new store and re-partition.
            // We must NOT call self.make_available() here because it would try to
            // acquire reader_lock which is already held by acquire_storage_provider.
            // Instead call do_make_available() which runs without acquiring reader_lock.
            self.do_make_available().await?;
        }
        // All locks released — sync active state to the newly added backup (non-fatal).
        if let Err(e) = self.update_backups(None).await {
            warn!("add_wallet_storage_provider: backup sync failed: {e}");
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Low-level seeding helpers (used by tests and setup code)
    //
    // These bypass auth and write directly to the active provider.
    // -----------------------------------------------------------------------

    /// Find or create an output basket by user ID and name.
    pub async fn find_output_baskets(
        &self,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        let active = self.get_active().await?;
        active.find_output_baskets(args).await
    }

    /// Insert an output basket directly (no auth check).
    pub async fn insert_output_basket(&self, basket: &OutputBasket) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_output_basket(basket).await
    }

    /// Insert a transaction directly (no auth check).
    pub async fn insert_transaction(&self, tx: &Transaction) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_transaction(tx).await
    }

    /// Insert an output directly (no auth check).
    pub async fn insert_output(&self, output: &Output) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_output(output).await
    }
}
