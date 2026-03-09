//! WalletStorageManager -- coordinates an active storage provider with an optional backup.
//!
//! The Wallet struct holds a WalletStorageManager (not a raw StorageProvider).
//! Reads route to the active provider. Writes propagate to both active and backup.
//! A tokio::sync::Mutex serializes write operations to prevent concurrent conflicts.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, ListActionsArgs, ListActionsResult,
    ListCertificatesArgs, ListCertificatesResult, ListOutputsArgs, ListOutputsResult,
    RelinquishCertificateArgs, RelinquishCertificateResult, RelinquishOutputArgs,
    RelinquishOutputResult,
};

use crate::error::{WalletError, WalletResult};
use crate::status::TransactionStatus;
use crate::storage::find_args::*;
use crate::storage::methods::list_actions;
use crate::storage::methods::list_certificates;
use crate::storage::methods::list_outputs;
use crate::storage::traits::provider::StorageProvider;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::{verify_one, TrxToken};
use crate::tables::*;
use crate::types::Chain;
use crate::wallet::types::AdminStatsResult;

/// Coordinates an active storage provider and an optional backup provider.
///
/// - Reads route to the active provider only.
/// - Writes propagate to both active and backup (backup failures are logged, not fatal).
/// - A lock serializes write operations to prevent concurrent conflicts.
/// - Implements `StorageProvider` so Wallet can use it as its storage backend.
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # fn example(
/// #     provider: Arc<dyn wallet_toolbox::storage::traits::provider::StorageProvider>,
/// # ) {
/// use wallet_toolbox::storage::manager::WalletStorageManager;
/// use wallet_toolbox::types::Chain;
///
/// let manager = WalletStorageManager::new(
///     provider,
///     None,              // no backup provider
///     Chain::Test,
///     "identity_key_hex".to_string(),
/// );
/// # }
/// ```
pub struct WalletStorageManager {
    /// The primary storage backend.
    active: Arc<dyn StorageProvider>,
    /// Optional backup provider.
    backup: Option<Arc<dyn StorageProvider>>,
    /// The chain this manager is configured for.
    chain: Chain,
    /// The wallet owner's identity key.
    user_identity_key: String,
    /// Write serialization lock (matches TS lock queue pattern).
    lock: tokio::sync::Mutex<()>,
}

impl WalletStorageManager {
    /// Create a new WalletStorageManager.
    pub fn new(
        active: Arc<dyn StorageProvider>,
        backup: Option<Arc<dyn StorageProvider>>,
        chain: Chain,
        user_identity_key: String,
    ) -> Self {
        Self {
            active,
            backup,
            chain,
            user_identity_key,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Expose the active provider for sync operations.
    pub fn active(&self) -> &Arc<dyn StorageProvider> {
        &self.active
    }

    /// Expose the backup provider (if any) for sync operations.
    pub fn backup(&self) -> Option<&Arc<dyn StorageProvider>> {
        self.backup.as_ref()
    }

    /// Check if a backup provider is configured.
    pub fn has_backup(&self) -> bool {
        self.backup.is_some()
    }

    /// Set the active storage provider.
    pub fn set_active_storage(&mut self, provider: Arc<dyn StorageProvider>) {
        self.active = provider;
    }

    /// Add a backup storage provider.
    pub fn add_wallet_storage_provider(&mut self, provider: Arc<dyn StorageProvider>) {
        self.backup = Some(provider);
    }

    /// Returns the user identity key for use by the Wallet struct.
    pub fn auth_id(&self) -> &str {
        &self.user_identity_key
    }

    /// Returns the user identity key reference.
    pub fn get_user_identity_key(&self) -> &str {
        &self.user_identity_key
    }

    // -----------------------------------------------------------------------
    // Admin methods
    // -----------------------------------------------------------------------

    /// Returns aggregate deployment statistics by delegating to the active provider.
    pub async fn admin_stats(&self, auth_id: &str) -> WalletResult<AdminStatsResult> {
        self.active.admin_stats(auth_id).await
    }

    // -----------------------------------------------------------------------
    // High-level query methods (delegated to storage::methods)
    // -----------------------------------------------------------------------

    /// List wallet transactions with label filtering, pagination, and specOp support.
    ///
    /// Delegates to `storage::methods::list_actions::list_actions_with_spec_ops`.
    pub async fn list_actions(
        &self,
        auth: &str,
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        let user = self
            .active
            .find_user_by_identity_key(auth, None)
            .await?
            .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;
        list_actions::list_actions_with_spec_ops(self.active.as_ref(), auth, user.user_id, args, None).await
    }

    /// List wallet outputs with basket/tag filtering, pagination, and specOp support.
    ///
    /// Delegates to `storage::methods::list_outputs::list_outputs_rw` which handles
    /// specOp dispatch (WalletBalance, InvalidChange, SetChangeParams).
    pub async fn list_outputs(
        &self,
        auth: &str,
        args: &ListOutputsArgs,
    ) -> WalletResult<ListOutputsResult> {
        let user = self
            .active
            .find_user_by_identity_key(auth, None)
            .await?
            .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;
        list_outputs::list_outputs_rw(self.active.as_ref(), auth, user.user_id, args, None).await
    }

    /// List certificates with certifier/type filtering and field joining.
    ///
    /// Delegates to `storage::methods::list_certificates::list_certificates`.
    pub async fn list_certificates(
        &self,
        auth: &str,
        args: &ListCertificatesArgs,
    ) -> WalletResult<ListCertificatesResult> {
        let user = self
            .active
            .find_user_by_identity_key(auth, None)
            .await?
            .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;
        list_certificates::list_certificates(self.active.as_ref(), auth, user.user_id, args, None)
            .await
    }

    // -----------------------------------------------------------------------
    // High-level mutation methods
    // -----------------------------------------------------------------------

    /// Abort (cancel) a transaction by reference, releasing locked UTXOs.
    ///
    /// Matches TS `StorageProvider.abortAction` logic:
    /// - Finds the transaction by reference
    /// - Validates it is outgoing and in an abortable status
    /// - Updates status to 'failed'
    /// - Releases inputs (sets spendable=true, clears spentBy)
    pub async fn abort_action(
        &self,
        auth: &str,
        args: &AbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        let _guard = self.lock.lock().await;

        let user = self
            .active
            .find_user_by_identity_key(auth, None)
            .await?
            .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;

        let reference = String::from_utf8_lossy(&args.reference).to_string();

        // Find the transaction by reference and user
        let txs = self
            .active
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(user.user_id),
                        reference: Some(reference.clone()),
                        ..Default::default()
                    },
                    no_raw_tx: true,
                    ..Default::default()
                },
                None,
            )
            .await?;

        let tx = if txs.is_empty() {
            return Err(WalletError::InvalidParameter {
                parameter: "reference".to_string(),
                must_be: "an existing action reference".to_string(),
            });
        } else {
            &txs[0]
        };

        // Validate abortability: must be outgoing and not in an un-abortable status
        let un_abortable = [
            TransactionStatus::Completed,
            TransactionStatus::Failed,
            TransactionStatus::Sending,
            TransactionStatus::Unproven,
        ];

        if !tx.is_outgoing || un_abortable.contains(&tx.status) {
            return Err(WalletError::InvalidParameter {
                parameter: "reference".to_string(),
                must_be: "an inprocess, outgoing action that has not been signed and shared to the network".to_string(),
            });
        }

        // Release inputs: find outputs spent by this transaction and make them spendable again
        let inputs = self
            .active
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        spent_by: Some(tx.transaction_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await?;

        for input in &inputs {
            self.active
                .update_output(
                    input.output_id,
                    &OutputPartial {
                        spendable: Some(true),
                        spent_by: Some(0), // Clear spent_by (0 = unset)
                        ..Default::default()
                    },
                    None,
                )
                .await?;

            // Propagate to backup
            if let Some(ref backup) = self.backup {
                if let Err(e) = backup
                    .update_output(
                        input.output_id,
                        &OutputPartial {
                            spendable: Some(true),
                            spent_by: Some(0),
                            ..Default::default()
                        },
                        None,
                    )
                    .await
                {
                    warn!(error = %e, "Backup: abort_action update_output failed");
                }
            }
        }

        // Update transaction status to failed
        self.active
            .update_transaction(
                tx.transaction_id,
                &TransactionPartial {
                    status: Some(TransactionStatus::Failed),
                    ..Default::default()
                },
                None,
            )
            .await?;

        if let Some(ref backup) = self.backup {
            if let Err(e) = backup
                .update_transaction(
                    tx.transaction_id,
                    &TransactionPartial {
                        status: Some(TransactionStatus::Failed),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                warn!(error = %e, "Backup: abort_action update_transaction failed");
            }
        }

        Ok(AbortActionResult { aborted: true })
    }

    /// Relinquish (soft-delete) an output by removing it from its basket.
    ///
    /// Matches TS `StorageProvider.relinquishOutput` logic:
    /// - Parse outpoint string to txid + vout
    /// - Find the output
    /// - Update basketId to None (remove from basket)
    pub async fn relinquish_output(
        &self,
        _auth: &str,
        args: &RelinquishOutputArgs,
    ) -> WalletResult<RelinquishOutputResult> {
        let _guard = self.lock.lock().await;

        // Parse outpoint: "txid.vout"
        let parts: Vec<&str> = args.output.rsplitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(WalletError::InvalidParameter {
                parameter: "output".to_string(),
                must_be: "a valid outpoint string (txid.vout)".to_string(),
            });
        }
        let vout: i32 = parts[0].parse().map_err(|_| WalletError::InvalidParameter {
            parameter: "output".to_string(),
            must_be: "a valid outpoint string with numeric vout".to_string(),
        })?;
        let txid = parts[1].to_string();

        let output = verify_one(
            self.active
                .find_outputs(
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

        // Remove from basket by clearing basket_id
        self.active
            .update_output(
                output.output_id,
                &OutputPartial {
                    basket_id: Some(0), // 0 = no basket
                    ..Default::default()
                },
                None,
            )
            .await?;

        if let Some(ref backup) = self.backup {
            if let Err(e) = backup
                .update_output(
                    output.output_id,
                    &OutputPartial {
                        basket_id: Some(0),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                warn!(error = %e, "Backup: relinquish_output failed");
            }
        }

        Ok(RelinquishOutputResult { relinquished: true })
    }

    /// Relinquish (soft-delete) a certificate by marking it as deleted.
    ///
    /// Matches TS `StorageProvider.relinquishCertificate` logic:
    /// - Find certificate by type + serial_number + certifier
    /// - Update isDeleted = true
    pub async fn relinquish_certificate(
        &self,
        _auth: &str,
        args: &RelinquishCertificateArgs,
    ) -> WalletResult<RelinquishCertificateResult> {
        let _guard = self.lock.lock().await;

        let certifier_hex = args.certifier.to_der_hex();

        // Convert CertificateType to string for storage lookup
        let cert_type_str = String::from_utf8_lossy(args.cert_type.bytes()).to_string();
        let cert_type_str = cert_type_str.trim_end_matches('\0').to_string();

        let cert = verify_one(
            self.active
                .find_certificates(
                    &FindCertificatesArgs {
                        partial: CertificatePartial {
                            certifier: Some(certifier_hex),
                            serial_number: Some(
                                String::from_utf8_lossy(&args.serial_number.0)
                                    .trim_end_matches('\0')
                                    .to_string(),
                            ),
                            cert_type: Some(cert_type_str),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    None,
                )
                .await?,
        )?;

        // Soft-delete by setting is_deleted = true
        self.active
            .update_certificate(
                cert.certificate_id,
                &CertificatePartial {
                    is_deleted: Some(true),
                    ..Default::default()
                },
                None,
            )
            .await?;

        if let Some(ref backup) = self.backup {
            if let Err(e) = backup
                .update_certificate(
                    cert.certificate_id,
                    &CertificatePartial {
                        is_deleted: Some(true),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                warn!(error = %e, "Backup: relinquish_certificate failed");
            }
        }

        Ok(RelinquishCertificateResult { relinquished: true })
    }
}

// ---------------------------------------------------------------------------
// Helper macro for write operations that propagate to backup
// ---------------------------------------------------------------------------

/// Execute an insert on the active provider, and if a backup is present, also
/// propagate the insert (logging but not failing on backup errors).
macro_rules! write_with_backup {
    ($self:ident, $method:ident, $($arg:expr),* $(,)?) => {{
        let _guard = $self.lock.lock().await;
        let result = $self.active.$method($($arg),*).await;
        if let Some(ref backup) = $self.backup {
            if let Err(e) = backup.$method($($arg),*).await {
                warn!(
                    method = stringify!($method),
                    error = %e,
                    "Backup provider write failed, continuing with active-only"
                );
            }
        }
        result
    }};
}

// ---------------------------------------------------------------------------
// StorageReader implementation -- delegates to active provider
// ---------------------------------------------------------------------------

#[async_trait]
impl StorageReader for WalletStorageManager {
    async fn find_users(&self, args: &FindUsersArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<User>> {
        self.active.find_users(args, trx).await
    }
    async fn find_certificates(&self, args: &FindCertificatesArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<Certificate>> {
        self.active.find_certificates(args, trx).await
    }
    async fn find_certificate_fields(&self, args: &FindCertificateFieldsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<CertificateField>> {
        self.active.find_certificate_fields(args, trx).await
    }
    async fn find_commissions(&self, args: &FindCommissionsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<Commission>> {
        self.active.find_commissions(args, trx).await
    }
    async fn find_monitor_events(&self, args: &FindMonitorEventsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<MonitorEvent>> {
        self.active.find_monitor_events(args, trx).await
    }
    async fn find_output_baskets(&self, args: &FindOutputBasketsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<OutputBasket>> {
        self.active.find_output_baskets(args, trx).await
    }
    async fn find_output_tag_maps(&self, args: &FindOutputTagMapsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<OutputTagMap>> {
        self.active.find_output_tag_maps(args, trx).await
    }
    async fn find_output_tags(&self, args: &FindOutputTagsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<OutputTag>> {
        self.active.find_output_tags(args, trx).await
    }
    async fn find_outputs(&self, args: &FindOutputsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<Output>> {
        self.active.find_outputs(args, trx).await
    }
    async fn find_proven_txs(&self, args: &FindProvenTxsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<ProvenTx>> {
        self.active.find_proven_txs(args, trx).await
    }
    async fn find_proven_tx_reqs(&self, args: &FindProvenTxReqsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<ProvenTxReq>> {
        self.active.find_proven_tx_reqs(args, trx).await
    }
    async fn find_settings(&self, args: &FindSettingsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<Settings>> {
        self.active.find_settings(args, trx).await
    }
    async fn find_sync_states(&self, args: &FindSyncStatesArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<SyncState>> {
        self.active.find_sync_states(args, trx).await
    }
    async fn find_transactions(&self, args: &FindTransactionsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<Transaction>> {
        self.active.find_transactions(args, trx).await
    }
    async fn find_tx_label_maps(&self, args: &FindTxLabelMapsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<TxLabelMap>> {
        self.active.find_tx_label_maps(args, trx).await
    }
    async fn find_tx_labels(&self, args: &FindTxLabelsArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<TxLabel>> {
        self.active.find_tx_labels(args, trx).await
    }

    // Count methods
    async fn count_users(&self, args: &FindUsersArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_users(args, trx).await
    }
    async fn count_certificates(&self, args: &FindCertificatesArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_certificates(args, trx).await
    }
    async fn count_certificate_fields(&self, args: &FindCertificateFieldsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_certificate_fields(args, trx).await
    }
    async fn count_commissions(&self, args: &FindCommissionsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_commissions(args, trx).await
    }
    async fn count_monitor_events(&self, args: &FindMonitorEventsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_monitor_events(args, trx).await
    }
    async fn count_output_baskets(&self, args: &FindOutputBasketsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_output_baskets(args, trx).await
    }
    async fn count_output_tag_maps(&self, args: &FindOutputTagMapsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_output_tag_maps(args, trx).await
    }
    async fn count_output_tags(&self, args: &FindOutputTagsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_output_tags(args, trx).await
    }
    async fn count_outputs(&self, args: &FindOutputsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_outputs(args, trx).await
    }
    async fn count_proven_txs(&self, args: &FindProvenTxsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_proven_txs(args, trx).await
    }
    async fn count_proven_tx_reqs(&self, args: &FindProvenTxReqsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_proven_tx_reqs(args, trx).await
    }
    async fn count_settings(&self, args: &FindSettingsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_settings(args, trx).await
    }
    async fn count_sync_states(&self, args: &FindSyncStatesArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_sync_states(args, trx).await
    }
    async fn count_transactions(&self, args: &FindTransactionsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_transactions(args, trx).await
    }
    async fn count_tx_label_maps(&self, args: &FindTxLabelMapsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_tx_label_maps(args, trx).await
    }
    async fn count_tx_labels(&self, args: &FindTxLabelsArgs, trx: Option<&TrxToken>) -> WalletResult<i64> {
        self.active.count_tx_labels(args, trx).await
    }

    // For-user methods
    async fn get_proven_txs_for_user(&self, args: &FindForUserSincePagedArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<ProvenTx>> {
        self.active.get_proven_txs_for_user(args, trx).await
    }
    async fn get_proven_tx_reqs_for_user(&self, args: &FindForUserSincePagedArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<ProvenTxReq>> {
        self.active.get_proven_tx_reqs_for_user(args, trx).await
    }
    async fn get_tx_label_maps_for_user(&self, args: &FindForUserSincePagedArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<TxLabelMap>> {
        self.active.get_tx_label_maps_for_user(args, trx).await
    }
    async fn get_output_tag_maps_for_user(&self, args: &FindForUserSincePagedArgs, trx: Option<&TrxToken>) -> WalletResult<Vec<OutputTagMap>> {
        self.active.get_output_tag_maps_for_user(args, trx).await
    }
}

// ---------------------------------------------------------------------------
// StorageReaderWriter implementation -- writes propagate to backup
// ---------------------------------------------------------------------------

#[async_trait]
impl StorageReaderWriter for WalletStorageManager {
    // Transaction management -- delegates to active only.
    // Transactions are single-provider; backup gets eventual consistency via sync.
    async fn begin_transaction(&self) -> WalletResult<TrxToken> {
        self.active.begin_transaction().await
    }
    async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        self.active.commit_transaction(trx).await
    }
    async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()> {
        self.active.rollback_transaction(trx).await
    }

    // Insert methods -- write to active, propagate to backup
    async fn insert_user(&self, user: &User, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_user, user, trx)
    }
    async fn insert_certificate(&self, certificate: &Certificate, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_certificate, certificate, trx)
    }
    async fn insert_certificate_field(&self, field: &CertificateField, trx: Option<&TrxToken>) -> WalletResult<()> {
        write_with_backup!(self, insert_certificate_field, field, trx)
    }
    async fn insert_commission(&self, commission: &Commission, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_commission, commission, trx)
    }
    async fn insert_monitor_event(&self, event: &MonitorEvent, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_monitor_event, event, trx)
    }
    async fn insert_output_basket(&self, basket: &OutputBasket, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_output_basket, basket, trx)
    }
    async fn insert_output_tag(&self, tag: &OutputTag, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_output_tag, tag, trx)
    }
    async fn insert_output_tag_map(&self, tag_map: &OutputTagMap, trx: Option<&TrxToken>) -> WalletResult<()> {
        write_with_backup!(self, insert_output_tag_map, tag_map, trx)
    }
    async fn insert_output(&self, output: &Output, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_output, output, trx)
    }
    async fn insert_proven_tx(&self, proven_tx: &ProvenTx, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_proven_tx, proven_tx, trx)
    }
    async fn insert_proven_tx_req(&self, proven_tx_req: &ProvenTxReq, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_proven_tx_req, proven_tx_req, trx)
    }
    async fn insert_transaction(&self, transaction: &Transaction, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_transaction, transaction, trx)
    }
    async fn insert_tx_label(&self, label: &TxLabel, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_tx_label, label, trx)
    }
    async fn insert_tx_label_map(&self, label_map: &TxLabelMap, trx: Option<&TrxToken>) -> WalletResult<()> {
        write_with_backup!(self, insert_tx_label_map, label_map, trx)
    }
    async fn insert_sync_state(&self, sync_state: &SyncState, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, insert_sync_state, sync_state, trx)
    }

    // Update methods -- write to active, propagate to backup
    async fn update_user(&self, id: i64, update: &UserPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_user, id, update, trx)
    }
    async fn update_certificate(&self, id: i64, update: &CertificatePartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_certificate, id, update, trx)
    }
    async fn update_certificate_field(&self, certificate_id: i64, field_name: &str, update: &CertificateFieldPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_certificate_field, certificate_id, field_name, update, trx)
    }
    async fn update_commission(&self, id: i64, update: &CommissionPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_commission, id, update, trx)
    }
    async fn update_monitor_event(&self, id: i64, update: &MonitorEventPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_monitor_event, id, update, trx)
    }
    async fn update_output_basket(&self, id: i64, update: &OutputBasketPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_output_basket, id, update, trx)
    }
    async fn update_output_tag(&self, id: i64, update: &OutputTagPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_output_tag, id, update, trx)
    }
    async fn update_output_tag_map(&self, output_id: i64, tag_id: i64, update: &OutputTagMapPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_output_tag_map, output_id, tag_id, update, trx)
    }
    async fn update_output(&self, id: i64, update: &OutputPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_output, id, update, trx)
    }
    async fn update_proven_tx(&self, id: i64, update: &ProvenTxPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_proven_tx, id, update, trx)
    }
    async fn update_proven_tx_req(&self, id: i64, update: &ProvenTxReqPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_proven_tx_req, id, update, trx)
    }
    async fn update_settings(&self, update: &SettingsPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_settings, update, trx)
    }
    async fn update_transaction(&self, id: i64, update: &TransactionPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_transaction, id, update, trx)
    }
    async fn update_tx_label(&self, id: i64, update: &TxLabelPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_tx_label, id, update, trx)
    }
    async fn update_tx_label_map(&self, transaction_id: i64, tx_label_id: i64, update: &TxLabelMapPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_tx_label_map, transaction_id, tx_label_id, update, trx)
    }
    async fn update_sync_state(&self, id: i64, update: &SyncStatePartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
        write_with_backup!(self, update_sync_state, id, update, trx)
    }

    // Monitor-related composite methods (Phase 6) -- delegate to active provider
    async fn update_transaction_status(
        &self,
        txid: &str,
        new_status: crate::status::TransactionStatus,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        self.active.update_transaction_status(txid, new_status, trx).await
    }

    async fn update_transactions_status(
        &self,
        txids: &[String],
        new_status: crate::status::TransactionStatus,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        self.active.update_transactions_status(txids, new_status, trx).await
    }

    async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        req_id: i64,
        proven_tx: &crate::tables::ProvenTx,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        self.active.update_proven_tx_req_with_new_proven_tx(req_id, proven_tx, trx).await
    }

    async fn review_status(
        &self,
        aged_limit: chrono::NaiveDateTime,
        trx: Option<&TrxToken>,
    ) -> WalletResult<String> {
        self.active.review_status(aged_limit, trx).await
    }

    async fn purge_data(
        &self,
        params: &crate::storage::find_args::PurgeParams,
        trx: Option<&TrxToken>,
    ) -> WalletResult<String> {
        self.active.purge_data(params, trx).await
    }
}

// ---------------------------------------------------------------------------
// StorageProvider implementation -- delegates to active, then backup
// ---------------------------------------------------------------------------

#[async_trait]
impl StorageProvider for WalletStorageManager {
    async fn migrate_database(&self) -> WalletResult<String> {
        let result = self.active.migrate_database().await?;
        if let Some(ref backup) = self.backup {
            if let Err(e) = backup.migrate_database().await {
                warn!(error = %e, "Backup provider migration failed");
            }
        }
        Ok(result)
    }

    async fn get_settings(&self, trx: Option<&TrxToken>) -> WalletResult<Settings> {
        self.active.get_settings(trx).await
    }

    async fn make_available(&self) -> WalletResult<Settings> {
        let settings = self.active.make_available().await?;
        if let Some(ref backup) = self.backup {
            if let Err(e) = backup.make_available().await {
                warn!(error = %e, "Backup provider make_available failed");
            }
        }
        info!("WalletStorageManager available with active provider");
        Ok(settings)
    }

    async fn destroy(&self) -> WalletResult<()> {
        self.active.destroy().await?;
        if let Some(ref backup) = self.backup {
            if let Err(e) = backup.destroy().await {
                warn!(error = %e, "Backup provider destroy failed");
            }
        }
        Ok(())
    }

    async fn drop_all_data(&self) -> WalletResult<()> {
        self.active.drop_all_data().await?;
        if let Some(ref backup) = self.backup {
            if let Err(e) = backup.drop_all_data().await {
                warn!(error = %e, "Backup provider drop_all_data failed");
            }
        }
        Ok(())
    }

    fn get_storage_identity_key(&self) -> WalletResult<String> {
        self.active.get_storage_identity_key()
    }

    fn is_available(&self) -> bool {
        self.active.is_available()
    }

    fn get_chain(&self) -> Chain {
        self.chain.clone()
    }

    fn set_active(&self, active: bool) {
        self.active.set_active(active);
    }

    fn is_active(&self) -> bool {
        self.active.is_active()
    }
}
