//! StorageReaderWriter trait -- extends StorageReader with mutation operations.
//!
//! Provides abstract insert/update methods for all table types,
//! transaction management (begin/commit/rollback), and default
//! find-or-insert helper methods.

use async_trait::async_trait;

use crate::error::WalletResult;
use crate::storage::find_args::*;
use crate::storage::traits::reader::StorageReader;
use crate::storage::verify_one_or_none;
use crate::storage::TrxToken;
use crate::tables::*;
use crate::wallet::types::AdminStatsResult;

/// Read-write storage interface extending StorageReader with insert, update,
/// and transaction management operations.
#[async_trait]
pub trait StorageReaderWriter: StorageReader {
    // -----------------------------------------------------------------------
    // Transaction management
    // -----------------------------------------------------------------------

    /// Begin a new database transaction. Returns a type-erased TrxToken.
    async fn begin_transaction(&self) -> WalletResult<TrxToken>;

    /// Commit a previously begun transaction.
    async fn commit_transaction(&self, trx: TrxToken) -> WalletResult<()>;

    /// Rollback a previously begun transaction.
    async fn rollback_transaction(&self, trx: TrxToken) -> WalletResult<()>;

    // -----------------------------------------------------------------------
    // Abstract insert methods
    // -----------------------------------------------------------------------

    /// Insert a user and return the new user_id.
    async fn insert_user(&self, user: &User, trx: Option<&TrxToken>) -> WalletResult<i64>;

    /// Insert a certificate and return the new certificate_id.
    async fn insert_certificate(
        &self,
        certificate: &Certificate,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert a certificate field.
    async fn insert_certificate_field(
        &self,
        field: &CertificateField,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()>;

    /// Insert a commission and return the new commission_id.
    async fn insert_commission(
        &self,
        commission: &Commission,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert a monitor event and return the new id.
    async fn insert_monitor_event(
        &self,
        event: &MonitorEvent,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Delete monitor events matching `event` name with `id < before_id`.
    ///
    /// Used to prune stale checkpoint entries after a complete review cycle.
    /// Returns the number of rows deleted.
    async fn delete_monitor_events_before_id(
        &self,
        event_name: &str,
        before_id: i64,
        trx: Option<&TrxToken>,
    ) -> WalletResult<u64>;

    /// Insert an output basket and return the new basket_id.
    async fn insert_output_basket(
        &self,
        basket: &OutputBasket,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert an output tag and return the new output_tag_id.
    async fn insert_output_tag(&self, tag: &OutputTag, trx: Option<&TrxToken>)
        -> WalletResult<i64>;

    /// Insert an output tag map entry.
    async fn insert_output_tag_map(
        &self,
        tag_map: &OutputTagMap,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()>;

    /// Insert an output and return the new output_id.
    async fn insert_output(&self, output: &Output, trx: Option<&TrxToken>) -> WalletResult<i64>;

    /// Insert a proven transaction and return the new proven_tx_id.
    async fn insert_proven_tx(
        &self,
        proven_tx: &ProvenTx,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert a proven transaction request and return the new proven_tx_req_id.
    async fn insert_proven_tx_req(
        &self,
        proven_tx_req: &ProvenTxReq,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert a transaction and return the new transaction_id.
    async fn insert_transaction(
        &self,
        transaction: &Transaction,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Insert a transaction label and return the new tx_label_id.
    async fn insert_tx_label(&self, label: &TxLabel, trx: Option<&TrxToken>) -> WalletResult<i64>;

    /// Insert a transaction label map entry.
    async fn insert_tx_label_map(
        &self,
        label_map: &TxLabelMap,
        trx: Option<&TrxToken>,
    ) -> WalletResult<()>;

    /// Insert a sync state and return the new sync_state_id.
    async fn insert_sync_state(
        &self,
        sync_state: &SyncState,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    // -----------------------------------------------------------------------
    // Abstract update methods
    // -----------------------------------------------------------------------

    /// Update a user by ID. Returns the number of rows affected.
    async fn update_user(
        &self,
        id: i64,
        update: &UserPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a certificate by ID. Returns the number of rows affected.
    async fn update_certificate(
        &self,
        id: i64,
        update: &CertificatePartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a certificate field by certificate_id and field_name.
    /// Returns the number of rows affected.
    async fn update_certificate_field(
        &self,
        certificate_id: i64,
        field_name: &str,
        update: &CertificateFieldPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a commission by ID. Returns the number of rows affected.
    async fn update_commission(
        &self,
        id: i64,
        update: &CommissionPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a monitor event by ID. Returns the number of rows affected.
    async fn update_monitor_event(
        &self,
        id: i64,
        update: &MonitorEventPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update an output basket by ID. Returns the number of rows affected.
    async fn update_output_basket(
        &self,
        id: i64,
        update: &OutputBasketPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update an output tag by ID. Returns the number of rows affected.
    async fn update_output_tag(
        &self,
        id: i64,
        update: &OutputTagPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update an output tag map by output_id and tag_id.
    /// Returns the number of rows affected.
    async fn update_output_tag_map(
        &self,
        output_id: i64,
        tag_id: i64,
        update: &OutputTagMapPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update an output by ID. Returns the number of rows affected.
    async fn update_output(
        &self,
        id: i64,
        update: &OutputPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a proven transaction by ID. Returns the number of rows affected.
    async fn update_proven_tx(
        &self,
        id: i64,
        update: &ProvenTxPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update proven transaction request(s) by ID or IDs.
    /// Returns the number of rows affected.
    async fn update_proven_tx_req(
        &self,
        id: i64,
        update: &ProvenTxReqPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update settings. Returns the number of rows affected.
    async fn update_settings(
        &self,
        update: &SettingsPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a transaction by ID. Returns the number of rows affected.
    async fn update_transaction(
        &self,
        id: i64,
        update: &TransactionPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a transaction label by ID. Returns the number of rows affected.
    async fn update_tx_label(
        &self,
        id: i64,
        update: &TxLabelPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a transaction label map by transaction_id and tx_label_id.
    /// Returns the number of rows affected.
    async fn update_tx_label_map(
        &self,
        transaction_id: i64,
        tx_label_id: i64,
        update: &TxLabelMapPartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    /// Update a sync state by ID. Returns the number of rows affected.
    async fn update_sync_state(
        &self,
        id: i64,
        update: &SyncStatePartial,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64>;

    // -----------------------------------------------------------------------
    // Monitor-related composite methods (Phase 6)
    // -----------------------------------------------------------------------

    /// Update a transaction's status by txid.
    ///
    /// NOTE: as of the broadcast-outcome correctness fix, this method only
    /// updates the transaction row's status. Callers that need to also
    /// release locked UTXOs (the inputs this tx consumed) must call
    /// [`Self::restore_consumed_inputs`] explicitly. That separation is required
    /// because some failure paths (e.g. DoubleSpend) need to restore ONLY
    /// chain-verified-unspent inputs, not every `spent_by = tx_id` row.
    async fn update_transaction_status(
        &self,
        _txid: &str,
        _new_status: crate::status::TransactionStatus,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        Err(crate::error::WalletError::NotImplemented(
            "update_transaction_status".to_string(),
        ))
    }

    /// Batch version of update_transaction_status.
    async fn update_transactions_status(
        &self,
        _txids: &[String],
        _new_status: crate::status::TransactionStatus,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<()> {
        Err(crate::error::WalletError::NotImplemented(
            "update_transactions_status".to_string(),
        ))
    }

    /// Restore every output this transaction consumed (rows with
    /// `spent_by = tx_id`) back to `spendable=true, spent_by=NULL`.
    ///
    /// Returns the number of output rows updated.
    ///
    /// This is the "release locked UTXOs" side-effect that used to be
    /// implicit in `update_transaction_status(..., Failed, ..)`. It is now
    /// explicit so callers with partial-restore semantics (e.g. DoubleSpend
    /// recovery, which only restores chain-verified-unspent inputs) can
    /// avoid the blanket cascade.
    async fn restore_consumed_inputs(
        &self,
        _tx_id: i64,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<u64> {
        Err(crate::error::WalletError::NotImplemented(
            "restore_consumed_inputs".to_string(),
        ))
    }

    /// Composite: inserts ProvenTx, then updates ProvenTxReq with proven_tx_id
    /// and status=completed.
    async fn update_proven_tx_req_with_new_proven_tx(
        &self,
        _req_id: i64,
        _proven_tx: &crate::tables::ProvenTx,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        Err(crate::error::WalletError::NotImplemented(
            "update_proven_tx_req_with_new_proven_tx".to_string(),
        ))
    }

    /// Finds reqs with stale statuses (e.g., unsent/sending older than aged_limit)
    /// and logs/corrects them. Returns summary string.
    async fn review_status(
        &self,
        _aged_limit: chrono::NaiveDateTime,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<String> {
        Err(crate::error::WalletError::NotImplemented(
            "review_status".to_string(),
        ))
    }

    /// Deletes old records based on purge params. Returns summary string.
    async fn purge_data(
        &self,
        _params: &crate::storage::find_args::PurgeParams,
        _trx: Option<&TrxToken>,
    ) -> WalletResult<String> {
        Err(crate::error::WalletError::NotImplemented(
            "purge_data".to_string(),
        ))
    }

    /// Returns aggregate deployment statistics for the admin dashboard.
    ///
    /// Default implementation returns `NotImplemented`. Storage backends
    /// (e.g., SQLite) can override with real SQL aggregate queries.
    async fn admin_stats(&self, _auth_id: &str) -> WalletResult<AdminStatsResult> {
        Err(crate::error::WalletError::NotImplemented(
            "admin_stats".to_string(),
        ))
    }

    // -----------------------------------------------------------------------
    // Default find-or-insert helper methods
    // -----------------------------------------------------------------------

    /// Find a user by identity key, or insert a new one if not found.
    async fn find_or_insert_user(
        &self,
        identity_key: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<(User, bool)> {
        let args = FindUsersArgs {
            partial: UserPartial {
                identity_key: Some(identity_key.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let existing = verify_one_or_none(self.find_users(&args, trx).await?)?;
        if let Some(user) = existing {
            return Ok((user, false));
        }
        let now = chrono::Utc::now().naive_utc();
        let new_user = User {
            created_at: now,
            updated_at: chrono::NaiveDate::from_ymd_opt(1971, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            user_id: 0,
            identity_key: identity_key.to_string(),
            active_storage: String::new(),
        };
        let user_id = self.insert_user(&new_user, trx).await?;
        let mut user = new_user;
        user.user_id = user_id;
        Ok((user, true))
    }

    /// Find an output basket by user and name, or insert a new one if not found.
    async fn find_or_insert_output_basket(
        &self,
        user_id: i64,
        name: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<OutputBasket> {
        let args = FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                user_id: Some(user_id),
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let existing = verify_one_or_none(self.find_output_baskets(&args, trx).await?)?;
        if let Some(basket) = existing {
            return Ok(basket);
        }
        let now = chrono::Utc::now().naive_utc();
        let new_basket = OutputBasket {
            created_at: now,
            updated_at: now,
            basket_id: 0,
            user_id,
            name: name.to_string(),
            number_of_desired_utxos: 0,
            minimum_desired_utxo_value: 0,
            is_deleted: false,
        };
        let basket_id = self.insert_output_basket(&new_basket, trx).await?;
        let mut basket = new_basket;
        basket.basket_id = basket_id;
        Ok(basket)
    }

    /// Find a transaction label by user and label text, or insert if not found.
    async fn find_or_insert_tx_label(
        &self,
        user_id: i64,
        label: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<TxLabel> {
        let args = FindTxLabelsArgs {
            partial: TxLabelPartial {
                user_id: Some(user_id),
                label: Some(label.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let existing = verify_one_or_none(self.find_tx_labels(&args, trx).await?)?;
        if let Some(tx_label) = existing {
            return Ok(tx_label);
        }
        let now = chrono::Utc::now().naive_utc();
        let new_label = TxLabel {
            created_at: now,
            updated_at: now,
            tx_label_id: 0,
            user_id,
            label: label.to_string(),
            is_deleted: false,
        };
        let tx_label_id = self.insert_tx_label(&new_label, trx).await?;
        let mut result = new_label;
        result.tx_label_id = tx_label_id;
        Ok(result)
    }

    /// Find an output tag by user and tag text, or insert if not found.
    async fn find_or_insert_output_tag(
        &self,
        user_id: i64,
        tag: &str,
        trx: Option<&TrxToken>,
    ) -> WalletResult<OutputTag> {
        let args = FindOutputTagsArgs {
            partial: OutputTagPartial {
                user_id: Some(user_id),
                tag: Some(tag.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let existing = verify_one_or_none(self.find_output_tags(&args, trx).await?)?;
        if let Some(output_tag) = existing {
            return Ok(output_tag);
        }
        let now = chrono::Utc::now().naive_utc();
        let new_tag = OutputTag {
            created_at: now,
            updated_at: now,
            output_tag_id: 0,
            user_id,
            tag: tag.to_string(),
            is_deleted: false,
        };
        let output_tag_id = self.insert_output_tag(&new_tag, trx).await?;
        let mut result = new_tag;
        result.output_tag_id = output_tag_id;
        Ok(result)
    }
}
