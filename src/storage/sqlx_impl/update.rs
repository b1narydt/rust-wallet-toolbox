//! Update implementations for StorageSqlx backends.
//!
//! Provides UPDATE operations for all 16 table types. Updates use the Partial
//! structs to set only the fields that are Some.
//! MySQL and PostgreSQL implementations are generated via the
//! `impl_update_methods!` macro.

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use sqlx::Sqlite;

    use crate::error::{WalletError, WalletResult};
    use crate::storage::find_args::*;
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::TrxToken;

    /// Bind value enum matching the one in find.rs, duplicated here to keep
    /// update module self-contained.
    #[derive(Debug, Clone)]
    enum BindVal {
        Int64(i64),
        Int32(i32),
        String(String),
        Bool(bool),
    }

    /// Execute an UPDATE with dynamic SET clauses and bind values.
    async fn exec_update(
        storage: &StorageSqlx<Sqlite>,
        sql: &str,
        binds: &[BindVal],
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        if let Some(trx_token) = trx {
            let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx(trx_token)?;
            let mut guard = inner.lock().await;
            let tx = guard
                .as_mut()
                .ok_or_else(|| WalletError::Internal("Transaction already consumed".to_string()))?;
            let mut q = sqlx::query(sql);
            for bind in binds {
                q = match bind {
                    BindVal::Int64(v) => q.bind(*v),
                    BindVal::Int32(v) => q.bind(*v),
                    BindVal::String(v) => q.bind(v.as_str()),
                    BindVal::Bool(v) => q.bind(*v),
                };
            }
            let result = q.execute(&mut **tx).await?;
            Ok(result.rows_affected() as i64)
        } else {
            let mut q = sqlx::query(sql);
            for bind in binds {
                q = match bind {
                    BindVal::Int64(v) => q.bind(*v),
                    BindVal::Int32(v) => q.bind(*v),
                    BindVal::String(v) => q.bind(v.as_str()),
                    BindVal::Bool(v) => q.bind(*v),
                };
            }
            let result = q.execute(&storage.write_pool).await?;
            Ok(result.rows_affected() as i64)
        }
    }

    impl StorageSqlx<Sqlite> {
        pub(crate) async fn update_user_impl(
            &self,
            id: i64,
            update: &UserPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.identity_key {
                sets.push("identityKey = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.active_storage {
                sets.push("activeStorage = ?");
                binds.push(BindVal::String(v.clone()));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!("UPDATE users SET {} WHERE userId = ?", sets.join(", "));
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_certificate_impl(
            &self,
            id: i64,
            update: &CertificatePartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.cert_type {
                sets.push("type = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.serial_number {
                sets.push("serialNumber = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.certifier {
                sets.push("certifier = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.subject {
                sets.push("subject = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.verifier {
                sets.push("verifier = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.revocation_outpoint {
                sets.push("revocationOutpoint = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.signature {
                sets.push("signature = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE certificates SET {} WHERE certificateId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_certificate_field_impl(
            &self,
            certificate_id: i64,
            field_name: &str,
            update: &CertificateFieldPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.field_value {
                sets.push("fieldValue = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.master_key {
                sets.push("masterKey = ?");
                binds.push(BindVal::String(v.clone()));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE certificate_fields SET {} WHERE certificateId = ? AND fieldName = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(certificate_id));
            binds.push(BindVal::String(field_name.to_string()));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_commission_impl(
            &self,
            id: i64,
            update: &CommissionPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.transaction_id {
                sets.push("transactionId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.satoshis {
                sets.push("satoshis = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.key_offset {
                sets.push("keyOffset = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_redeemed {
                sets.push("isRedeemed = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE commissions SET {} WHERE commissionId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_monitor_event_impl(
            &self,
            id: i64,
            update: &MonitorEventPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.event {
                sets.push("event = ?");
                binds.push(BindVal::String(v.clone()));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!("UPDATE monitor_events SET {} WHERE id = ?", sets.join(", "));
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_output_basket_impl(
            &self,
            id: i64,
            update: &OutputBasketPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.name {
                sets.push("name = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            if let Some(v) = &update.number_of_desired_utxos {
                sets.push("numberOfDesiredUTXOs = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.minimum_desired_utxo_value {
                sets.push("minimumDesiredUTXOValue = ?");
                binds.push(BindVal::Int64(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE output_baskets SET {} WHERE basketId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_output_tag_impl(
            &self,
            id: i64,
            update: &OutputTagPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.tag {
                sets.push("tag = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE output_tags SET {} WHERE outputTagId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_output_tag_map_impl(
            &self,
            output_id: i64,
            tag_id: i64,
            update: &OutputTagMapPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE output_tags_map SET {} WHERE outputId = ? AND outputTagId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(output_id));
            binds.push(BindVal::Int64(tag_id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_output_impl(
            &self,
            id: i64,
            update: &OutputPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.transaction_id {
                sets.push("transactionId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.basket_id {
                sets.push("basketId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.spendable {
                sets.push("spendable = ?");
                binds.push(BindVal::Bool(*v));
            }
            if let Some(v) = &update.change {
                sets.push("change = ?");
                binds.push(BindVal::Bool(*v));
            }
            if let Some(v) = &update.vout {
                sets.push("vout = ?");
                binds.push(BindVal::Int32(*v));
            }
            if let Some(v) = &update.satoshis {
                sets.push("satoshis = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.provided_by {
                sets.push("providedBy = ?");
                binds.push(BindVal::String(v.to_string()));
            }
            if let Some(v) = &update.purpose {
                sets.push("purpose = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.output_type {
                sets.push("type = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.txid {
                sets.push("txid = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.sender_identity_key {
                sets.push("senderIdentityKey = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.spent_by {
                sets.push("spentBy = ?");
                binds.push(BindVal::Int64(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!("UPDATE outputs SET {} WHERE outputId = ?", sets.join(", "));
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_proven_tx_impl(
            &self,
            id: i64,
            update: &ProvenTxPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.txid {
                sets.push("txid = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.height {
                sets.push("height = ?");
                binds.push(BindVal::Int32(*v));
            }
            if let Some(v) = &update.block_hash {
                sets.push("blockHash = ?");
                binds.push(BindVal::String(v.clone()));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE proven_txs SET {} WHERE provenTxId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_proven_tx_req_impl(
            &self,
            id: i64,
            update: &ProvenTxReqPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.proven_tx_id {
                sets.push("provenTxId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.status {
                sets.push("status = ?");
                binds.push(BindVal::String(v.to_string()));
            }
            if let Some(v) = &update.txid {
                sets.push("txid = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.batch {
                sets.push("batch = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.notified {
                sets.push("notified = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE proven_tx_reqs SET {} WHERE provenTxReqId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_settings_impl(
            &self,
            update: &SettingsPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.storage_identity_key {
                sets.push("storageIdentityKey = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.storage_name {
                sets.push("storageName = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.chain {
                sets.push("chain = ?");
                binds.push(BindVal::String(v.to_string()));
            }
            sets.push("updated_at = datetime('now')");
            // Settings has no PK -- update all rows
            let sql = format!("UPDATE settings SET {}", sets.join(", "));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_transaction_impl(
            &self,
            id: i64,
            update: &TransactionPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.proven_tx_id {
                sets.push("provenTxId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.status {
                sets.push("status = ?");
                binds.push(BindVal::String(v.to_string()));
            }
            if let Some(v) = &update.reference {
                sets.push("reference = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_outgoing {
                sets.push("isOutgoing = ?");
                binds.push(BindVal::Bool(*v));
            }
            if let Some(v) = &update.txid {
                sets.push("txid = ?");
                binds.push(BindVal::String(v.clone()));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE transactions SET {} WHERE transactionId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_tx_label_impl(
            &self,
            id: i64,
            update: &TxLabelPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.label {
                sets.push("label = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE tx_labels SET {} WHERE txLabelId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_tx_label_map_impl(
            &self,
            transaction_id: i64,
            tx_label_id: i64,
            update: &TxLabelMapPartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.is_deleted {
                sets.push("isDeleted = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE tx_labels_map SET {} WHERE transactionId = ? AND txLabelId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(transaction_id));
            binds.push(BindVal::Int64(tx_label_id));
            exec_update(self, &sql, &binds, trx).await
        }

        pub(crate) async fn update_sync_state_impl(
            &self,
            id: i64,
            update: &SyncStatePartial,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let mut sets = Vec::new();
            let mut binds = Vec::new();
            if let Some(v) = &update.user_id {
                sets.push("userId = ?");
                binds.push(BindVal::Int64(*v));
            }
            if let Some(v) = &update.storage_identity_key {
                sets.push("storageIdentityKey = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.storage_name {
                sets.push("storageName = ?");
                binds.push(BindVal::String(v.clone()));
            }
            if let Some(v) = &update.status {
                sets.push("status = ?");
                binds.push(BindVal::String(v.to_string()));
            }
            if let Some(v) = &update.init {
                sets.push("init = ?");
                binds.push(BindVal::Bool(*v));
            }
            sets.push("updated_at = datetime('now')");
            let sql = format!(
                "UPDATE sync_states SET {} WHERE syncStateId = ?",
                sets.join(", ")
            );
            binds.push(BindVal::Int64(id));
            exec_update(self, &sql, &binds, trx).await
        }
    }
}

// ---------------------------------------------------------------------------
// Macro for MySQL/PostgreSQL update implementations
// ---------------------------------------------------------------------------
//
// The update logic is identical across backends except for:
// - Placeholder style: `?` for MySQL, `$N` for PostgreSQL
// - NOW() expression: `NOW()` for both MySQL and PostgreSQL (vs datetime('now') for SQLite)
// - Transaction extraction function
//
// The macro generates all 16 update methods plus the exec_update helper.

macro_rules! impl_update_methods {
    (
        feature = $feature:literal,
        mod_name = $mod_name:ident,
        db = $db:ty,
        args_ty = $args_ty:ty,
        extract_trx = $extract:path,
        now_expr = $now_expr:literal,
        placeholder_fn = $ph_fn:expr
    ) => {
        #[cfg(feature = $feature)]
        mod $mod_name {
            use crate::error::{WalletError, WalletResult};
            use crate::storage::find_args::*;
            use crate::storage::sqlx_impl::StorageSqlx;
            use crate::storage::TrxToken;

            #[derive(Debug, Clone)]
            enum BindVal {
                Int64(i64),
                Int32(i32),
                String(String),
                Bool(bool),
            }

            async fn exec_update(
                storage: &StorageSqlx<$db>,
                sql: &str,
                binds: &[BindVal],
                trx: Option<&TrxToken>,
            ) -> WalletResult<i64> {
                if let Some(trx_token) = trx {
                    let inner = $extract(trx_token)?;
                    let mut guard = inner.lock().await;
                    let tx = guard.as_mut().ok_or_else(|| {
                        WalletError::Internal("Transaction already consumed".to_string())
                    })?;
                    let mut q = sqlx::query(sql);
                    for bind in binds {
                        q = match bind {
                            BindVal::Int64(v) => q.bind(*v),
                            BindVal::Int32(v) => q.bind(*v),
                            BindVal::String(v) => q.bind(v.as_str()),
                            BindVal::Bool(v) => q.bind(*v),
                        };
                    }
                    let result = q.execute(&mut **tx).await?;
                    Ok(result.rows_affected() as i64)
                } else {
                    let mut q = sqlx::query(sql);
                    for bind in binds {
                        q = match bind {
                            BindVal::Int64(v) => q.bind(*v),
                            BindVal::Int32(v) => q.bind(*v),
                            BindVal::String(v) => q.bind(v.as_str()),
                            BindVal::Bool(v) => q.bind(*v),
                        };
                    }
                    let result = q.execute(&storage.write_pool).await?;
                    Ok(result.rows_affected() as i64)
                }
            }

            /// Returns the placeholder for the given 1-based index.
            fn ph(index: usize) -> String {
                ($ph_fn)(index)
            }

            impl StorageSqlx<$db> {
                pub(crate) async fn update_user_impl(&self, id: i64, update: &UserPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.identity_key { idx += 1; sets.push(format!("identityKey = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.active_storage { idx += 1; sets.push(format!("activeStorage = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE users SET {} WHERE userId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_certificate_impl(&self, id: i64, update: &CertificatePartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.cert_type { idx += 1; sets.push(format!("type = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.serial_number { idx += 1; sets.push(format!("serialNumber = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.certifier { idx += 1; sets.push(format!("certifier = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.subject { idx += 1; sets.push(format!("subject = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.verifier { idx += 1; sets.push(format!("verifier = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.revocation_outpoint { idx += 1; sets.push(format!("revocationOutpoint = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.signature { idx += 1; sets.push(format!("signature = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE certificates SET {} WHERE certificateId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_certificate_field_impl(&self, certificate_id: i64, field_name: &str, update: &CertificateFieldPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.field_value { idx += 1; sets.push(format!("fieldValue = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.master_key { idx += 1; sets.push(format!("masterKey = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let p1 = ph(idx);
                    idx += 1; let p2 = ph(idx);
                    let sql = format!("UPDATE certificate_fields SET {} WHERE certificateId = {} AND fieldName = {}", sets.join(", "), p1, p2);
                    binds.push(BindVal::Int64(certificate_id));
                    binds.push(BindVal::String(field_name.to_string()));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_commission_impl(&self, id: i64, update: &CommissionPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.transaction_id { idx += 1; sets.push(format!("transactionId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.satoshis { idx += 1; sets.push(format!("satoshis = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.key_offset { idx += 1; sets.push(format!("keyOffset = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_redeemed { idx += 1; sets.push(format!("isRedeemed = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE commissions SET {} WHERE commissionId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_monitor_event_impl(&self, id: i64, update: &MonitorEventPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.event { idx += 1; sets.push(format!("event = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE monitor_events SET {} WHERE id = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_output_basket_impl(&self, id: i64, update: &OutputBasketPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.name { idx += 1; sets.push(format!("name = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    if let Some(v) = &update.number_of_desired_utxos { idx += 1; sets.push(format!("numberOfDesiredUTXOs = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.minimum_desired_utxo_value { idx += 1; sets.push(format!("minimumDesiredUTXOValue = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE output_baskets SET {} WHERE basketId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_output_tag_impl(&self, id: i64, update: &OutputTagPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.tag { idx += 1; sets.push(format!("tag = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE output_tags SET {} WHERE outputTagId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_output_tag_map_impl(&self, output_id: i64, tag_id: i64, update: &OutputTagMapPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let p1 = ph(idx);
                    idx += 1; let p2 = ph(idx);
                    let sql = format!("UPDATE output_tags_map SET {} WHERE outputId = {} AND outputTagId = {}", sets.join(", "), p1, p2);
                    binds.push(BindVal::Int64(output_id));
                    binds.push(BindVal::Int64(tag_id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_output_impl(&self, id: i64, update: &OutputPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.transaction_id { idx += 1; sets.push(format!("transactionId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.basket_id { idx += 1; sets.push(format!("basketId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.spendable { idx += 1; sets.push(format!("spendable = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    if let Some(v) = &update.change { idx += 1; sets.push(format!("change = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    if let Some(v) = &update.vout { idx += 1; sets.push(format!("vout = {}", ph(idx))); binds.push(BindVal::Int32(*v)); }
                    if let Some(v) = &update.satoshis { idx += 1; sets.push(format!("satoshis = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.provided_by { idx += 1; sets.push(format!("providedBy = {}", ph(idx))); binds.push(BindVal::String(v.to_string())); }
                    if let Some(v) = &update.purpose { idx += 1; sets.push(format!("purpose = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.output_type { idx += 1; sets.push(format!("type = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.txid { idx += 1; sets.push(format!("txid = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.sender_identity_key { idx += 1; sets.push(format!("senderIdentityKey = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.spent_by { idx += 1; sets.push(format!("spentBy = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE outputs SET {} WHERE outputId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_proven_tx_impl(&self, id: i64, update: &ProvenTxPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.txid { idx += 1; sets.push(format!("txid = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.height { idx += 1; sets.push(format!("height = {}", ph(idx))); binds.push(BindVal::Int32(*v)); }
                    if let Some(v) = &update.block_hash { idx += 1; sets.push(format!("blockHash = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE proven_txs SET {} WHERE provenTxId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_proven_tx_req_impl(&self, id: i64, update: &ProvenTxReqPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.proven_tx_id { idx += 1; sets.push(format!("provenTxId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.status { idx += 1; sets.push(format!("status = {}", ph(idx))); binds.push(BindVal::String(v.to_string())); }
                    if let Some(v) = &update.txid { idx += 1; sets.push(format!("txid = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.batch { idx += 1; sets.push(format!("batch = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.notified { idx += 1; sets.push(format!("notified = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE proven_tx_reqs SET {} WHERE provenTxReqId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_settings_impl(&self, update: &SettingsPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.storage_identity_key { idx += 1; sets.push(format!("storageIdentityKey = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.storage_name { idx += 1; sets.push(format!("storageName = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.chain { idx += 1; sets.push(format!("chain = {}", ph(idx))); binds.push(BindVal::String(v.to_string())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    let _ = idx;
                    let sql = format!("UPDATE settings SET {}", sets.join(", "));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_transaction_impl(&self, id: i64, update: &TransactionPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.proven_tx_id { idx += 1; sets.push(format!("provenTxId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.status { idx += 1; sets.push(format!("status = {}", ph(idx))); binds.push(BindVal::String(v.to_string())); }
                    if let Some(v) = &update.reference { idx += 1; sets.push(format!("reference = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_outgoing { idx += 1; sets.push(format!("isOutgoing = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    if let Some(v) = &update.txid { idx += 1; sets.push(format!("txid = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE transactions SET {} WHERE transactionId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_tx_label_impl(&self, id: i64, update: &TxLabelPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.label { idx += 1; sets.push(format!("label = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE tx_labels SET {} WHERE txLabelId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_tx_label_map_impl(&self, transaction_id: i64, tx_label_id: i64, update: &TxLabelMapPartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.is_deleted { idx += 1; sets.push(format!("isDeleted = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let p1 = ph(idx);
                    idx += 1; let p2 = ph(idx);
                    let sql = format!("UPDATE tx_labels_map SET {} WHERE transactionId = {} AND txLabelId = {}", sets.join(", "), p1, p2);
                    binds.push(BindVal::Int64(transaction_id));
                    binds.push(BindVal::Int64(tx_label_id));
                    exec_update(self, &sql, &binds, trx).await
                }

                pub(crate) async fn update_sync_state_impl(&self, id: i64, update: &SyncStatePartial, trx: Option<&TrxToken>) -> WalletResult<i64> {
                    let mut sets = Vec::new();
                    let mut binds = Vec::new();
                    let mut idx = 0usize;
                    if let Some(v) = &update.user_id { idx += 1; sets.push(format!("userId = {}", ph(idx))); binds.push(BindVal::Int64(*v)); }
                    if let Some(v) = &update.storage_identity_key { idx += 1; sets.push(format!("storageIdentityKey = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.storage_name { idx += 1; sets.push(format!("storageName = {}", ph(idx))); binds.push(BindVal::String(v.clone())); }
                    if let Some(v) = &update.status { idx += 1; sets.push(format!("status = {}", ph(idx))); binds.push(BindVal::String(v.to_string())); }
                    if let Some(v) = &update.init { idx += 1; sets.push(format!("init = {}", ph(idx))); binds.push(BindVal::Bool(*v)); }
                    sets.push(format!("updated_at = {}", $now_expr));
                    idx += 1; let sql = format!("UPDATE sync_states SET {} WHERE syncStateId = {}", sets.join(", "), ph(idx));
                    binds.push(BindVal::Int64(id));
                    exec_update(self, &sql, &binds, trx).await
                }
            }
        }
    };
}

impl_update_methods! {
    feature = "mysql",
    mod_name = mysql_update_impl,
    db = sqlx::MySql,
    args_ty = sqlx::mysql::MySqlArguments,
    extract_trx = StorageSqlx::<sqlx::MySql>::extract_mysql_trx,
    now_expr = "NOW()",
    placeholder_fn = |_idx: usize| -> String { "?".to_string() }
}

impl_update_methods! {
    feature = "postgres",
    mod_name = postgres_update_impl,
    db = sqlx::Postgres,
    args_ty = sqlx::postgres::PgArguments,
    extract_trx = StorageSqlx::<sqlx::Postgres>::extract_pg_trx,
    now_expr = "NOW()",
    placeholder_fn = |idx: usize| -> String { format!("${}", idx) }
}
