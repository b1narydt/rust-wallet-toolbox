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
use base64::Engine as _;
use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, RelinquishCertificateArgs,
    RelinquishOutputArgs,
};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::status::SyncStatus;
use crate::status::TransactionStatus;
use crate::storage::action_traits::StorageActionProvider;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::storage::find_args::{
    CertificatePartial, FindCertificateFieldsArgs, FindCertificatesArgs, FindMonitorEventsArgs,
    FindOutputBasketsArgs, FindOutputsArgs, FindProvenTxReqsArgs, FindProvenTxsArgs,
    FindSettingsArgs, FindSyncStatesArgs, FindTransactionsArgs, OutputPartial, ProvenTxPartial,
    ProvenTxReqPartial, PurgeParams, SettingsPartial, SyncStatePartial, TransactionPartial,
    UserPartial,
};
use crate::storage::sync::get_sync_chunk::{GetSyncChunkArgs, SyncChunkOffsets};
use crate::storage::sync::request_args::{RequestSyncChunkArgs, SyncChunkOffset};
use crate::storage::sync::sync_map::SyncMap;
use crate::storage::sync::{ProcessSyncChunkResult, SyncChunk};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::TrxToken;
use crate::storage::{verify_one, verify_one_or_none};
use crate::tables::{
    Certificate, CertificateField, MonitorEvent, Output, OutputBasket, ProvenTx, ProvenTxReq,
    Settings, SyncState, Transaction, User,
};
use crate::wallet::types::{AdminStatsResult, AuthId};

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

    /// Returns the remote endpoint URL for this storage provider, if any.
    ///
    /// Returns None for local providers (SQLite). StorageClient overrides to
    /// return Some(url) so get_stores() can populate WalletStorageInfo.endpoint_url.
    fn get_endpoint_url(&self) -> Option<String> {
        None
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
    async fn migrate(&self, storage_name: &str, storage_identity_key: &str)
        -> WalletResult<String>;

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

    // -----------------------------------------------------------------------
    // Low-level CRUD delegation methods
    // These are not part of the remote wire protocol but are needed by wallet
    // internals (signer, certificates, beef helper, monitor).
    // Local providers implement via the blanket impl.
    // StorageClient returns NotImplemented for these.
    // -----------------------------------------------------------------------

    /// Find a user by identity key.
    async fn find_user_by_identity_key(&self, key: &str) -> WalletResult<Option<User>> {
        let _ = key;
        Err(WalletError::NotImplemented(
            "find_user_by_identity_key".into(),
        ))
    }

    /// Find certificates matching the given filter (no transaction context).
    async fn find_certificates_storage(
        &self,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        let _ = args;
        Err(WalletError::NotImplemented(
            "find_certificates_storage".into(),
        ))
    }

    /// Find certificate fields matching the given filter.
    async fn find_certificate_fields(
        &self,
        args: &FindCertificateFieldsArgs,
    ) -> WalletResult<Vec<CertificateField>> {
        let _ = args;
        Err(WalletError::NotImplemented(
            "find_certificate_fields".into(),
        ))
    }

    /// Find outputs matching the given filter (no transaction context).
    async fn find_outputs_storage(&self, args: &FindOutputsArgs) -> WalletResult<Vec<Output>> {
        let _ = args;
        Err(WalletError::NotImplemented("find_outputs_storage".into()))
    }

    /// Find outputs (alias for find_outputs_storage, used by signer pipeline).
    async fn find_outputs(&self, args: &FindOutputsArgs) -> WalletResult<Vec<Output>> {
        self.find_outputs_storage(args).await
    }

    /// Update an output by ID. Returns the number of rows affected.
    async fn update_output(&self, id: i64, update: &OutputPartial) -> WalletResult<i64> {
        let _ = (id, update);
        Err(WalletError::NotImplemented("update_output".into()))
    }

    /// Insert a certificate and return the new certificate_id.
    async fn insert_certificate_storage(&self, cert: &Certificate) -> WalletResult<i64> {
        let _ = cert;
        Err(WalletError::NotImplemented(
            "insert_certificate_storage".into(),
        ))
    }

    /// Insert a certificate field.
    async fn insert_certificate_field_storage(&self, field: &CertificateField) -> WalletResult<()> {
        let _ = field;
        Err(WalletError::NotImplemented(
            "insert_certificate_field_storage".into(),
        ))
    }

    /// Find settings records.
    async fn find_settings_storage(&self, args: &FindSettingsArgs) -> WalletResult<Vec<Settings>> {
        let _ = args;
        Err(WalletError::NotImplemented("find_settings_storage".into()))
    }

    /// Update settings record.
    async fn update_settings_storage(&self, update: &SettingsPartial) -> WalletResult<i64> {
        let _ = update;
        Err(WalletError::NotImplemented(
            "update_settings_storage".into(),
        ))
    }

    // -----------------------------------------------------------------------
    // Internal delegation methods (monitor/signer use these via manager)
    // Not part of the remote wire protocol — local providers implement them,
    // StorageClient returns NotImplemented.
    // -----------------------------------------------------------------------

    /// Find proven transactions matching the given filter.
    async fn find_proven_txs(&self, _args: &FindProvenTxsArgs) -> WalletResult<Vec<ProvenTx>> {
        Err(WalletError::NotImplemented("find_proven_txs".into()))
    }

    /// Find transactions matching the given filter.
    async fn find_transactions(
        &self,
        _args: &FindTransactionsArgs,
    ) -> WalletResult<Vec<Transaction>> {
        Err(WalletError::NotImplemented("find_transactions".into()))
    }

    /// Update a proven transaction request by ID.
    async fn update_proven_tx_req(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
    ) -> WalletResult<i64> {
        let _ = (id, update);
        Err(WalletError::NotImplemented("update_proven_tx_req".into()))
    }

    /// Update a proven transaction by ID.
    async fn update_proven_tx(&self, id: i64, update: &ProvenTxPartial) -> WalletResult<i64> {
        let _ = (id, update);
        Err(WalletError::NotImplemented("update_proven_tx".into()))
    }

    /// Update a transaction by ID.
    async fn update_transaction(&self, id: i64, update: &TransactionPartial) -> WalletResult<i64> {
        let _ = (id, update);
        Err(WalletError::NotImplemented("update_transaction".into()))
    }

    /// Update a transaction's status by txid.
    async fn update_transaction_status(
        &self,
        txid: &str,
        new_status: TransactionStatus,
    ) -> WalletResult<()> {
        let _ = (txid, new_status);
        Err(WalletError::NotImplemented(
            "update_transaction_status".into(),
        ))
    }

    /// Composite: insert ProvenTx and update ProvenTxReq.
    async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        req_id: i64,
        proven_tx: &ProvenTx,
    ) -> WalletResult<i64> {
        let _ = (req_id, proven_tx);
        Err(WalletError::NotImplemented(
            "update_proven_tx_req_with_new_proven_tx".into(),
        ))
    }

    /// Find output baskets matching the given filter (no auth, no trx).
    async fn find_output_baskets(
        &self,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        let _ = args;
        Err(WalletError::NotImplemented("find_output_baskets".into()))
    }

    /// Insert an output basket directly (no auth check).
    async fn insert_output_basket(&self, basket: &OutputBasket) -> WalletResult<i64> {
        let _ = basket;
        Err(WalletError::NotImplemented("insert_output_basket".into()))
    }

    /// Insert a transaction directly (no auth check).
    async fn insert_transaction(&self, tx: &Transaction) -> WalletResult<i64> {
        let _ = tx;
        Err(WalletError::NotImplemented("insert_transaction".into()))
    }

    /// Insert an output directly (no auth check).
    async fn insert_output(&self, output: &Output) -> WalletResult<i64> {
        let _ = output;
        Err(WalletError::NotImplemented("insert_output".into()))
    }

    /// Insert a monitor event record.
    async fn insert_monitor_event(&self, event: &MonitorEvent) -> WalletResult<i64> {
        let _ = event;
        Err(WalletError::NotImplemented("insert_monitor_event".into()))
    }

    /// Delete monitor events matching `event` name with `id < before_id`.
    async fn delete_monitor_events_before_id(
        &self,
        event_name: &str,
        before_id: i64,
    ) -> WalletResult<u64> {
        let _ = (event_name, before_id);
        Err(WalletError::NotImplemented(
            "delete_monitor_events_before_id".into(),
        ))
    }

    /// Find monitor events matching the given filter.
    async fn find_monitor_events(
        &self,
        args: &FindMonitorEventsArgs,
    ) -> WalletResult<Vec<MonitorEvent>> {
        let _ = args;
        Err(WalletError::NotImplemented("find_monitor_events".into()))
    }

    /// Count monitor events matching the given filter.
    async fn count_monitor_events(&self, args: &FindMonitorEventsArgs) -> WalletResult<i64> {
        let _ = args;
        Err(WalletError::NotImplemented("count_monitor_events".into()))
    }

    /// Returns aggregate deployment statistics.
    async fn admin_stats(&self, auth_id: &str) -> WalletResult<AdminStatsResult> {
        let _ = auth_id;
        Err(WalletError::NotImplemented("admin_stats".into()))
    }

    /// Delete old records based on purge params.
    async fn purge_data(&self, params: &PurgeParams) -> WalletResult<String> {
        let _ = params;
        Err(WalletError::NotImplemented("purge_data".into()))
    }

    /// Review and correct stale transaction statuses.
    async fn review_status(&self, aged_limit: chrono::NaiveDateTime) -> WalletResult<String> {
        let _ = aged_limit;
        Err(WalletError::NotImplemented("review_status".into()))
    }

    /// Returns the storage identity key.
    async fn get_storage_identity_key(&self) -> WalletResult<String> {
        let settings = self.get_settings().await?;
        Ok(settings.storage_identity_key)
    }

    // -----------------------------------------------------------------------
    // Transaction management + trx-aware CRUD
    //
    // These permit callers (e.g. handle_permanent_broadcast_failure) to group
    // a sequence of writes under one atomic DB transaction. Remote storage
    // (StorageClient) returns NotImplemented; local providers delegate to
    // StorageReaderWriter via the blanket impl.
    // -----------------------------------------------------------------------

    /// Begin a new database transaction. Returns an opaque TrxToken.
    async fn begin_transaction(&self) -> WalletResult<TrxToken> {
        Err(WalletError::NotImplemented("begin_transaction".into()))
    }

    /// Commit a previously begun transaction.
    async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        let _ = trx;
        Err(WalletError::NotImplemented("commit_transaction".into()))
    }

    /// Rollback a previously begun transaction.
    async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        let _ = trx;
        Err(WalletError::NotImplemented("rollback_transaction".into()))
    }

    /// Find outputs scoped to an optional open transaction.
    async fn find_outputs_trx(
        &self,
        args: &FindOutputsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Output>> {
        let _ = (args, trx);
        Err(WalletError::NotImplemented("find_outputs_trx".into()))
    }

    /// Find transactions scoped to an optional open transaction.
    async fn find_transactions_trx(
        &self,
        args: &FindTransactionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Transaction>> {
        let _ = (args, trx);
        Err(WalletError::NotImplemented("find_transactions_trx".into()))
    }

    /// Find proven-tx-reqs scoped to an optional open transaction.
    async fn find_proven_tx_reqs_trx(
        &self,
        args: &FindProvenTxReqsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        let _ = (args, trx);
        Err(WalletError::NotImplemented("find_proven_tx_reqs_trx".into()))
    }

    /// Update an output inside an optional open transaction.
    async fn update_output_trx(
        &self,
        id: i64,
        update: &OutputPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        let _ = (id, update, trx);
        Err(WalletError::NotImplemented("update_output_trx".into()))
    }

    /// Update a proven-tx-req inside an optional open transaction.
    async fn update_proven_tx_req_trx(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        let _ = (id, update, trx);
        Err(WalletError::NotImplemented("update_proven_tx_req_trx".into()))
    }

    /// Update a transaction's status by txid inside an optional open transaction.
    async fn update_transaction_status_trx(
        &self,
        txid: &str,
        new_status: TransactionStatus,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        let _ = (txid, new_status, trx);
        Err(WalletError::NotImplemented(
            "update_transaction_status_trx".into(),
        ))
    }

    /// Restore all outputs consumed by `tx_id` back to spendable (clears
    /// spent_by and sets spendable=true). Callers that previously relied on
    /// the implicit restore-on-Failed side-effect of update_transaction_status
    /// must now call this explicitly.
    async fn restore_consumed_inputs_trx(
        &self,
        tx_id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<u64> {
        let _ = (tx_id, trx);
        Err(WalletError::NotImplemented(
            "restore_consumed_inputs_trx".into(),
        ))
    }
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
        // Use list_outputs_rw so that specOp basket names (e.g. wallet balance) are handled.
        crate::storage::methods::list_outputs::list_outputs_rw(
            self as &dyn StorageReaderWriter,
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;

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

        // Insert a new sync state.
        // Generate a unique ref_num (matches TS `randomBytesBase64(12)`).
        let ref_num = {
            use rand::RngCore;
            let mut bytes = [0u8; 12];
            rand::thread_rng().fill_bytes(&mut bytes);
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
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
            ref_num,
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
        let (user, _) =
            StorageReaderWriter::find_or_insert_user(self, &auth.identity_key, None).await?;
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
        // Determine if chunk is empty (nothing to process).
        // An empty chunk signals that the reader has no more data — sync is done.
        let chunk_is_empty = chunk.user.is_none()
            && chunk.proven_txs.as_ref().is_none_or(|v| v.is_empty())
            && chunk.output_baskets.as_ref().is_none_or(|v| v.is_empty())
            && chunk.transactions.as_ref().is_none_or(|v| v.is_empty())
            && chunk.outputs.as_ref().is_none_or(|v| v.is_empty())
            && chunk.tx_labels.as_ref().is_none_or(|v| v.is_empty())
            && chunk.tx_label_maps.as_ref().is_none_or(|v| v.is_empty())
            && chunk.output_tags.as_ref().is_none_or(|v| v.is_empty())
            && chunk.output_tag_maps.as_ref().is_none_or(|v| v.is_empty())
            && chunk.certificates.as_ref().is_none_or(|v| v.is_empty())
            && chunk
                .certificate_fields
                .as_ref()
                .is_none_or(|v| v.is_empty())
            && chunk.commissions.as_ref().is_none_or(|v| v.is_empty())
            && chunk.proven_tx_reqs.as_ref().is_none_or(|v| v.is_empty());

        let (mut sync_map, _) = build_sync_map_and_offsets(args);
        let mut result = crate::storage::sync::process_sync_chunk::process_sync_chunk(
            self,
            chunk.clone(),
            &mut sync_map,
            None,
        )
        .await?;

        // Determine `done`:
        // - If the chunk was empty, there is nothing left to sync.
        // - TS parity: processSyncChunk sets done=true when chunk was empty.
        result.done = chunk_is_empty;

        // Persist updated SyncState so the next iteration knows where to resume.
        // Find the existing SyncState for this storage pair and update its sync_map + when.
        if !chunk_is_empty {
            // Only update if we actually processed something.
            let (user, _) =
                StorageReaderWriter::find_or_insert_user(self, &chunk.user_identity_key, None)
                    .await?;
            let existing = verify_one_or_none(
                StorageReader::find_sync_states(
                    self,
                    &FindSyncStatesArgs {
                        partial: SyncStatePartial {
                            user_id: Some(user.user_id),
                            storage_identity_key: Some(args.from_storage_identity_key.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    None,
                )
                .await?,
            )?;

            if let Some(state) = existing {
                // Serialize updated sync_map to JSON for storage.
                let new_sync_map_json = serde_json::to_string(&sync_map).map_err(|e| {
                    WalletError::Internal(format!("failed to serialize sync_map: {e}"))
                })?;

                // Advance the `when` high-watermark.
                // Use the maximum updated_at seen in this chunk if available,
                // otherwise use the current time. This ensures the next iteration
                // passes a non-None `since` to get_sync_chunk, preventing re-fetching
                // the same data on subsequent calls.
                let new_when = result
                    .max_updated_at
                    .or_else(|| Some(chrono::Utc::now().naive_utc()));

                StorageReaderWriter::update_sync_state(
                    self,
                    state.sync_state_id,
                    &SyncStatePartial {
                        sync_map: Some(new_sync_map_json),
                        when: new_when,
                        ..Default::default()
                    },
                    None,
                )
                .await?;
            }
        }

        Ok(result)
    }

    // Low-level CRUD delegation -- blanket impl

    async fn find_user_by_identity_key(&self, key: &str) -> WalletResult<Option<User>> {
        StorageReader::find_user_by_identity_key(self, key, None).await
    }

    async fn find_certificates_storage(
        &self,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>> {
        StorageReader::find_certificates(self, args, None).await
    }

    async fn find_certificate_fields(
        &self,
        args: &FindCertificateFieldsArgs,
    ) -> WalletResult<Vec<CertificateField>> {
        StorageReader::find_certificate_fields(self, args, None).await
    }

    async fn find_outputs_storage(&self, args: &FindOutputsArgs) -> WalletResult<Vec<Output>> {
        StorageReader::find_outputs(self, args, None).await
    }

    async fn update_output(&self, id: i64, update: &OutputPartial) -> WalletResult<i64> {
        StorageReaderWriter::update_output(self, id, update, None).await
    }

    async fn insert_certificate_storage(&self, cert: &Certificate) -> WalletResult<i64> {
        StorageReaderWriter::insert_certificate(self, cert, None).await
    }

    async fn insert_certificate_field_storage(&self, field: &CertificateField) -> WalletResult<()> {
        StorageReaderWriter::insert_certificate_field(self, field, None).await
    }

    async fn find_settings_storage(&self, args: &FindSettingsArgs) -> WalletResult<Vec<Settings>> {
        StorageReader::find_settings(self, args, None).await
    }

    async fn update_settings_storage(&self, update: &SettingsPartial) -> WalletResult<i64> {
        StorageReaderWriter::update_settings(self, update, None).await
    }

    // Internal delegation methods -- blanket impl delegates to StorageReader/Writer

    async fn find_proven_txs(&self, args: &FindProvenTxsArgs) -> WalletResult<Vec<ProvenTx>> {
        StorageReader::find_proven_txs(self, args, None).await
    }

    async fn find_transactions(
        &self,
        args: &FindTransactionsArgs,
    ) -> WalletResult<Vec<Transaction>> {
        StorageReader::find_transactions(self, args, None).await
    }

    async fn update_proven_tx_req(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
    ) -> WalletResult<i64> {
        StorageReaderWriter::update_proven_tx_req(self, id, update, None).await
    }

    async fn update_proven_tx(&self, id: i64, update: &ProvenTxPartial) -> WalletResult<i64> {
        StorageReaderWriter::update_proven_tx(self, id, update, None).await
    }

    async fn update_transaction(&self, id: i64, update: &TransactionPartial) -> WalletResult<i64> {
        StorageReaderWriter::update_transaction(self, id, update, None).await
    }

    async fn update_transaction_status(
        &self,
        txid: &str,
        new_status: TransactionStatus,
    ) -> WalletResult<()> {
        StorageReaderWriter::update_transaction_status(self, txid, new_status, None).await
    }

    async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        req_id: i64,
        proven_tx: &ProvenTx,
    ) -> WalletResult<i64> {
        StorageReaderWriter::update_proven_tx_req_with_new_proven_tx(self, req_id, proven_tx, None)
            .await
    }

    async fn find_output_baskets(
        &self,
        args: &FindOutputBasketsArgs,
    ) -> WalletResult<Vec<OutputBasket>> {
        StorageReader::find_output_baskets(self, args, None).await
    }

    async fn insert_output_basket(&self, basket: &OutputBasket) -> WalletResult<i64> {
        StorageReaderWriter::insert_output_basket(self, basket, None).await
    }

    async fn insert_transaction(&self, tx: &Transaction) -> WalletResult<i64> {
        StorageReaderWriter::insert_transaction(self, tx, None).await
    }

    async fn insert_output(&self, output: &Output) -> WalletResult<i64> {
        StorageReaderWriter::insert_output(self, output, None).await
    }

    async fn insert_monitor_event(&self, event: &MonitorEvent) -> WalletResult<i64> {
        StorageReaderWriter::insert_monitor_event(self, event, None).await
    }

    async fn delete_monitor_events_before_id(
        &self,
        event_name: &str,
        before_id: i64,
    ) -> WalletResult<u64> {
        StorageReaderWriter::delete_monitor_events_before_id(self, event_name, before_id, None)
            .await
    }

    async fn find_monitor_events(
        &self,
        args: &FindMonitorEventsArgs,
    ) -> WalletResult<Vec<MonitorEvent>> {
        StorageReader::find_monitor_events(self, args, None).await
    }

    async fn count_monitor_events(&self, args: &FindMonitorEventsArgs) -> WalletResult<i64> {
        StorageReader::count_monitor_events(self, args, None).await
    }

    async fn admin_stats(&self, auth_id: &str) -> WalletResult<AdminStatsResult> {
        StorageReaderWriter::admin_stats(self, auth_id).await
    }

    async fn purge_data(&self, params: &PurgeParams) -> WalletResult<String> {
        StorageReaderWriter::purge_data(self, params, None).await
    }

    async fn review_status(&self, aged_limit: chrono::NaiveDateTime) -> WalletResult<String> {
        StorageReaderWriter::review_status(self, aged_limit, None).await
    }

    async fn get_storage_identity_key(&self) -> WalletResult<String> {
        StorageProvider::get_storage_identity_key(self)
    }

    // -----------------------------------------------------------------------
    // Transaction management + trx-aware CRUD overrides
    // Local providers delegate to the underlying StorageReaderWriter.
    // -----------------------------------------------------------------------

    async fn begin_transaction(&self) -> WalletResult<TrxToken> {
        StorageReaderWriter::begin_transaction(self).await
    }

    async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        StorageReaderWriter::commit_transaction(self, trx).await
    }

    async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        StorageReaderWriter::rollback_transaction(self, trx).await
    }

    async fn find_outputs_trx(
        &self,
        args: &FindOutputsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Output>> {
        StorageReader::find_outputs(self, args, trx).await
    }

    async fn find_transactions_trx(
        &self,
        args: &FindTransactionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Transaction>> {
        StorageReader::find_transactions(self, args, trx).await
    }

    async fn find_proven_tx_reqs_trx(
        &self,
        args: &FindProvenTxReqsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTxReq>> {
        StorageReader::find_proven_tx_reqs(self, args, trx).await
    }

    async fn update_output_trx(
        &self,
        id: i64,
        update: &OutputPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        StorageReaderWriter::update_output(self, id, update, trx).await
    }

    async fn update_proven_tx_req_trx(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        StorageReaderWriter::update_proven_tx_req(self, id, update, trx).await
    }

    async fn update_transaction_status_trx(
        &self,
        txid: &str,
        new_status: TransactionStatus,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        StorageReaderWriter::update_transaction_status(self, txid, new_status, trx).await
    }

    async fn restore_consumed_inputs_trx(
        &self,
        tx_id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<u64> {
        StorageReaderWriter::restore_consumed_inputs(self, tx_id, trx).await
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
