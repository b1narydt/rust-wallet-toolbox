//! Find and count implementations for StorageSqlx backends.
//!
//! Implements the StorageReader trait for SQLite, MySQL, and PostgreSQL,
//! providing dynamic WHERE clause building for all 16 table types plus
//! the 4 for-user query methods.
//!
//! MySQL and PostgreSQL implementations are generated via the
//! `impl_storage_reader_find!` macro to avoid massive code duplication.
//! The key difference is placeholder style: `?` for SQLite/MySQL, `$N` for PostgreSQL.

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use async_trait::async_trait;
    use sqlx::sqlite::SqliteRow;
    use sqlx::{FromRow, Row, Sqlite};

    use crate::error::{WalletError, WalletResult};
    use crate::storage::find_args::*;
    use crate::storage::sqlx_impl::dialect::WhereBuilder;
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::traits::reader::StorageReader;
    use crate::storage::TrxToken;
    use crate::tables::*;

    // -----------------------------------------------------------------------
    // Helper: execute a query that returns rows, routing through trx or pool
    // -----------------------------------------------------------------------

    /// Execute a SELECT query against the SQLite database. If a transaction token
    /// is provided, the query runs within that transaction; otherwise it uses
    /// the read pool.
    async fn query_rows<T: for<'r> FromRow<'r, SqliteRow> + Send + Unpin>(
        storage: &StorageSqlx<Sqlite>,
        sql: &str,
        binds: Vec<SqliteBindValue>,
        trx: Option<&TrxToken>,
    ) -> WalletResult<Vec<T>> {
        if let Some(trx_token) = trx {
            let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx(trx_token)?;
            let mut guard = inner.lock().await;
            let tx = guard
                .as_mut()
                .ok_or_else(|| WalletError::Internal("Transaction already consumed".to_string()))?;
            let mut q = sqlx::query_as::<_, T>(sql);
            for bind in &binds {
                q = bind_value(q, bind);
            }
            let rows = q.fetch_all(&mut **tx).await?;
            Ok(rows)
        } else {
            let pool = storage.read_pool();
            let mut q = sqlx::query_as::<_, T>(sql);
            for bind in &binds {
                q = bind_value(q, bind);
            }
            let rows = q.fetch_all(pool).await?;
            Ok(rows)
        }
    }

    /// Execute a SELECT COUNT(*) query, returning the count as i64.
    async fn query_count(
        storage: &StorageSqlx<Sqlite>,
        sql: &str,
        binds: Vec<SqliteBindValue>,
        trx: Option<&TrxToken>,
    ) -> WalletResult<i64> {
        if let Some(trx_token) = trx {
            let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx(trx_token)?;
            let mut guard = inner.lock().await;
            let tx = guard
                .as_mut()
                .ok_or_else(|| WalletError::Internal("Transaction already consumed".to_string()))?;
            let mut q = sqlx::query(sql);
            for bind in &binds {
                q = bind_value_raw(q, bind);
            }
            let row = q.fetch_one(&mut **tx).await?;
            let count: i64 = row.get(0);
            Ok(count)
        } else {
            let pool = storage.read_pool();
            let mut q = sqlx::query(sql);
            for bind in &binds {
                q = bind_value_raw(q, bind);
            }
            let row = q.fetch_one(pool).await?;
            let count: i64 = row.get(0);
            Ok(count)
        }
    }

    // -----------------------------------------------------------------------
    // Bind value abstraction
    // -----------------------------------------------------------------------

    /// Represents a value that can be bound to a SQLite query parameter.
    #[derive(Debug, Clone)]
    enum SqliteBindValue {
        Int64(i64),
        Int32(i32),
        String(String),
        Bool(bool),
    }

    /// Bind a SqliteBindValue to a typed query_as.
    fn bind_value<'q, T>(
        q: sqlx::query::QueryAs<'q, Sqlite, T, sqlx::sqlite::SqliteArguments<'q>>,
        val: &'q SqliteBindValue,
    ) -> sqlx::query::QueryAs<'q, Sqlite, T, sqlx::sqlite::SqliteArguments<'q>>
    where
        T: for<'r> FromRow<'r, SqliteRow>,
    {
        match val {
            SqliteBindValue::Int64(v) => q.bind(*v),
            SqliteBindValue::Int32(v) => q.bind(*v),
            SqliteBindValue::String(v) => q.bind(v.as_str()),
            SqliteBindValue::Bool(v) => q.bind(*v),
        }
    }

    /// Bind a SqliteBindValue to a raw query.
    fn bind_value_raw<'q>(
        q: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
        val: &'q SqliteBindValue,
    ) -> sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
        match val {
            SqliteBindValue::Int64(v) => q.bind(*v),
            SqliteBindValue::Int32(v) => q.bind(*v),
            SqliteBindValue::String(v) => q.bind(v.as_str()),
            SqliteBindValue::Bool(v) => q.bind(*v),
        }
    }

    // -----------------------------------------------------------------------
    // WHERE clause builders for each table's partial args
    // -----------------------------------------------------------------------

    fn build_users_where(args: &FindUsersArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.identity_key {
            wb.add_eq("identityKey");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.active_storage {
            wb.add_eq("activeStorage");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_certificates_where(args: &FindCertificatesArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.certificate_id {
            wb.add_eq("certificateId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.cert_type {
            wb.add_eq("type");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.serial_number {
            wb.add_eq("serialNumber");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.certifier {
            wb.add_eq("certifier");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.subject {
            wb.add_eq("subject");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.verifier {
            wb.add_eq("verifier");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.revocation_outpoint {
            wb.add_eq("revocationOutpoint");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.signature {
            wb.add_eq("signature");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_certificate_fields_where(
        args: &FindCertificateFieldsArgs,
    ) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.certificate_id {
            wb.add_eq("certificateId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.field_name {
            wb.add_eq("fieldName");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.field_value {
            wb.add_eq("fieldValue");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.master_key {
            wb.add_eq("masterKey");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_commissions_where(args: &FindCommissionsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.commission_id {
            wb.add_eq("commissionId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.transaction_id {
            wb.add_eq("transactionId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.satoshis {
            wb.add_eq("satoshis");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.key_offset {
            wb.add_eq("keyOffset");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_redeemed {
            wb.add_eq("isRedeemed");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_monitor_events_where(args: &FindMonitorEventsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.id {
            wb.add_eq("id");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.event {
            wb.add_eq("event");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_output_baskets_where(args: &FindOutputBasketsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.basket_id {
            wb.add_eq("basketId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.name {
            wb.add_eq("name");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_output_tag_maps_where(args: &FindOutputTagMapsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.output_tag_id {
            wb.add_eq("outputTagId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.output_id {
            wb.add_eq("outputId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_output_tags_where(args: &FindOutputTagsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.output_tag_id {
            wb.add_eq("outputTagId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.tag {
            wb.add_eq("tag");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_outputs_where(args: &FindOutputsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.output_id {
            wb.add_eq("outputId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.transaction_id {
            wb.add_eq("transactionId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.basket_id {
            wb.add_eq("basketId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.spendable {
            wb.add_eq("spendable");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.partial.change {
            wb.add_eq("change");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.partial.vout {
            wb.add_eq("vout");
            binds.push(SqliteBindValue::Int32(*v));
        }
        if let Some(v) = &args.partial.satoshis {
            wb.add_eq("satoshis");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.provided_by {
            wb.add_eq("providedBy");
            binds.push(SqliteBindValue::String(v.to_string()));
        }
        if let Some(v) = &args.partial.purpose {
            wb.add_eq("purpose");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.output_type {
            wb.add_eq("type");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.txid {
            wb.add_eq("txid");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.sender_identity_key {
            wb.add_eq("senderIdentityKey");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.spent_by {
            wb.add_eq("spentBy");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_proven_txs_where(args: &FindProvenTxsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.proven_tx_id {
            wb.add_eq("provenTxId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.txid {
            wb.add_eq("txid");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.height {
            wb.add_eq("height");
            binds.push(SqliteBindValue::Int32(*v));
        }
        if let Some(v) = &args.partial.block_hash {
            wb.add_eq("blockHash");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_proven_tx_reqs_where(args: &FindProvenTxReqsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.proven_tx_req_id {
            wb.add_eq("provenTxReqId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.proven_tx_id {
            wb.add_eq("provenTxId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        // Multi-status filter takes precedence over single partial.status
        if let Some(statuses) = &args.statuses {
            if !statuses.is_empty() {
                wb.add_in("status", statuses.len());
                for s in statuses {
                    binds.push(SqliteBindValue::String(s.to_string()));
                }
            }
        } else if let Some(v) = &args.partial.status {
            wb.add_eq("status");
            binds.push(SqliteBindValue::String(v.to_string()));
        }
        if let Some(v) = &args.partial.txid {
            wb.add_eq("txid");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.batch {
            wb.add_eq("batch");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.notified {
            wb.add_eq("notified");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_settings_where(args: &FindSettingsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.storage_identity_key {
            wb.add_eq("storageIdentityKey");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.storage_name {
            wb.add_eq("storageName");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.chain {
            wb.add_eq("chain");
            binds.push(SqliteBindValue::String(v.to_string()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_sync_states_where(args: &FindSyncStatesArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.sync_state_id {
            wb.add_eq("syncStateId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.storage_identity_key {
            wb.add_eq("storageIdentityKey");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.storage_name {
            wb.add_eq("storageName");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.status {
            wb.add_eq("status");
            binds.push(SqliteBindValue::String(v.to_string()));
        }
        if let Some(v) = &args.partial.init {
            wb.add_eq("init");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_transactions_where(args: &FindTransactionsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.transaction_id {
            wb.add_eq("transactionId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.proven_tx_id {
            wb.add_eq("provenTxId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.status {
            wb.add_eq("status");
            binds.push(SqliteBindValue::String(v.to_string()));
        }
        if let Some(v) = &args.partial.reference {
            wb.add_eq("reference");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_outgoing {
            wb.add_eq("isOutgoing");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.partial.txid {
            wb.add_eq("txid");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_tx_label_maps_where(args: &FindTxLabelMapsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.tx_label_id {
            wb.add_eq("txLabelId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.transaction_id {
            wb.add_eq("transactionId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    fn build_tx_labels_where(args: &FindTxLabelsArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        if let Some(v) = &args.partial.tx_label_id {
            wb.add_eq("txLabelId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.user_id {
            wb.add_eq("userId");
            binds.push(SqliteBindValue::Int64(*v));
        }
        if let Some(v) = &args.partial.label {
            wb.add_eq("label");
            binds.push(SqliteBindValue::String(v.clone()));
        }
        if let Some(v) = &args.partial.is_deleted {
            wb.add_eq("isDeleted");
            binds.push(SqliteBindValue::Bool(*v));
        }
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    // -----------------------------------------------------------------------
    // For-user WHERE builders
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    fn build_for_user_where(args: &FindForUserSincePagedArgs) -> (String, Vec<SqliteBindValue>) {
        let mut wb = WhereBuilder::new_sqlite();
        let mut binds = Vec::new();
        wb.add_eq("userId");
        binds.push(SqliteBindValue::Int64(args.user_id));
        if let Some(v) = &args.since {
            wb.add_gte("updated_at");
            binds.push(SqliteBindValue::String(
                v.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
        let mut sql = wb.build_where();
        if let Some(paged) = &args.paged {
            sql.push_str(&WhereBuilder::build_limit_offset(paged));
        }
        (sql, binds)
    }

    // -----------------------------------------------------------------------
    // StorageReader implementation
    // -----------------------------------------------------------------------

    #[async_trait]
    impl StorageReader for StorageSqlx<Sqlite> {
        async fn find_users(
            &self,
            args: &FindUsersArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<User>> {
            let (where_clause, binds) = build_users_where(args);
            let sql = format!("SELECT * FROM users{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_users(
            &self,
            args: &FindUsersArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_users_where(args);
            let sql = format!("SELECT COUNT(*) FROM users{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_certificates(
            &self,
            args: &FindCertificatesArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<Certificate>> {
            let (where_clause, binds) = build_certificates_where(args);
            let sql = format!("SELECT * FROM certificates{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_certificates(
            &self,
            args: &FindCertificatesArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_certificates_where(args);
            let sql = format!("SELECT COUNT(*) FROM certificates{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_certificate_fields(
            &self,
            args: &FindCertificateFieldsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<CertificateField>> {
            let (where_clause, binds) = build_certificate_fields_where(args);
            let sql = format!("SELECT * FROM certificate_fields{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_certificate_fields(
            &self,
            args: &FindCertificateFieldsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_certificate_fields_where(args);
            let sql = format!("SELECT COUNT(*) FROM certificate_fields{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_commissions(
            &self,
            args: &FindCommissionsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<Commission>> {
            let (where_clause, binds) = build_commissions_where(args);
            let sql = format!("SELECT * FROM commissions{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_commissions(
            &self,
            args: &FindCommissionsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_commissions_where(args);
            let sql = format!("SELECT COUNT(*) FROM commissions{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_monitor_events(
            &self,
            args: &FindMonitorEventsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<MonitorEvent>> {
            let (where_clause, binds) = build_monitor_events_where(args);
            let sql = format!("SELECT * FROM monitor_events{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_monitor_events(
            &self,
            args: &FindMonitorEventsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_monitor_events_where(args);
            let sql = format!("SELECT COUNT(*) FROM monitor_events{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_output_baskets(
            &self,
            args: &FindOutputBasketsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<OutputBasket>> {
            let (where_clause, binds) = build_output_baskets_where(args);
            let sql = format!("SELECT * FROM output_baskets{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_output_baskets(
            &self,
            args: &FindOutputBasketsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_output_baskets_where(args);
            let sql = format!("SELECT COUNT(*) FROM output_baskets{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_output_tag_maps(
            &self,
            args: &FindOutputTagMapsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<OutputTagMap>> {
            let (where_clause, binds) = build_output_tag_maps_where(args);
            let sql = format!("SELECT * FROM output_tags_map{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_output_tag_maps(
            &self,
            args: &FindOutputTagMapsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_output_tag_maps_where(args);
            let sql = format!("SELECT COUNT(*) FROM output_tags_map{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_output_tags(
            &self,
            args: &FindOutputTagsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<OutputTag>> {
            let (where_clause, binds) = build_output_tags_where(args);
            let sql = format!("SELECT * FROM output_tags{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_output_tags(
            &self,
            args: &FindOutputTagsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_output_tags_where(args);
            let sql = format!("SELECT COUNT(*) FROM output_tags{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_outputs(
            &self,
            args: &FindOutputsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<Output>> {
            let (where_clause, binds) = build_outputs_where(args);
            let sql = format!("SELECT * FROM outputs{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_outputs(
            &self,
            args: &FindOutputsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_outputs_where(args);
            let sql = format!("SELECT COUNT(*) FROM outputs{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_proven_txs(
            &self,
            args: &FindProvenTxsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<ProvenTx>> {
            let (where_clause, binds) = build_proven_txs_where(args);
            let sql = format!("SELECT * FROM proven_txs{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_proven_txs(
            &self,
            args: &FindProvenTxsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_proven_txs_where(args);
            let sql = format!("SELECT COUNT(*) FROM proven_txs{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_proven_tx_reqs(
            &self,
            args: &FindProvenTxReqsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<ProvenTxReq>> {
            let (where_clause, binds) = build_proven_tx_reqs_where(args);
            let sql = format!("SELECT * FROM proven_tx_reqs{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_proven_tx_reqs(
            &self,
            args: &FindProvenTxReqsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_proven_tx_reqs_where(args);
            let sql = format!("SELECT COUNT(*) FROM proven_tx_reqs{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_settings(
            &self,
            args: &FindSettingsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<Settings>> {
            let (where_clause, binds) = build_settings_where(args);
            let sql = format!("SELECT * FROM settings{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_settings(
            &self,
            args: &FindSettingsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_settings_where(args);
            let sql = format!("SELECT COUNT(*) FROM settings{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_sync_states(
            &self,
            args: &FindSyncStatesArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<SyncState>> {
            let (where_clause, binds) = build_sync_states_where(args);
            let sql = format!("SELECT * FROM sync_states{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_sync_states(
            &self,
            args: &FindSyncStatesArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_sync_states_where(args);
            let sql = format!("SELECT COUNT(*) FROM sync_states{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_transactions(
            &self,
            args: &FindTransactionsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<Transaction>> {
            let (where_clause, binds) = build_transactions_where(args);
            let sql = format!("SELECT * FROM transactions{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_transactions(
            &self,
            args: &FindTransactionsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_transactions_where(args);
            let sql = format!("SELECT COUNT(*) FROM transactions{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_tx_label_maps(
            &self,
            args: &FindTxLabelMapsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<TxLabelMap>> {
            let (where_clause, binds) = build_tx_label_maps_where(args);
            let sql = format!("SELECT * FROM tx_labels_map{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_tx_label_maps(
            &self,
            args: &FindTxLabelMapsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_tx_label_maps_where(args);
            let sql = format!("SELECT COUNT(*) FROM tx_labels_map{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        async fn find_tx_labels(
            &self,
            args: &FindTxLabelsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<TxLabel>> {
            let (where_clause, binds) = build_tx_labels_where(args);
            let sql = format!("SELECT * FROM tx_labels{}", where_clause);
            query_rows(self, &sql, binds, trx).await
        }

        async fn count_tx_labels(
            &self,
            args: &FindTxLabelsArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let (where_clause, binds) = build_tx_labels_where(args);
            let sql = format!("SELECT COUNT(*) FROM tx_labels{}", where_clause);
            query_count(self, &sql, binds, trx).await
        }

        // -------------------------------------------------------------------
        // For-user methods
        // -------------------------------------------------------------------

        async fn get_proven_txs_for_user(
            &self,
            args: &FindForUserSincePagedArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<ProvenTx>> {
            // Join through transactions to find proven_txs for a user
            let mut sql = String::from(
                "SELECT DISTINCT pt.* FROM proven_txs pt \
                 INNER JOIN transactions t ON t.provenTxId = pt.provenTxId \
                 WHERE t.userId = ?",
            );
            let mut binds = vec![SqliteBindValue::Int64(args.user_id)];
            if let Some(v) = &args.since {
                sql.push_str(" AND pt.updated_at >= ?");
                binds.push(SqliteBindValue::String(
                    v.format("%Y-%m-%d %H:%M:%S").to_string(),
                ));
            }
            if let Some(paged) = &args.paged {
                sql.push_str(&WhereBuilder::build_limit_offset(paged));
            }
            query_rows(self, &sql, binds, trx).await
        }

        async fn get_proven_tx_reqs_for_user(
            &self,
            args: &FindForUserSincePagedArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<ProvenTxReq>> {
            // Join through transactions to find proven_tx_reqs for a user
            let mut sql = String::from(
                "SELECT DISTINCT ptr.* FROM proven_tx_reqs ptr \
                 INNER JOIN transactions t ON t.txid = ptr.txid \
                 WHERE t.userId = ?",
            );
            let mut binds = vec![SqliteBindValue::Int64(args.user_id)];
            if let Some(v) = &args.since {
                sql.push_str(" AND ptr.updated_at >= ?");
                binds.push(SqliteBindValue::String(
                    v.format("%Y-%m-%d %H:%M:%S").to_string(),
                ));
            }
            if let Some(paged) = &args.paged {
                sql.push_str(&WhereBuilder::build_limit_offset(paged));
            }
            query_rows(self, &sql, binds, trx).await
        }

        async fn get_tx_label_maps_for_user(
            &self,
            args: &FindForUserSincePagedArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<TxLabelMap>> {
            // Join through tx_labels to find maps for a user
            let mut sql = String::from(
                "SELECT DISTINCT tlm.* FROM tx_labels_map tlm \
                 INNER JOIN tx_labels tl ON tl.txLabelId = tlm.txLabelId \
                 WHERE tl.userId = ?",
            );
            let mut binds = vec![SqliteBindValue::Int64(args.user_id)];
            if let Some(v) = &args.since {
                sql.push_str(" AND tlm.updated_at >= ?");
                binds.push(SqliteBindValue::String(
                    v.format("%Y-%m-%d %H:%M:%S").to_string(),
                ));
            }
            if let Some(paged) = &args.paged {
                sql.push_str(&WhereBuilder::build_limit_offset(paged));
            }
            query_rows(self, &sql, binds, trx).await
        }

        async fn get_output_tag_maps_for_user(
            &self,
            args: &FindForUserSincePagedArgs,
            trx: Option<&TrxToken>,
        ) -> WalletResult<Vec<OutputTagMap>> {
            // Join through output_tags to find maps for a user
            let mut sql = String::from(
                "SELECT DISTINCT otm.* FROM output_tags_map otm \
                 INNER JOIN output_tags ot ON ot.outputTagId = otm.outputTagId \
                 WHERE ot.userId = ?",
            );
            let mut binds = vec![SqliteBindValue::Int64(args.user_id)];
            if let Some(v) = &args.since {
                sql.push_str(" AND otm.updated_at >= ?");
                binds.push(SqliteBindValue::String(
                    v.format("%Y-%m-%d %H:%M:%S").to_string(),
                ));
            }
            if let Some(paged) = &args.paged {
                sql.push_str(&WhereBuilder::build_limit_offset(paged));
            }
            query_rows(self, &sql, binds, trx).await
        }
    }
}

// ---------------------------------------------------------------------------
// Macro-generated MySQL and PostgreSQL StorageReader implementations
// ---------------------------------------------------------------------------
//
// Design decision: Macro-based code generation (Plan Option A).
// sqlx's generic trait bounds make truly generic code difficult because
// each backend has its own Row/Arguments types. A macro generates nearly
// identical code for each backend, varying only the DB type, Row type,
// Arguments type, placeholder style, trx extraction function, and dialect.
// ---------------------------------------------------------------------------

/// Generates a complete StorageReader implementation for a non-SQLite backend.
///
/// Parameters:
///   $feature   -- cargo feature gate string (e.g., "mysql")
///   $mod_name  -- module name (e.g., mysql_impl)
///   $db        -- sqlx database type (e.g., sqlx::MySql)
///   $row       -- sqlx row type (e.g., sqlx::mysql::MySqlRow)
///   $args_ty   -- sqlx arguments type (e.g., sqlx::mysql::MySqlArguments)
///   $dialect   -- Dialect variant (e.g., Dialect::Mysql)
///   $extract   -- trx extraction function (e.g., StorageSqlx::<sqlx::MySql>::extract_mysql_trx)
///   $trx_name  -- human name for error messages (e.g., "MySQL")
macro_rules! impl_storage_reader_find {
    (
        feature = $feature:literal,
        mod_name = $mod_name:ident,
        db = $db:ty,
        row = $row:ty,
        args_ty = $args_ty:ty,
        dialect = $dialect:expr,
        extract_trx = $extract:path,
        trx_name = $trx_name:literal
    ) => {
        #[cfg(feature = $feature)]
        mod $mod_name {
            use async_trait::async_trait;
            use sqlx::{FromRow, Row};

            use crate::error::{WalletError, WalletResult};
            use crate::storage::find_args::*;
            use crate::storage::sqlx_impl::dialect::{Dialect, WhereBuilder};
            use crate::storage::sqlx_impl::StorageSqlx;
            use crate::storage::traits::reader::StorageReader;
            use crate::storage::TrxToken;
            use crate::tables::*;

            #[derive(Debug, Clone)]
            enum BindVal {
                Int64(i64),
                Int32(i32),
                String(String),
                Bool(bool),
            }

            fn bind_value<'q, T>(
                q: sqlx::query::QueryAs<'q, $db, T, $args_ty>,
                val: &'q BindVal,
            ) -> sqlx::query::QueryAs<'q, $db, T, $args_ty>
            where
                T: for<'r> FromRow<'r, $row>,
            {
                match val {
                    BindVal::Int64(v) => q.bind(*v),
                    BindVal::Int32(v) => q.bind(*v),
                    BindVal::String(v) => q.bind(v.as_str()),
                    BindVal::Bool(v) => q.bind(*v),
                }
            }

            fn bind_value_raw<'q>(
                q: sqlx::query::Query<'q, $db, $args_ty>,
                val: &'q BindVal,
            ) -> sqlx::query::Query<'q, $db, $args_ty> {
                match val {
                    BindVal::Int64(v) => q.bind(*v),
                    BindVal::Int32(v) => q.bind(*v),
                    BindVal::String(v) => q.bind(v.as_str()),
                    BindVal::Bool(v) => q.bind(*v),
                }
            }

            async fn query_rows<T: for<'r> FromRow<'r, $row> + Send + Unpin>(
                storage: &StorageSqlx<$db>,
                sql: &str,
                binds: Vec<BindVal>,
                trx: Option<&TrxToken>,
            ) -> WalletResult<Vec<T>> {
                if let Some(trx_token) = trx {
                    let inner = $extract(trx_token)?;
                    let mut guard = inner.lock().await;
                    let tx = guard.as_mut().ok_or_else(|| {
                        WalletError::Internal("Transaction already consumed".to_string())
                    })?;
                    let mut q = sqlx::query_as::<_, T>(sql);
                    for bind in &binds {
                        q = bind_value(q, bind);
                    }
                    let rows = q.fetch_all(&mut **tx).await?;
                    Ok(rows)
                } else {
                    let pool = storage.read_pool();
                    let mut q = sqlx::query_as::<_, T>(sql);
                    for bind in &binds {
                        q = bind_value(q, bind);
                    }
                    let rows = q.fetch_all(pool).await?;
                    Ok(rows)
                }
            }

            async fn query_count(
                storage: &StorageSqlx<$db>,
                sql: &str,
                binds: Vec<BindVal>,
                trx: Option<&TrxToken>,
            ) -> WalletResult<i64> {
                if let Some(trx_token) = trx {
                    let inner = $extract(trx_token)?;
                    let mut guard = inner.lock().await;
                    let tx = guard.as_mut().ok_or_else(|| {
                        WalletError::Internal("Transaction already consumed".to_string())
                    })?;
                    let mut q = sqlx::query(sql);
                    for bind in &binds {
                        q = bind_value_raw(q, bind);
                    }
                    let row = q.fetch_one(&mut **tx).await?;
                    let count: i64 = row.get(0);
                    Ok(count)
                } else {
                    let pool = storage.read_pool();
                    let mut q = sqlx::query(sql);
                    for bind in &binds {
                        q = bind_value_raw(q, bind);
                    }
                    let row = q.fetch_one(pool).await?;
                    let count: i64 = row.get(0);
                    Ok(count)
                }
            }

            // WHERE clause builders -- identical logic, just use dialect-aware WhereBuilder

            fn wb() -> WhereBuilder {
                WhereBuilder::new($dialect)
            }

            fn build_users_where(args: &FindUsersArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.identity_key {
                    w.add_eq("identityKey");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.active_storage {
                    w.add_eq("activeStorage");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_certificates_where(args: &FindCertificatesArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.certificate_id {
                    w.add_eq("certificateId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.cert_type {
                    w.add_eq("type");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.serial_number {
                    w.add_eq("serialNumber");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.certifier {
                    w.add_eq("certifier");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.subject {
                    w.add_eq("subject");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.verifier {
                    w.add_eq("verifier");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.revocation_outpoint {
                    w.add_eq("revocationOutpoint");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.signature {
                    w.add_eq("signature");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_certificate_fields_where(
                args: &FindCertificateFieldsArgs,
            ) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.certificate_id {
                    w.add_eq("certificateId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.field_name {
                    w.add_eq("fieldName");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.field_value {
                    w.add_eq("fieldValue");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.master_key {
                    w.add_eq("masterKey");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_commissions_where(args: &FindCommissionsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.commission_id {
                    w.add_eq("commissionId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.transaction_id {
                    w.add_eq("transactionId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.satoshis {
                    w.add_eq("satoshis");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.key_offset {
                    w.add_eq("keyOffset");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_redeemed {
                    w.add_eq("isRedeemed");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_monitor_events_where(args: &FindMonitorEventsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.id {
                    w.add_eq("id");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.event {
                    w.add_eq("event");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_output_baskets_where(args: &FindOutputBasketsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.basket_id {
                    w.add_eq("basketId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.name {
                    w.add_eq("name");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_output_tag_maps_where(args: &FindOutputTagMapsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.output_tag_id {
                    w.add_eq("outputTagId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.output_id {
                    w.add_eq("outputId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_output_tags_where(args: &FindOutputTagsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.output_tag_id {
                    w.add_eq("outputTagId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.tag {
                    w.add_eq("tag");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_outputs_where(args: &FindOutputsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.output_id {
                    w.add_eq("outputId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.transaction_id {
                    w.add_eq("transactionId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.basket_id {
                    w.add_eq("basketId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.spendable {
                    w.add_eq("spendable");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.partial.change {
                    w.add_eq("change");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.partial.vout {
                    w.add_eq("vout");
                    binds.push(BindVal::Int32(*v));
                }
                if let Some(v) = &args.partial.satoshis {
                    w.add_eq("satoshis");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.provided_by {
                    w.add_eq("providedBy");
                    binds.push(BindVal::String(v.to_string()));
                }
                if let Some(v) = &args.partial.purpose {
                    w.add_eq("purpose");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.output_type {
                    w.add_eq("type");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.txid {
                    w.add_eq("txid");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.sender_identity_key {
                    w.add_eq("senderIdentityKey");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.spent_by {
                    w.add_eq("spentBy");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_proven_txs_where(args: &FindProvenTxsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.proven_tx_id {
                    w.add_eq("provenTxId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.txid {
                    w.add_eq("txid");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.height {
                    w.add_eq("height");
                    binds.push(BindVal::Int32(*v));
                }
                if let Some(v) = &args.partial.block_hash {
                    w.add_eq("blockHash");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_proven_tx_reqs_where(args: &FindProvenTxReqsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.proven_tx_req_id {
                    w.add_eq("provenTxReqId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.proven_tx_id {
                    w.add_eq("provenTxId");
                    binds.push(BindVal::Int64(*v));
                }
                // Multi-status filter takes precedence over single partial.status
                if let Some(statuses) = &args.statuses {
                    if !statuses.is_empty() {
                        w.add_in("status", statuses.len());
                        for s in statuses {
                            binds.push(BindVal::String(s.to_string()));
                        }
                    }
                } else if let Some(v) = &args.partial.status {
                    w.add_eq("status");
                    binds.push(BindVal::String(v.to_string()));
                }
                if let Some(v) = &args.partial.txid {
                    w.add_eq("txid");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.batch {
                    w.add_eq("batch");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.notified {
                    w.add_eq("notified");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_settings_where(args: &FindSettingsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.storage_identity_key {
                    w.add_eq("storageIdentityKey");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.storage_name {
                    w.add_eq("storageName");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.chain {
                    w.add_eq("chain");
                    binds.push(BindVal::String(v.to_string()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_sync_states_where(args: &FindSyncStatesArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.sync_state_id {
                    w.add_eq("syncStateId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.storage_identity_key {
                    w.add_eq("storageIdentityKey");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.storage_name {
                    w.add_eq("storageName");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.status {
                    w.add_eq("status");
                    binds.push(BindVal::String(v.to_string()));
                }
                if let Some(v) = &args.partial.init {
                    w.add_eq("init");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_transactions_where(args: &FindTransactionsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.transaction_id {
                    w.add_eq("transactionId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.proven_tx_id {
                    w.add_eq("provenTxId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.status {
                    w.add_eq("status");
                    binds.push(BindVal::String(v.to_string()));
                }
                if let Some(v) = &args.partial.reference {
                    w.add_eq("reference");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_outgoing {
                    w.add_eq("isOutgoing");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.partial.txid {
                    w.add_eq("txid");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_tx_label_maps_where(args: &FindTxLabelMapsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.tx_label_id {
                    w.add_eq("txLabelId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.transaction_id {
                    w.add_eq("transactionId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            fn build_tx_labels_where(args: &FindTxLabelsArgs) -> (String, Vec<BindVal>) {
                let mut w = wb();
                let mut binds = Vec::new();
                if let Some(v) = &args.partial.tx_label_id {
                    w.add_eq("txLabelId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.user_id {
                    w.add_eq("userId");
                    binds.push(BindVal::Int64(*v));
                }
                if let Some(v) = &args.partial.label {
                    w.add_eq("label");
                    binds.push(BindVal::String(v.clone()));
                }
                if let Some(v) = &args.partial.is_deleted {
                    w.add_eq("isDeleted");
                    binds.push(BindVal::Bool(*v));
                }
                if let Some(v) = &args.since {
                    w.add_gte("updated_at");
                    binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                }
                let mut sql = w.build_where();
                if let Some(paged) = &args.paged {
                    sql.push_str(&WhereBuilder::build_limit_offset(paged));
                }
                (sql, binds)
            }

            // -- For-user helper placeholder --
            fn ph(index: usize) -> String {
                crate::storage::sqlx_impl::dialect::placeholder($dialect, index)
            }

            // StorageReader implementation
            #[async_trait]
            impl StorageReader for StorageSqlx<$db> {
                async fn find_users(
                    &self,
                    args: &FindUsersArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<User>> {
                    let (w, b) = build_users_where(args);
                    query_rows(self, &format!("SELECT * FROM users{}", w), b, trx).await
                }
                async fn count_users(
                    &self,
                    args: &FindUsersArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_users_where(args);
                    query_count(self, &format!("SELECT COUNT(*) FROM users{}", w), b, trx).await
                }
                async fn find_certificates(
                    &self,
                    args: &FindCertificatesArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<Certificate>> {
                    let (w, b) = build_certificates_where(args);
                    query_rows(self, &format!("SELECT * FROM certificates{}", w), b, trx).await
                }
                async fn count_certificates(
                    &self,
                    args: &FindCertificatesArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_certificates_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM certificates{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_certificate_fields(
                    &self,
                    args: &FindCertificateFieldsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<CertificateField>> {
                    let (w, b) = build_certificate_fields_where(args);
                    query_rows(
                        self,
                        &format!("SELECT * FROM certificate_fields{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn count_certificate_fields(
                    &self,
                    args: &FindCertificateFieldsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_certificate_fields_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM certificate_fields{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_commissions(
                    &self,
                    args: &FindCommissionsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<Commission>> {
                    let (w, b) = build_commissions_where(args);
                    query_rows(self, &format!("SELECT * FROM commissions{}", w), b, trx).await
                }
                async fn count_commissions(
                    &self,
                    args: &FindCommissionsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_commissions_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM commissions{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_monitor_events(
                    &self,
                    args: &FindMonitorEventsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<MonitorEvent>> {
                    let (w, b) = build_monitor_events_where(args);
                    query_rows(self, &format!("SELECT * FROM monitor_events{}", w), b, trx).await
                }
                async fn count_monitor_events(
                    &self,
                    args: &FindMonitorEventsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_monitor_events_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM monitor_events{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_output_baskets(
                    &self,
                    args: &FindOutputBasketsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<OutputBasket>> {
                    let (w, b) = build_output_baskets_where(args);
                    query_rows(self, &format!("SELECT * FROM output_baskets{}", w), b, trx).await
                }
                async fn count_output_baskets(
                    &self,
                    args: &FindOutputBasketsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_output_baskets_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM output_baskets{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_output_tag_maps(
                    &self,
                    args: &FindOutputTagMapsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<OutputTagMap>> {
                    let (w, b) = build_output_tag_maps_where(args);
                    query_rows(self, &format!("SELECT * FROM output_tags_map{}", w), b, trx).await
                }
                async fn count_output_tag_maps(
                    &self,
                    args: &FindOutputTagMapsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_output_tag_maps_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM output_tags_map{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_output_tags(
                    &self,
                    args: &FindOutputTagsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<OutputTag>> {
                    let (w, b) = build_output_tags_where(args);
                    query_rows(self, &format!("SELECT * FROM output_tags{}", w), b, trx).await
                }
                async fn count_output_tags(
                    &self,
                    args: &FindOutputTagsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_output_tags_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM output_tags{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_outputs(
                    &self,
                    args: &FindOutputsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<Output>> {
                    let (w, b) = build_outputs_where(args);
                    query_rows(self, &format!("SELECT * FROM outputs{}", w), b, trx).await
                }
                async fn count_outputs(
                    &self,
                    args: &FindOutputsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_outputs_where(args);
                    query_count(self, &format!("SELECT COUNT(*) FROM outputs{}", w), b, trx).await
                }
                async fn find_proven_txs(
                    &self,
                    args: &FindProvenTxsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<ProvenTx>> {
                    let (w, b) = build_proven_txs_where(args);
                    query_rows(self, &format!("SELECT * FROM proven_txs{}", w), b, trx).await
                }
                async fn count_proven_txs(
                    &self,
                    args: &FindProvenTxsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_proven_txs_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM proven_txs{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_proven_tx_reqs(
                    &self,
                    args: &FindProvenTxReqsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<ProvenTxReq>> {
                    let (w, b) = build_proven_tx_reqs_where(args);
                    query_rows(self, &format!("SELECT * FROM proven_tx_reqs{}", w), b, trx).await
                }
                async fn count_proven_tx_reqs(
                    &self,
                    args: &FindProvenTxReqsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_proven_tx_reqs_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM proven_tx_reqs{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_settings(
                    &self,
                    args: &FindSettingsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<Settings>> {
                    let (w, b) = build_settings_where(args);
                    query_rows(self, &format!("SELECT * FROM settings{}", w), b, trx).await
                }
                async fn count_settings(
                    &self,
                    args: &FindSettingsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_settings_where(args);
                    query_count(self, &format!("SELECT COUNT(*) FROM settings{}", w), b, trx).await
                }
                async fn find_sync_states(
                    &self,
                    args: &FindSyncStatesArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<SyncState>> {
                    let (w, b) = build_sync_states_where(args);
                    query_rows(self, &format!("SELECT * FROM sync_states{}", w), b, trx).await
                }
                async fn count_sync_states(
                    &self,
                    args: &FindSyncStatesArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_sync_states_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM sync_states{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_transactions(
                    &self,
                    args: &FindTransactionsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<Transaction>> {
                    let (w, b) = build_transactions_where(args);
                    query_rows(self, &format!("SELECT * FROM transactions{}", w), b, trx).await
                }
                async fn count_transactions(
                    &self,
                    args: &FindTransactionsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_transactions_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM transactions{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_tx_label_maps(
                    &self,
                    args: &FindTxLabelMapsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<TxLabelMap>> {
                    let (w, b) = build_tx_label_maps_where(args);
                    query_rows(self, &format!("SELECT * FROM tx_labels_map{}", w), b, trx).await
                }
                async fn count_tx_label_maps(
                    &self,
                    args: &FindTxLabelMapsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_tx_label_maps_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM tx_labels_map{}", w),
                        b,
                        trx,
                    )
                    .await
                }
                async fn find_tx_labels(
                    &self,
                    args: &FindTxLabelsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<TxLabel>> {
                    let (w, b) = build_tx_labels_where(args);
                    query_rows(self, &format!("SELECT * FROM tx_labels{}", w), b, trx).await
                }
                async fn count_tx_labels(
                    &self,
                    args: &FindTxLabelsArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<i64> {
                    let (w, b) = build_tx_labels_where(args);
                    query_count(
                        self,
                        &format!("SELECT COUNT(*) FROM tx_labels{}", w),
                        b,
                        trx,
                    )
                    .await
                }

                // For-user methods use JOIN queries with dialect-specific placeholders
                async fn get_proven_txs_for_user(
                    &self,
                    args: &FindForUserSincePagedArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<ProvenTx>> {
                    let mut sql = format!(
                        "SELECT DISTINCT pt.* FROM proven_txs pt \
                         INNER JOIN transactions t ON t.provenTxId = pt.provenTxId \
                         WHERE t.userId = {}",
                        ph(1)
                    );
                    let mut binds = vec![BindVal::Int64(args.user_id)];
                    if let Some(v) = &args.since {
                        sql.push_str(&format!(" AND pt.updated_at >= {}", ph(2)));
                        binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                    }
                    if let Some(paged) = &args.paged {
                        sql.push_str(&WhereBuilder::build_limit_offset(paged));
                    }
                    query_rows(self, &sql, binds, trx).await
                }

                async fn get_proven_tx_reqs_for_user(
                    &self,
                    args: &FindForUserSincePagedArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<ProvenTxReq>> {
                    let mut sql = format!(
                        "SELECT DISTINCT ptr.* FROM proven_tx_reqs ptr \
                         INNER JOIN transactions t ON t.txid = ptr.txid \
                         WHERE t.userId = {}",
                        ph(1)
                    );
                    let mut binds = vec![BindVal::Int64(args.user_id)];
                    if let Some(v) = &args.since {
                        sql.push_str(&format!(" AND ptr.updated_at >= {}", ph(2)));
                        binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                    }
                    if let Some(paged) = &args.paged {
                        sql.push_str(&WhereBuilder::build_limit_offset(paged));
                    }
                    query_rows(self, &sql, binds, trx).await
                }

                async fn get_tx_label_maps_for_user(
                    &self,
                    args: &FindForUserSincePagedArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<TxLabelMap>> {
                    let mut sql = format!(
                        "SELECT DISTINCT tlm.* FROM tx_labels_map tlm \
                         INNER JOIN tx_labels tl ON tl.txLabelId = tlm.txLabelId \
                         WHERE tl.userId = {}",
                        ph(1)
                    );
                    let mut binds = vec![BindVal::Int64(args.user_id)];
                    if let Some(v) = &args.since {
                        sql.push_str(&format!(" AND tlm.updated_at >= {}", ph(2)));
                        binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                    }
                    if let Some(paged) = &args.paged {
                        sql.push_str(&WhereBuilder::build_limit_offset(paged));
                    }
                    query_rows(self, &sql, binds, trx).await
                }

                async fn get_output_tag_maps_for_user(
                    &self,
                    args: &FindForUserSincePagedArgs,
                    trx: Option<&TrxToken>,
                ) -> WalletResult<Vec<OutputTagMap>> {
                    let mut sql = format!(
                        "SELECT DISTINCT otm.* FROM output_tags_map otm \
                         INNER JOIN output_tags ot ON ot.outputTagId = otm.outputTagId \
                         WHERE ot.userId = {}",
                        ph(1)
                    );
                    let mut binds = vec![BindVal::Int64(args.user_id)];
                    if let Some(v) = &args.since {
                        sql.push_str(&format!(" AND otm.updated_at >= {}", ph(2)));
                        binds.push(BindVal::String(v.format("%Y-%m-%d %H:%M:%S").to_string()));
                    }
                    if let Some(paged) = &args.paged {
                        sql.push_str(&WhereBuilder::build_limit_offset(paged));
                    }
                    query_rows(self, &sql, binds, trx).await
                }
            }
        }
    };
}

impl_storage_reader_find! {
    feature = "mysql",
    mod_name = mysql_find_impl,
    db = sqlx::MySql,
    row = sqlx::mysql::MySqlRow,
    args_ty = sqlx::mysql::MySqlArguments,
    dialect = Dialect::Mysql,
    extract_trx = StorageSqlx::<sqlx::MySql>::extract_mysql_trx,
    trx_name = "MySQL"
}

impl_storage_reader_find! {
    feature = "postgres",
    mod_name = postgres_find_impl,
    db = sqlx::Postgres,
    row = sqlx::postgres::PgRow,
    args_ty = sqlx::postgres::PgArguments,
    dialect = Dialect::Postgres,
    extract_trx = StorageSqlx::<sqlx::Postgres>::extract_pg_trx,
    trx_name = "PostgreSQL"
}
