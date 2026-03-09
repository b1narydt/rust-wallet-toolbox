//! StorageReader trait -- the base of the storage trait hierarchy.
//!
//! Provides abstract find/count methods for all 16 table types,
//! plus default helper methods for common single-record lookups.

use async_trait::async_trait;

use crate::error::WalletResult;
use crate::storage::find_args::*;
use crate::storage::verify_one_or_none;
use crate::storage::TrxToken;
use crate::tables::*;

/// Minimal read-only storage interface for the wallet.
///
/// All methods accept an optional `TrxToken` for transaction participation.
/// Implementations must be Send + Sync for use behind `Arc<dyn StorageProvider>`.
#[async_trait]
pub trait StorageReader: Send + Sync {
    // -----------------------------------------------------------------------
    // Abstract find methods (one per table)
    // -----------------------------------------------------------------------

    /// Find users matching the given filter criteria.
    async fn find_users(
        &self,
        args: &FindUsersArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<User>>;

    /// Find certificates matching the given filter criteria.
    async fn find_certificates(
        &self,
        args: &FindCertificatesArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Certificate>>;

    /// Find certificate fields matching the given filter criteria.
    async fn find_certificate_fields(
        &self,
        args: &FindCertificateFieldsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<CertificateField>>;

    /// Find commissions matching the given filter criteria.
    async fn find_commissions(
        &self,
        args: &FindCommissionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Commission>>;

    /// Find monitor events matching the given filter criteria.
    async fn find_monitor_events(
        &self,
        args: &FindMonitorEventsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<MonitorEvent>>;

    /// Find output baskets matching the given filter criteria.
    async fn find_output_baskets(
        &self,
        args: &FindOutputBasketsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<OutputBasket>>;

    /// Find output tag mappings matching the given filter criteria.
    async fn find_output_tag_maps(
        &self,
        args: &FindOutputTagMapsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<OutputTagMap>>;

    /// Find output tags matching the given filter criteria.
    async fn find_output_tags(
        &self,
        args: &FindOutputTagsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<OutputTag>>;

    /// Find outputs matching the given filter criteria.
    async fn find_outputs(
        &self,
        args: &FindOutputsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Output>>;

    /// Find proven transactions matching the given filter criteria.
    async fn find_proven_txs(
        &self,
        args: &FindProvenTxsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTx>>;

    /// Find proven transaction requests matching the given filter criteria.
    async fn find_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTxReq>>;

    /// Find settings records matching the given filter criteria.
    async fn find_settings(
        &self,
        args: &FindSettingsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Settings>>;

    /// Find sync states matching the given filter criteria.
    async fn find_sync_states(
        &self,
        args: &FindSyncStatesArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<SyncState>>;

    /// Find transactions matching the given filter criteria.
    async fn find_transactions(
        &self,
        args: &FindTransactionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<Transaction>>;

    /// Find transaction label mappings matching the given filter criteria.
    async fn find_tx_label_maps(
        &self,
        args: &FindTxLabelMapsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<TxLabelMap>>;

    /// Find transaction labels matching the given filter criteria.
    async fn find_tx_labels(
        &self,
        args: &FindTxLabelsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<TxLabel>>;

    // -----------------------------------------------------------------------
    // Abstract count methods (one per table)
    // -----------------------------------------------------------------------

    /// Count users matching the given filter criteria.
    async fn count_users(
        &self,
        args: &FindUsersArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count certificates matching the given filter criteria.
    async fn count_certificates(
        &self,
        args: &FindCertificatesArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count certificate fields matching the given filter criteria.
    async fn count_certificate_fields(
        &self,
        args: &FindCertificateFieldsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count commissions matching the given filter criteria.
    async fn count_commissions(
        &self,
        args: &FindCommissionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count monitor events matching the given filter criteria.
    async fn count_monitor_events(
        &self,
        args: &FindMonitorEventsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count output baskets matching the given filter criteria.
    async fn count_output_baskets(
        &self,
        args: &FindOutputBasketsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count output tag mappings matching the given filter criteria.
    async fn count_output_tag_maps(
        &self,
        args: &FindOutputTagMapsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count output tags matching the given filter criteria.
    async fn count_output_tags(
        &self,
        args: &FindOutputTagsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count outputs matching the given filter criteria.
    async fn count_outputs(
        &self,
        args: &FindOutputsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count proven transactions matching the given filter criteria.
    async fn count_proven_txs(
        &self,
        args: &FindProvenTxsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count proven transaction requests matching the given filter criteria.
    async fn count_proven_tx_reqs(
        &self,
        args: &FindProvenTxReqsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count settings records matching the given filter criteria.
    async fn count_settings(
        &self,
        args: &FindSettingsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count sync states matching the given filter criteria.
    async fn count_sync_states(
        &self,
        args: &FindSyncStatesArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count transactions matching the given filter criteria.
    async fn count_transactions(
        &self,
        args: &FindTransactionsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count transaction label mappings matching the given filter criteria.
    async fn count_tx_label_maps(
        &self,
        args: &FindTxLabelMapsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Count transaction labels matching the given filter criteria.
    async fn count_tx_labels(
        &self,
        args: &FindTxLabelsArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    // -----------------------------------------------------------------------
    // Abstract for-user methods (TS getProvenTxsForUser, etc.)
    // -----------------------------------------------------------------------

    /// Get proven transactions for a specific user.
    async fn get_proven_txs_for_user(
        &self,
        args: &FindForUserSincePagedArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTx>>;

    /// Get proven transaction requests for a specific user.
    async fn get_proven_tx_reqs_for_user(
        &self,
        args: &FindForUserSincePagedArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<ProvenTxReq>>;

    /// Get transaction label mappings for a specific user.
    async fn get_tx_label_maps_for_user(
        &self,
        args: &FindForUserSincePagedArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<TxLabelMap>>;

    /// Get output tag mappings for a specific user.
    async fn get_output_tag_maps_for_user(
        &self,
        args: &FindForUserSincePagedArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<OutputTagMap>>;

    // -----------------------------------------------------------------------
    // Default helper methods
    // -----------------------------------------------------------------------

    /// Find a user by identity key. Returns None if not found.
    async fn find_user_by_identity_key(
        &self,
        key: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<User>> {
        let args = FindUsersArgs {
            partial: UserPartial {
                identity_key: Some(key.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_users(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find a certificate by its ID. Returns None if not found.
    async fn find_certificate_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<Certificate>> {
        let args = FindCertificatesArgs {
            partial: CertificatePartial {
                certificate_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_certificates(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find an output basket by its ID. Returns None if not found.
    async fn find_output_basket_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<OutputBasket>> {
        let args = FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                basket_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_output_baskets(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find an output by its ID. Returns None if not found.
    async fn find_output_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<Output>> {
        let args = FindOutputsArgs {
            partial: OutputPartial {
                output_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_outputs(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find a proven transaction by its ID. Returns None if not found.
    async fn find_proven_tx_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<ProvenTx>> {
        let args = FindProvenTxsArgs {
            partial: ProvenTxPartial {
                proven_tx_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_proven_txs(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find a transaction by its ID. Returns None if not found.
    async fn find_transaction_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<Transaction>> {
        let args = FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_transactions(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find a transaction label by its ID. Returns None if not found.
    async fn find_tx_label_by_id(
        &self,
        id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<TxLabel>> {
        let args = FindTxLabelsArgs {
            partial: TxLabelPartial {
                tx_label_id: Some(id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_tx_labels(&args, trx).await?;
        verify_one_or_none(results)
    }

    /// Find settings by storage identity key. Returns None if not found.
    async fn find_settings_by_identity_key(
        &self,
        key: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Option<Settings>> {
        let args = FindSettingsArgs {
            partial: SettingsPartial {
                storage_identity_key: Some(key.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = self.find_settings(&args, trx).await?;
        verify_one_or_none(results)
    }
}
