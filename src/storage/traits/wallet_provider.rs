//! WalletStorageProvider trait -- the narrowed storage interface for remote clients.
//!
//! This trait mirrors the TypeScript `WalletStorageProvider` interface hierarchy
//! (WalletStorageReader -> WalletStorageWriter -> WalletStorageSync ->
//! WalletStorageProvider) flattened into a single Rust trait.
//!
//! Design:
//! - Local providers (SqliteStorage) satisfy this via a blanket impl over
//!   `StorageProvider`. No changes to existing provider sources required.
//! - `StorageClient` (Phase 3) implements this trait directly WITHOUT implementing
//!   `StorageProvider`, matching the TS pattern.
//! - Object-safe: all methods use `async_trait`, no generic parameters, no lifetimes
//!   in return types. Suitable for `Arc<dyn WalletStorageProvider>`.
//! - `is_storage_provider()` is the runtime discriminant: `true` for local
//!   (blanket impl default), `false` for StorageClient (overridden).

use std::sync::Arc;

use async_trait::async_trait;
use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, RelinquishCertificateArgs,
    RelinquishOutputArgs,
};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::storage::action_traits::StorageActionProvider;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::status::SyncStatus;
use crate::storage::find_args::{
    CertificatePartial, FindCertificatesArgs, FindOutputBasketsArgs, FindOutputsArgs,
    FindProvenTxReqsArgs, FindSyncStatesArgs, FindTransactionsArgs, OutputPartial,
    SyncStatePartial, TransactionPartial, UserPartial,
};
use crate::storage::sync::get_sync_chunk::{GetSyncChunkArgs, SyncChunkOffsets};
use crate::storage::sync::request_args::{RequestSyncChunkArgs, SyncChunkOffset};
use crate::storage::sync::sync_map::SyncMap;
use crate::storage::sync::{ProcessSyncChunkResult, SyncChunk};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::{verify_one, verify_one_or_none, TrxToken};
use crate::tables::{Certificate, Output, OutputBasket, ProvenTxReq, Settings, SyncState, User};
use crate::wallet::types::AuthId;

/// Narrowed storage interface for the wallet's remote and local storage clients.
///
/// This is the contract that:
/// - Local providers (SqliteStorage) satisfy via the blanket impl below.
/// - `StorageClient` (Phase 3) implements directly for remote JSON-RPC storage.
/// - `WalletStorageManager` (Phase 4) accepts as `Arc<dyn WalletStorageProvider>`.
///
/// Method groupings follow the TypeScript hierarchy:
/// - WalletStorageReader: is_available, get_settings, find_*, list_*
/// - WalletStorageWriter: make_available, migrate, destroy, find_or_insert_user,
///   abort_action, create_action, process_action, internalize_action,
///   insert_certificate_auth, relinquish_certificate, relinquish_output
/// - WalletStorageSync: find_or_insert_sync_state_auth, set_active,
///   get_sync_chunk, process_sync_chunk
/// - WalletStorageProvider: is_storage_provider, get_services, set_services
#[async_trait]
pub trait WalletStorageProvider: Send + Sync {
    // -----------------------------------------------------------------------
    // WalletStorageProvider level
    // -----------------------------------------------------------------------

    /// Returns true if this is a local storage provider, false for remote (StorageClient).
    ///
    /// Default implementation returns true for local providers via the blanket impl.
    /// StorageClient overrides to return false.
    fn is_storage_provider(&self) -> bool {
        true
    }

    /// Get wallet services. Not available on storage providers.
    ///
    /// Wire method: `getServices`
    /// Both local blanket impl and StorageClient return NotImplemented.
    async fn get_services(&self) -> WalletResult<Arc<dyn WalletServices>> {
        Err(WalletError::NotImplemented(
            "services not available on storage providers".into(),
        ))
    }

    /// Set wallet services. No-op on storage providers.
    ///
    /// Wire method: `setServices`
    async fn set_services(&self, _services: Arc<dyn WalletServices>) -> WalletResult<()> {
        Ok(())
    }

    // -----------------------------------------------------------------------
    // WalletStorageReader level -- sync methods
    // -----------------------------------------------------------------------

    /// Check whether the storage is currently available.
    ///
    /// Wire method: `isAvailable`
    fn is_available(&self) -> bool;

    /// Get the current settings record.
    ///
    /// Wire method: `getSettings`
    async fn get_settings(&self) -> WalletResult<Settings>;

    // -----------------------------------------------------------------------
    // WalletStorageReader level -- async find methods
    // -----------------------------------------------------------------------

    /// Find certificates scoped to the given auth identity.
    ///
    /// Wire method: `findCertificatesAuth`
    async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>>;

    /// Find output baskets scoped to the given auth identity.
    ///
    /// Wire method: `findOutputBaskets` (no Auth suffix on wire)
    async fn find_output_baskets_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>>;

    /// Find outputs scoped to the given auth identity.
    ///
    /// Wire method: `findOutputsAuth`
    async fn find_outputs_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>>;

    /// Find proven transaction requests.
    ///
    /// Wire method: `findProvenTxReqs`
    async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
    ) -> WalletResult<Vec<ProvenTxReq>>;

    /// List wallet actions (transactions) with filtering and pagination.
    ///
    /// Wire method: `listActions`
    async fn list_actions(
        &self,
        auth: &AuthId,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult>;

    /// List certificates with certifier/type filtering.
    ///
    /// Wire method: `listCertificates`
    async fn list_certificates(
        &self,
        auth: &AuthId,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult>;

    /// List outputs with basket/tag filtering.
    ///
    /// Wire method: `listOutputs`
    async fn list_outputs(
        &self,
        auth: &AuthId,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult>;

    // -----------------------------------------------------------------------
    // WalletStorageWriter level
    // -----------------------------------------------------------------------

    /// Ensure the storage is ready for use. Reads settings and caches them.
    ///
    /// Wire method: `makeAvailable`
    async fn make_available(&self) -> WalletResult<Settings>;

    /// Run database migrations to bring the schema up to date.
    ///
    /// Wire method: `migrate`
    async fn migrate(
        &self,
        storage_name: &str,
        storage_identity_key: &str,
    ) -> WalletResult<String>;

    /// Destroy the storage, cleaning up all resources.
    ///
    /// Wire method: `destroy`
    async fn destroy(&self) -> WalletResult<()>;

    /// Find or insert a user record by identity key.
    ///
    /// Wire method: `findOrInsertUser`
    async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)>;

    /// Abort (cancel) a transaction by reference, releasing locked UTXOs.
    ///
    /// Wire method: `abortAction`
    async fn abort_action(
        &self,
        auth: &AuthId,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult>;

    /// Create a new transaction in storage.
    ///
    /// Wire method: `createAction`
    async fn create_action(
        &self,
        auth: &AuthId,
        args: &StorageCreateActionArgs,
    ) -> WalletResult<StorageCreateActionResult>;

    /// Process (commit) a signed transaction to storage.
    ///
    /// Wire method: `processAction`
    async fn process_action(
        &self,
        auth: &AuthId,
        args: &StorageProcessActionArgs,
    ) -> WalletResult<StorageProcessActionResult>;

    /// Internalize outputs from an external transaction.
    ///
    /// Wire method: `internalizeAction`
    async fn internalize_action(
        &self,
        auth: &AuthId,
        args: &StorageInternalizeActionArgs,
        services: &dyn WalletServices,
    ) -> WalletResult<StorageInternalizeActionResult>;

    /// Insert a certificate scoped to the given auth identity.
    ///
    /// Wire method: `insertCertificateAuth`
    async fn insert_certificate_auth(
        &self,
        auth: &AuthId,
        certificate: &Certificate,
    ) -> WalletResult<i64>;

    /// Relinquish (soft-delete) a certificate by marking it deleted.
    ///
    /// Wire method: `relinquishCertificate`
    async fn relinquish_certificate(
        &self,
        auth: &AuthId,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<i64>;

    /// Relinquish (remove from basket) an output by outpoint.
    ///
    /// Wire method: `relinquishOutput`
    async fn relinquish_output(
        &self,
        auth: &AuthId,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<i64>;

    // -----------------------------------------------------------------------
    // WalletStorageSync level
    // -----------------------------------------------------------------------

    /// Find or insert a sync state record for the given auth and storage pair.
    ///
    /// Wire method: `findOrInsertSyncStateAuth`
    async fn find_or_insert_sync_state_auth(
        &self,
        auth: &AuthId,
        storage_identity_key: &str,
        storage_name: &str,
    ) -> WalletResult<(SyncState, bool)>;

    /// Update the user's active storage identity key.
    ///
    /// Wire method: `setActive`
    async fn set_active(
        &self,
        auth: &AuthId,
        new_active_storage_identity_key: &str,
    ) -> WalletResult<i64>;

    /// Get the next incremental sync chunk of entities changed since the last sync.
    ///
    /// Wire method: `getSyncChunk`
    async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk>;

    /// Process an incoming sync chunk, applying entities with ID remapping.
    ///
    /// Wire method: `processSyncChunk`
    async fn process_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
        chunk: &SyncChunk,
    ) -> WalletResult<ProcessSyncChunkResult>;
}

// ---------------------------------------------------------------------------
// Blanket impl: every StorageProvider satisfies WalletStorageProvider
// ---------------------------------------------------------------------------

/// Blanket implementation: every local `StorageProvider` automatically satisfies
/// `WalletStorageProvider`.
///
/// This impl delegates all methods to the existing `StorageProvider`,
/// `StorageReader`, `StorageReaderWriter`, and `StorageActionProvider` methods.
/// `StorageClient` (Phase 3) will NOT use this blanket impl — it implements
/// `WalletStorageProvider` directly.
#[async_trait]
impl<T: StorageProvider> WalletStorageProvider for T {
    fn is_storage_provider(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        StorageProvider::is_available(self)
    }

    async fn get_settings(&self) -> WalletResult<Settings> {
        StorageProvider::get_settings(self, None).await
    }

    async fn make_available(&self) -> WalletResult<Settings> {
        StorageProvider::make_available(self).await
    }

    async fn migrate(
        &self,
        _storage_name: &str,
        _storage_identity_key: &str,
    ) -> WalletResult<String> {
        StorageProvider::migrate_database(self).await
    }

    async fn destroy(&self) -> WalletResult<()> {
        StorageProvider::destroy(self).await
    }

    async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)> {
        StorageReaderWriter::find_or_insert_user(self, identity_key, None).await
    }

    async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        // Resolve user_id to scope the query to this identity
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        let mut scoped_args = FindCertificatesArgs {
            partial: CertificatePartial {
                user_id: Some(user.user_id),
                ..Default::default()
            },
            since: args.since,
            paged: args.paged.clone(),
        };
        // Merge caller-supplied partial (caller fields take precedence except user_id)
        if scoped_args.partial.cert_type.is_none() {
            scoped_args.partial.cert_type = args.partial.cert_type.clone();
        }
        if scoped_args.partial.serial_number.is_none() {
            scoped_args.partial.serial_number = args.partial.serial_number.clone();
        }
        if scoped_args.partial.certifier.is_none() {
            scoped_args.partial.certifier = args.partial.certifier.clone();
        }
        if scoped_args.partial.subject.is_none() {
            scoped_args.partial.subject = args.partial.subject.clone();
        }
        if scoped_args.partial.is_deleted.is_none() {
            scoped_args.partial.is_deleted = args.partial.is_deleted;
        }
        StorageReader::find_certificates(self, &scoped_args, None).await
    }

    async fn find_output_baskets_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        let _ = auth; // auth context available if needed for future scoping
        StorageReader::find_output_baskets(self, args, None).await
    }

    async fn find_outputs_auth(
        &self,
        auth: &AuthId,
        args: &FindOutputsArgs,
    ) -> WalletResult<Vec<Output>> {
        let _ = auth; // auth context available if needed for future scoping
        StorageReader::find_outputs(self, args, None).await
    }

    async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        StorageReader::find_proven_tx_reqs(self, args, None).await
    }

    async fn list_actions(
        &self,
        auth: &AuthId,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        crate::storage::methods::list_actions::list_actions(
            self as &dyn StorageReader,
            &auth.identity_key,
            user.user_id,
            args,
            None,
        )
        .await
    }

    async fn list_certificates(
        &self,
        auth: &AuthId,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult> {
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        crate::storage::methods::list_certificates::list_certificates(
            self as &dyn StorageReader,
            &auth.identity_key,
            user.user_id,
            args,
            None,
        )
        .await
    }

    async fn list_outputs(
        &self,
        auth: &AuthId,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult> {
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        crate::storage::methods::list_outputs::list_outputs(
            self as &dyn StorageReader,
            &auth.identity_key,
            user.user_id,
            args,
            None,
        )
        .await
    }

    async fn abort_action(
        &self,
        auth: &AuthId,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        crate::storage::methods::abort_action::abort_action(self, &auth.identity_key, args, None)
            .await
    }

    async fn create_action(
        &self,
        auth: &AuthId,
        args: &StorageCreateActionArgs,
    ) -> WalletResult<StorageCreateActionResult> {
        StorageActionProvider::create_action(self, &auth.identity_key, args, None).await
    }

    async fn process_action(
        &self,
        auth: &AuthId,
        args: &StorageProcessActionArgs,
    ) -> WalletResult<StorageProcessActionResult> {
        StorageActionProvider::process_action(self, &auth.identity_key, args, None).await
    }

    async fn internalize_action(
        &self,
        auth: &AuthId,
        args: &StorageInternalizeActionArgs,
        services: &dyn WalletServices,
    ) -> WalletResult<StorageInternalizeActionResult> {
        StorageActionProvider::internalize_action(self, &auth.identity_key, args, services, None)
            .await
    }

    async fn insert_certificate_auth(
        &self,
        auth: &AuthId,
        certificate: &Certificate,
    ) -> WalletResult<i64> {
        let _ = auth; // auth context available if needed for future scoping
        StorageReaderWriter::insert_certificate(self, certificate, None).await
    }

    async fn relinquish_certificate(
        &self,
        _auth: &AuthId,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<i64> {
        let certifier_hex = args.certifier.to_der_hex();
        let cert_type_str = String::from_utf8_lossy(args.cert_type.bytes()).to_string();
        let cert_type_str = cert_type_str.trim_end_matches('\0').to_string();
        let serial_number_str = String::from_utf8_lossy(&args.serial_number.0)
            .trim_end_matches('\0')
            .to_string();

        let cert = verify_one(
            StorageReader::find_certificates(
                self,
                &FindCertificatesArgs {
                    partial: CertificatePartial {
                        certifier: Some(certifier_hex),
                        serial_number: Some(serial_number_str),
                        cert_type: Some(cert_type_str),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await?,
        )?;

        StorageReaderWriter::update_certificate(
            self,
            cert.certificate_id,
            &CertificatePartial {
                is_deleted: Some(true),
                ..Default::default()
            },
            None,
        )
        .await?;

        Ok(1)
    }

    async fn relinquish_output(
        &self,
        _auth: &AuthId,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<i64> {
        // Parse outpoint: "txid.vout"
        let outpoint_str = args.output.to_string();
        let parts: Vec<&str> = outpoint_str.rsplitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(WalletError::InvalidParameter {
                parameter: "output".to_string(),
                must_be: "a valid outpoint string (txid.vout)".to_string(),
            });
        }
        let vout: i32 = parts[0]
            .parse()
            .map_err(|_| WalletError::InvalidParameter {
                parameter: "output".to_string(),
                must_be: "a valid outpoint string with numeric vout".to_string(),
            })?;
        let txid = parts[1].to_string();

        let output = verify_one(
            StorageReader::find_outputs(
                self,
                &FindOutputsArgs {
                    partial: OutputPartial {
                        txid: Some(txid),
                        vout: Some(vout),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await?,
        )?;

        StorageReaderWriter::update_output(
            self,
            output.output_id,
            &OutputPartial {
                basket_id: Some(0), // 0 = no basket
                ..Default::default()
            },
            None,
        )
        .await?;

        Ok(1)
    }

    async fn find_or_insert_sync_state_auth(
        &self,
        auth: &AuthId,
        storage_identity_key: &str,
        storage_name: &str,
    ) -> WalletResult<(SyncState, bool)> {
        // Resolve user_id first
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;

        // Look up existing sync state
        let existing = verify_one_or_none(
            StorageReader::find_sync_states(
                self,
                &FindSyncStatesArgs {
                    partial: SyncStatePartial {
                        user_id: Some(user.user_id),
                        storage_identity_key: Some(storage_identity_key.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await?,
        )?;

        if let Some(state) = existing {
            return Ok((state, false));
        }

        // Insert a new sync state
        let now = chrono::Utc::now().naive_utc();
        let new_state = SyncState {
            created_at: now,
            updated_at: now,
            sync_state_id: 0,
            user_id: user.user_id,
            storage_identity_key: storage_identity_key.to_string(),
            storage_name: storage_name.to_string(),
            status: SyncStatus::Unknown,
            init: false,
            ref_num: String::new(),
            sync_map: "{}".to_string(),
            when: None,
            satoshis: None,
            error_local: None,
            error_other: None,
        };
        let id = StorageReaderWriter::insert_sync_state(self, &new_state, None).await?;
        let mut inserted = new_state;
        inserted.sync_state_id = id;
        Ok((inserted, true))
    }

    async fn set_active(
        &self,
        auth: &AuthId,
        new_active_storage_identity_key: &str,
    ) -> WalletResult<i64> {
        let (user, _) = StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        StorageReaderWriter::update_user(
            self,
            user.user_id,
            &UserPartial {
                active_storage: Some(new_active_storage_identity_key.to_string()),
                ..Default::default()
            },
            None,
        )
        .await
    }

    async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk> {
        let (sync_map, offsets) = build_sync_map_and_offsets(args);
        let internal_args = GetSyncChunkArgs {
            from_storage_identity_key: args.from_storage_identity_key.clone(),
            to_storage_identity_key: args.to_storage_identity_key.clone(),
            user_identity_key: args.identity_key.clone(),
            sync_map: &sync_map,
            max_items_per_entity: args.max_items,
            offsets,
        };
        crate::storage::sync::get_sync_chunk::get_sync_chunk(self, internal_args, None).await
    }

    async fn process_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
        chunk: &SyncChunk,
    ) -> WalletResult<ProcessSyncChunkResult> {
        let (mut sync_map, _) = build_sync_map_and_offsets(args);
        crate::storage::sync::process_sync_chunk::process_sync_chunk(
            self,
            chunk.clone(),
            &mut sync_map,
            None,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Helper: build SyncMap and SyncChunkOffsets from RequestSyncChunkArgs
// ---------------------------------------------------------------------------

/// Convert `RequestSyncChunkArgs` wire format into the internal `SyncMap` and
/// `SyncChunkOffsets` types used by the low-level sync functions.
fn build_sync_map_and_offsets(args: &RequestSyncChunkArgs) -> (SyncMap, SyncChunkOffsets) {
    let mut sync_map = SyncMap::new();

    // Apply the `since` timestamp to all 12 entity maps uniformly
    if let Some(since) = args.since {
        sync_map.proven_tx.max_updated_at = Some(since);
        sync_map.output_basket.max_updated_at = Some(since);
        sync_map.transaction.max_updated_at = Some(since);
        sync_map.output.max_updated_at = Some(since);
        sync_map.tx_label.max_updated_at = Some(since);
        sync_map.tx_label_map.max_updated_at = Some(since);
        sync_map.output_tag.max_updated_at = Some(since);
        sync_map.output_tag_map.max_updated_at = Some(since);
        sync_map.certificate.max_updated_at = Some(since);
        sync_map.certificate_field.max_updated_at = Some(since);
        sync_map.commission.max_updated_at = Some(since);
        sync_map.proven_tx_req.max_updated_at = Some(since);
    }

    // Convert Vec<SyncChunkOffset> to SyncChunkOffsets struct
    let mut offsets = SyncChunkOffsets::default();
    for SyncChunkOffset { name, offset } in &args.offsets {
        match name.as_str() {
            "provenTx" => offsets.proven_tx = *offset,
            "outputBasket" => offsets.output_basket = *offset,
            "outputTag" => offsets.output_tag = *offset,
            "txLabel" => offsets.tx_label = *offset,
            "transaction" => offsets.transaction = *offset,
            "output" => offsets.output = *offset,
            "txLabelMap" => offsets.tx_label_map = *offset,
            "outputTagMap" => offsets.output_tag_map = *offset,
            "certificate" => offsets.certificate = *offset,
            "certificateField" => offsets.certificate_field = *offset,
            "commission" => offsets.commission = *offset,
            "provenTxReq" => offsets.proven_tx_req = *offset,
            _ => {} // unknown entity names are ignored
        }
    }

    (sync_map, offsets)
}
