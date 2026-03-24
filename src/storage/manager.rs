//! WalletStorageManager -- multi-provider storage coordinator with hierarchical locking.
//!
//! Implements the TS-parity manager architecture:
//! - Multiple providers wrapped in `ManagedStorage` with cached settings/user
//! - `make_available()` partitions stores into active/backups/conflicting_actives
//! - Four-level hierarchical lock system (reader < writer < sync < storage_provider)
//! - All writes route to active only; backups sync via `update_backups()` (Plan 02)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, RelinquishCertificateArgs,
    RelinquishOutputArgs,
};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::status::TransactionStatus;
use crate::storage::find_args::{
    FindCertificateFieldsArgs, FindCertificatesArgs, FindOutputBasketsArgs, FindOutputsArgs,
    FindProvenTxReqsArgs, FindProvenTxsArgs, FindTransactionsArgs, OutputPartial, ProvenTxPartial,
    ProvenTxReqPartial, PurgeParams, TransactionPartial,
};
use crate::storage::sync::request_args::RequestSyncChunkArgs;
use crate::storage::sync::{ProcessSyncChunkResult, SyncChunk};
use crate::storage::traits::wallet_provider::WalletStorageProvider;
use crate::tables::{
    Certificate, MonitorEvent, Output, OutputBasket, ProvenTx, ProvenTxReq, Settings, Transaction,
    User,
};
use crate::wallet::types::{AdminStatsResult, AuthId};

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
            .ok_or_else(|| WalletError::InvalidOperation("Active settings not cached".to_string()))?;
        Ok(settings.storage_identity_key)
    }

    /// Returns the active store's `settings.storage_name`.
    pub async fn get_active_store_name(&self) -> WalletResult<String> {
        let state = self.state.lock().await;
        let idx = state.require_active()?;
        let settings = state.stores[idx]
            .get_settings_cached()
            .await
            .ok_or_else(|| WalletError::InvalidOperation("Active settings not cached".to_string()))?;
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
            return Err(WalletError::NotActive(
                "Storage manager is not yet available — call make_available() first".to_string(),
            ));
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
        let mut state = self.state.lock().await;

        if state.stores.is_empty() {
            return Err(WalletError::InvalidParameter {
                parameter: "stores".to_string(),
                must_be: "non-empty — provide at least one storage provider".to_string(),
            });
        }

        // Step 1: Call make_available() + find_or_insert_user() on each store
        for store in &state.stores {
            let settings = store.storage.make_available().await?;
            let (user, _) = store
                .storage
                .find_or_insert_user(&self.identity_key)
                .await?;
            *store.settings.lock().await = Some(settings);
            *store.user.lock().await = Some(user);
            store.is_available.store(true, Ordering::Release);
        }

        // Step 2: Partition stores — first store is default active
        let num_stores = state.stores.len();
        state.active_index = Some(0);
        state.backup_indices.clear();
        state.conflicting_active_indices.clear();

        for i in 1..num_stores {
            let store_sik = {
                let s = state.stores[i].settings.lock().await;
                s.as_ref().map(|s| s.storage_identity_key.clone()).unwrap_or_default()
            };
            let store_user_active = {
                let u = state.stores[i].user.lock().await;
                u.as_ref().map(|u| u.active_storage.clone()).unwrap_or_default()
            };

            // Check if current active is enabled (settings.sik == user.active_storage)
            let current_active_enabled = {
                let active_idx = state.active_index.unwrap();
                let active_sik = {
                    let s = state.stores[active_idx].settings.lock().await;
                    s.as_ref().map(|s| s.storage_identity_key.clone()).unwrap_or_default()
                };
                let active_user_active = {
                    let u = state.stores[active_idx].user.lock().await;
                    u.as_ref().map(|u| u.active_storage.clone()).unwrap_or_default()
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
            s.as_ref().map(|s| s.storage_identity_key.clone()).unwrap_or_default()
        };

        let old_backups = std::mem::take(&mut state.backup_indices);
        for backup_idx in old_backups {
            let backup_user_active = {
                let u = state.stores[backup_idx].user.lock().await;
                u.as_ref().map(|u| u.active_storage.clone()).unwrap_or_default()
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
    ) -> WalletResult<(tokio::sync::MutexGuard<'_, ()>, tokio::sync::MutexGuard<'_, ()>)> {
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
    // -----------------------------------------------------------------------

    pub async fn list_actions(
        &self,
        auth: &AuthId,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        let active = self.get_active().await?;
        active.list_actions(auth, args).await
    }

    pub async fn list_outputs(
        &self,
        auth: &AuthId,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult> {
        let active = self.get_active().await?;
        active.list_outputs(auth, args).await
    }

    pub async fn list_certificates(
        &self,
        auth: &AuthId,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult> {
        let active = self.get_active().await?;
        active.list_certificates(auth, args).await
    }

    pub async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        let active = self.get_active().await?;
        active.find_certificates_auth(auth, args).await
    }

    pub async fn find_output_baskets_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        let active = self.get_active().await?;
        active.find_output_baskets_auth(auth, args).await
    }

    pub async fn find_outputs_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>> {
        let active = self.get_active().await?;
        active.find_outputs_auth(auth, args).await
    }

    pub async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        let active = self.get_active().await?;
        active.find_proven_tx_reqs(args).await
    }

    // -----------------------------------------------------------------------
    // WalletStorageProvider delegation — write operations
    // -----------------------------------------------------------------------

    pub async fn abort_action(
        &self,
        auth: &AuthId,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        let active = self.get_active().await?;
        active.abort_action(auth, args).await
    }

    pub async fn create_action(
        &self,
        auth: &AuthId,
        args: &StorageCreateActionArgs,
    ) -> WalletResult<StorageCreateActionResult> {
        let active = self.get_active().await?;
        active.create_action(auth, args).await
    }

    pub async fn process_action(
        &self,
        auth: &AuthId,
        args: &StorageProcessActionArgs,
    ) -> WalletResult<StorageProcessActionResult> {
        let active = self.get_active().await?;
        active.process_action(auth, args).await
    }

    pub async fn internalize_action(
        &self,
        auth: &AuthId,
        args: &StorageInternalizeActionArgs,
        services: &dyn WalletServices,
    ) -> WalletResult<StorageInternalizeActionResult> {
        let active = self.get_active().await?;
        active.internalize_action(auth, args, services).await
    }

    pub async fn insert_certificate_auth(
        &self,
        auth: &AuthId,
        certificate: &Certificate,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_certificate_auth(auth, certificate).await
    }

    pub async fn relinquish_certificate(
        &self,
        auth: &AuthId,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.relinquish_certificate(auth, args).await
    }

    pub async fn relinquish_output(
        &self,
        auth: &AuthId,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.relinquish_output(auth, args).await
    }

    // -----------------------------------------------------------------------
    // Sync-level delegation
    // -----------------------------------------------------------------------

    pub async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)> {
        let active = self.get_active().await?;
        active.find_or_insert_user(identity_key).await
    }

    pub async fn get_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
    ) -> WalletResult<SyncChunk> {
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

    pub async fn find_user_by_identity_key(
        &self,
        key: &str,
    ) -> WalletResult<Option<User>> {
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

    pub async fn find_outputs_storage(
        &self,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>> {
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

    pub async fn find_proven_txs(
        &self,
        args: &FindProvenTxsArgs,
    ) -> WalletResult<Vec<ProvenTx>> {
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

    pub async fn insert_monitor_event(&self, event: &MonitorEvent) -> WalletResult<i64> {
        let active = self.get_active().await?;
        active.insert_monitor_event(event).await
    }

    pub async fn admin_stats(&self, auth_id: &str) -> WalletResult<AdminStatsResult> {
        let active = self.get_active().await?;
        active.admin_stats(auth_id).await
    }

    pub async fn purge_data(&self, params: &PurgeParams) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.purge_data(params).await
    }

    pub async fn review_status(
        &self,
        aged_limit: chrono::NaiveDateTime,
    ) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.review_status(aged_limit).await
    }

    pub async fn get_storage_identity_key(&self) -> WalletResult<String> {
        let active = self.get_active().await?;
        active.get_storage_identity_key().await
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
