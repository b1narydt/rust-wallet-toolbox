//! Insert implementations for StorageSqlx backends.
//!
//! Provides INSERT operations for all 16 table types, plus transaction
//! management (begin/commit/rollback) as part of StorageReaderWriter.
//! MySQL and PostgreSQL implementations are generated via macros.

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use sqlx::Sqlite;

    use crate::error::{WalletError, WalletResult};
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::TrxToken;
    use crate::tables::*;

    /// Helper macro to reduce boilerplate for insert implementations.
    /// Executes an INSERT statement and returns last_insert_rowid.
    macro_rules! exec_insert {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                let result = sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&mut **tx)
                    .await?;
                Ok(result.last_insert_rowid())
            } else {
                let result = sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&$storage.write_pool)
                    .await?;
                Ok(result.last_insert_rowid())
            }
        }};
    }

    /// Helper for inserts that do not return a rowid (composite PK tables).
    macro_rules! exec_insert_no_id {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&mut **tx)
                    .await?;
                Ok(())
            } else {
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&$storage.write_pool)
                    .await?;
                Ok(())
            }
        }};
    }

    impl StorageSqlx<Sqlite> {
        pub(crate) async fn insert_user_impl(
            &self,
            user: &User,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO users (created_at, updated_at, identityKey, activeStorage) \
                       VALUES (?, ?, ?, ?)";
            let created = user.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = user.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                &user.identity_key,
                &user.active_storage,
            )
        }

        pub(crate) async fn insert_certificate_impl(
            &self,
            cert: &Certificate,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO certificates (created_at, updated_at, userId, `type`, \
                       serialNumber, certifier, subject, verifier, revocationOutpoint, \
                       signature, isDeleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = cert.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = cert.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                cert.user_id,
                &cert.cert_type,
                &cert.serial_number,
                &cert.certifier,
                &cert.subject,
                &cert.verifier,
                &cert.revocation_outpoint,
                &cert.signature,
                cert.is_deleted,
            )
        }

        pub(crate) async fn insert_certificate_field_impl(
            &self,
            field: &CertificateField,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO certificate_fields (created_at, updated_at, userId, \
                       certificateId, fieldName, fieldValue, masterKey) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = field.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = field.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id!(
                self,
                sql,
                trx,
                created,
                updated,
                field.user_id,
                field.certificate_id,
                &field.field_name,
                &field.field_value,
                &field.master_key,
            )
        }

        pub(crate) async fn insert_commission_impl(
            &self,
            commission: &Commission,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO commissions (created_at, updated_at, userId, transactionId, \
                       satoshis, keyOffset, isRedeemed, lockingScript) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
            let created = commission
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let updated = commission
                .updated_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                commission.user_id,
                commission.transaction_id,
                commission.satoshis,
                &commission.key_offset,
                commission.is_redeemed,
                &commission.locking_script,
            )
        }

        pub(crate) async fn insert_monitor_event_impl(
            &self,
            event: &MonitorEvent,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO monitor_events (created_at, updated_at, event, details) \
                       VALUES (?, ?, ?, ?)";
            let created = event.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = event.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                &event.event,
                &event.details,
            )
        }

        pub(crate) async fn insert_output_basket_impl(
            &self,
            basket: &OutputBasket,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO output_baskets (created_at, updated_at, userId, name, \
                       numberOfDesiredUTXOs, minimumDesiredUTXOValue, isDeleted) \
                       VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = basket.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = basket.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                basket.user_id,
                &basket.name,
                basket.number_of_desired_utxos,
                basket.minimum_desired_utxo_value,
                basket.is_deleted,
            )
        }

        pub(crate) async fn insert_output_tag_impl(
            &self,
            tag: &OutputTag,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO output_tags (created_at, updated_at, userId, tag, isDeleted) \
                       VALUES (?, ?, ?, ?, ?)";
            let created = tag.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = tag.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                tag.user_id,
                &tag.tag,
                tag.is_deleted,
            )
        }

        pub(crate) async fn insert_output_tag_map_impl(
            &self,
            tag_map: &OutputTagMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql =
                "INSERT INTO output_tags_map (created_at, updated_at, outputTagId, outputId, \
                       isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = tag_map.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = tag_map.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id!(
                self,
                sql,
                trx,
                created,
                updated,
                tag_map.output_tag_id,
                tag_map.output_id,
                tag_map.is_deleted,
            )
        }

        pub(crate) async fn insert_output_impl(
            &self,
            output: &Output,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO outputs (created_at, updated_at, userId, transactionId, \
                       basketId, spendable, `change`, vout, satoshis, providedBy, purpose, \
                       `type`, outputDescription, txid, senderIdentityKey, derivationPrefix, \
                       derivationSuffix, customInstructions, spentBy, sequenceNumber, \
                       spendingDescription, scriptLength, scriptOffset, lockingScript) \
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = output.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = output.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let provided_by_str = output.provided_by.to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                output.user_id,
                output.transaction_id,
                output.basket_id,
                output.spendable,
                output.change,
                output.vout,
                output.satoshis,
                &provided_by_str,
                &output.purpose,
                &output.output_type,
                &output.output_description,
                &output.txid,
                &output.sender_identity_key,
                &output.derivation_prefix,
                &output.derivation_suffix,
                &output.custom_instructions,
                output.spent_by,
                output.sequence_number,
                &output.spending_description,
                output.script_length,
                output.script_offset,
                &output.locking_script,
            )
        }

        pub(crate) async fn insert_proven_tx_impl(
            &self,
            ptx: &ProvenTx,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO proven_txs (created_at, updated_at, txid, height, \
                       \"index\", merklePath, rawTx, blockHash, merkleRoot) \
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = ptx.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = ptx.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                &ptx.txid,
                ptx.height,
                ptx.index,
                &ptx.merkle_path,
                &ptx.raw_tx,
                &ptx.block_hash,
                &ptx.merkle_root,
            )
        }

        pub(crate) async fn insert_proven_tx_req_impl(
            &self,
            req: &ProvenTxReq,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO proven_tx_reqs (created_at, updated_at, provenTxId, status, \
                       attempts, notified, txid, batch, history, notify, rawTx, inputBEEF) \
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = req.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = req.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let status_str = req.status.to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                req.proven_tx_id,
                &status_str,
                req.attempts,
                req.notified,
                &req.txid,
                &req.batch,
                &req.history,
                &req.notify,
                &req.raw_tx,
                &req.input_beef,
            )
        }

        pub(crate) async fn insert_transaction_impl(
            &self,
            transaction: &crate::tables::Transaction,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO transactions (created_at, updated_at, userId, provenTxId, \
                       status, reference, isOutgoing, satoshis, description, version, lockTime, \
                       txid, inputBEEF, rawTx) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = transaction
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let updated = transaction
                .updated_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            let status_str = transaction.status.to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                transaction.user_id,
                transaction.proven_tx_id,
                &status_str,
                &transaction.reference,
                transaction.is_outgoing,
                transaction.satoshis,
                &transaction.description,
                transaction.version,
                transaction.lock_time,
                &transaction.txid,
                &transaction.input_beef,
                &transaction.raw_tx,
            )
        }

        pub(crate) async fn insert_tx_label_impl(
            &self,
            label: &TxLabel,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO tx_labels (created_at, updated_at, userId, label, isDeleted) \
                       VALUES (?, ?, ?, ?, ?)";
            let created = label.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = label.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                label.user_id,
                &label.label,
                label.is_deleted,
            )
        }

        pub(crate) async fn insert_tx_label_map_impl(
            &self,
            label_map: &TxLabelMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO tx_labels_map (created_at, updated_at, txLabelId, \
                       transactionId, isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = label_map.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = label_map.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id!(
                self,
                sql,
                trx,
                created,
                updated,
                label_map.tx_label_id,
                label_map.transaction_id,
                label_map.is_deleted,
            )
        }

        pub(crate) async fn insert_sync_state_impl(
            &self,
            ss: &SyncState,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO sync_states (created_at, updated_at, userId, \
                       storageIdentityKey, storageName, status, init, refNum, syncMap, \
                       \"when\", satoshis, errorLocal, errorOther) \
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = ss.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = ss.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let status_str = ss.status.to_string();
            let when_str = ss.when.map(|w| w.format("%Y-%m-%d %H:%M:%S").to_string());
            exec_insert!(
                self,
                sql,
                trx,
                created,
                updated,
                ss.user_id,
                &ss.storage_identity_key,
                &ss.storage_name,
                &status_str,
                ss.init,
                &ss.ref_num,
                &ss.sync_map,
                &when_str,
                ss.satoshis,
                &ss.error_local,
                &ss.error_other,
            )
        }

        pub(crate) async fn insert_settings_impl(
            &self,
            settings: &Settings,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO settings (created_at, updated_at, storageIdentityKey, \
                       storageName, chain, dbtype, maxOutputScript) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = settings.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = settings.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let chain_str = settings.chain.to_string();
            exec_insert_no_id!(
                self,
                sql,
                trx,
                created,
                updated,
                &settings.storage_identity_key,
                &settings.storage_name,
                &chain_str,
                &settings.dbtype,
                settings.max_output_script,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// MySQL insert implementation (macro-generated)
// ---------------------------------------------------------------------------
// MySQL uses ? placeholders (same as SQLite) and last_insert_id() returning u64.

#[cfg(feature = "mysql")]
mod mysql_insert_impl {
    use sqlx::MySql;

    use crate::error::{WalletError, WalletResult};
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::TrxToken;
    use crate::tables::*;

    macro_rules! exec_insert_mysql {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<MySql>::extract_mysql_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                let result = sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&mut **tx)
                    .await?;
                Ok(result.last_insert_id() as i64)
            } else {
                let result = sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&$storage.write_pool)
                    .await?;
                Ok(result.last_insert_id() as i64)
            }
        }};
    }

    macro_rules! exec_insert_no_id_mysql {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<MySql>::extract_mysql_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&mut **tx)
                    .await?;
                Ok(())
            } else {
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&$storage.write_pool)
                    .await?;
                Ok(())
            }
        }};
    }

    impl StorageSqlx<MySql> {
        pub(crate) async fn insert_user_impl(
            &self,
            user: &User,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO users (created_at, updated_at, identityKey, activeStorage) VALUES (?, ?, ?, ?)";
            let created = user.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = user.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                &user.identity_key,
                &user.active_storage
            )
        }

        pub(crate) async fn insert_certificate_impl(
            &self,
            cert: &Certificate,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO certificates (created_at, updated_at, userId, `type`, serialNumber, certifier, subject, verifier, revocationOutpoint, signature, isDeleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = cert.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = cert.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                cert.user_id,
                &cert.cert_type,
                &cert.serial_number,
                &cert.certifier,
                &cert.subject,
                &cert.verifier,
                &cert.revocation_outpoint,
                &cert.signature,
                cert.is_deleted
            )
        }

        pub(crate) async fn insert_certificate_field_impl(
            &self,
            field: &CertificateField,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO certificate_fields (created_at, updated_at, userId, certificateId, fieldName, fieldValue, masterKey) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = field.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = field.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                field.user_id,
                field.certificate_id,
                &field.field_name,
                &field.field_value,
                &field.master_key
            )
        }

        pub(crate) async fn insert_commission_impl(
            &self,
            c: &Commission,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO commissions (created_at, updated_at, userId, transactionId, satoshis, keyOffset, isRedeemed, lockingScript) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
            let created = c.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = c.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                c.user_id,
                c.transaction_id,
                c.satoshis,
                &c.key_offset,
                c.is_redeemed,
                &c.locking_script
            )
        }

        pub(crate) async fn insert_monitor_event_impl(
            &self,
            e: &MonitorEvent,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO monitor_events (created_at, updated_at, event, details) VALUES (?, ?, ?, ?)";
            let created = e.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = e.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(self, sql, trx, created, updated, &e.event, &e.details)
        }

        pub(crate) async fn insert_output_basket_impl(
            &self,
            b: &OutputBasket,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO output_baskets (created_at, updated_at, userId, name, numberOfDesiredUTXOs, minimumDesiredUTXOValue, isDeleted) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = b.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = b.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                b.user_id,
                &b.name,
                b.number_of_desired_utxos,
                b.minimum_desired_utxo_value,
                b.is_deleted
            )
        }

        pub(crate) async fn insert_output_tag_impl(
            &self,
            t: &OutputTag,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO output_tags (created_at, updated_at, userId, tag, isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = t.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = t.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                t.user_id,
                &t.tag,
                t.is_deleted
            )
        }

        pub(crate) async fn insert_output_tag_map_impl(
            &self,
            m: &OutputTagMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO output_tags_map (created_at, updated_at, outputTagId, outputId, isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = m.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = m.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                m.output_tag_id,
                m.output_id,
                m.is_deleted
            )
        }

        pub(crate) async fn insert_output_impl(
            &self,
            o: &Output,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO outputs (created_at, updated_at, userId, transactionId, basketId, spendable, `change`, vout, satoshis, providedBy, purpose, `type`, outputDescription, txid, senderIdentityKey, derivationPrefix, derivationSuffix, customInstructions, spentBy, sequenceNumber, spendingDescription, scriptLength, scriptOffset, lockingScript) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = o.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = o.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let provided_by_str = o.provided_by.to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                o.user_id,
                o.transaction_id,
                o.basket_id,
                o.spendable,
                o.change,
                o.vout,
                o.satoshis,
                &provided_by_str,
                &o.purpose,
                &o.output_type,
                &o.output_description,
                &o.txid,
                &o.sender_identity_key,
                &o.derivation_prefix,
                &o.derivation_suffix,
                &o.custom_instructions,
                o.spent_by,
                o.sequence_number,
                &o.spending_description,
                o.script_length,
                o.script_offset,
                &o.locking_script
            )
        }

        pub(crate) async fn insert_proven_tx_impl(
            &self,
            p: &ProvenTx,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO proven_txs (created_at, updated_at, txid, height, `index`, merklePath, rawTx, blockHash, merkleRoot) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = p.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = p.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                &p.txid,
                p.height,
                p.index,
                &p.merkle_path,
                &p.raw_tx,
                &p.block_hash,
                &p.merkle_root
            )
        }

        pub(crate) async fn insert_proven_tx_req_impl(
            &self,
            r: &ProvenTxReq,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO proven_tx_reqs (created_at, updated_at, provenTxId, status, attempts, notified, txid, batch, history, notify, rawTx, inputBEEF) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = r.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = r.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let status_str = r.status.to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                r.proven_tx_id,
                &status_str,
                r.attempts,
                r.notified,
                &r.txid,
                &r.batch,
                &r.history,
                &r.notify,
                &r.raw_tx,
                &r.input_beef
            )
        }

        pub(crate) async fn insert_transaction_impl(
            &self,
            t: &crate::tables::Transaction,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO transactions (created_at, updated_at, userId, provenTxId, status, reference, isOutgoing, satoshis, description, version, lockTime, txid, inputBEEF, rawTx) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = t.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = t.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let status_str = t.status.to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                t.user_id,
                t.proven_tx_id,
                &status_str,
                &t.reference,
                t.is_outgoing,
                t.satoshis,
                &t.description,
                t.version,
                t.lock_time,
                &t.txid,
                &t.input_beef,
                &t.raw_tx
            )
        }

        pub(crate) async fn insert_tx_label_impl(
            &self,
            l: &TxLabel,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO tx_labels (created_at, updated_at, userId, label, isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = l.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = l.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                l.user_id,
                &l.label,
                l.is_deleted
            )
        }

        pub(crate) async fn insert_tx_label_map_impl(
            &self,
            m: &TxLabelMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO tx_labels_map (created_at, updated_at, txLabelId, transactionId, isDeleted) VALUES (?, ?, ?, ?, ?)";
            let created = m.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = m.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            exec_insert_no_id_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                m.tx_label_id,
                m.transaction_id,
                m.is_deleted
            )
        }

        pub(crate) async fn insert_sync_state_impl(
            &self,
            ss: &SyncState,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = "INSERT INTO sync_states (created_at, updated_at, userId, storageIdentityKey, storageName, status, init, refNum, syncMap, `when`, satoshis, errorLocal, errorOther) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
            let created = ss.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = ss.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let status_str = ss.status.to_string();
            let when_str = ss.when.map(|w| w.format("%Y-%m-%d %H:%M:%S").to_string());
            exec_insert_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                ss.user_id,
                &ss.storage_identity_key,
                &ss.storage_name,
                &status_str,
                ss.init,
                &ss.ref_num,
                &ss.sync_map,
                &when_str,
                ss.satoshis,
                &ss.error_local,
                &ss.error_other
            )
        }

        pub(crate) async fn insert_settings_impl(
            &self,
            s: &Settings,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = "INSERT INTO settings (created_at, updated_at, storageIdentityKey, storageName, chain, dbtype, maxOutputScript) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let created = s.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let updated = s.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
            let chain_str = s.chain.to_string();
            exec_insert_no_id_mysql!(
                self,
                sql,
                trx,
                created,
                updated,
                &s.storage_identity_key,
                &s.storage_name,
                &chain_str,
                &s.dbtype,
                s.max_output_script
            )
        }
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL insert implementation
// ---------------------------------------------------------------------------
// PostgreSQL uses $N placeholders and RETURNING for auto-increment IDs.
// Uses sqlx::Row::get to extract the returned ID.

#[cfg(feature = "postgres")]
mod postgres_insert_impl {
    use sqlx::{Postgres, Row};

    use crate::error::{WalletError, WalletResult};
    use crate::storage::sqlx_impl::dialect::Dialect;
    use crate::storage::sqlx_impl::StorageSqlx;
    use crate::storage::TrxToken;
    use crate::tables::*;

    fn qc(col: &str) -> String {
        Dialect::Postgres.quote_column(col)
    }

    macro_rules! exec_insert_pg {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<Postgres>::extract_pg_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                let row = sqlx::query($sql)
                    $( .bind($bind) )*
                    .fetch_one(&mut **tx)
                    .await?;
                let id: i64 = row.get(0);
                Ok(id)
            } else {
                let row = sqlx::query($sql)
                    $( .bind($bind) )*
                    .fetch_one(&$storage.write_pool)
                    .await?;
                let id: i64 = row.get(0);
                Ok(id)
            }
        }};
    }

    macro_rules! exec_insert_no_id_pg {
        ($storage:expr, $sql:expr, $trx:expr, $( $bind:expr ),* $(,)?) => {{
            if let Some(trx_token) = $trx {
                let inner = StorageSqlx::<Postgres>::extract_pg_trx(trx_token)?;
                let mut guard = inner.lock().await;
                let tx = guard.as_mut().ok_or_else(|| {
                    WalletError::Internal("Transaction already consumed".to_string())
                })?;
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&mut **tx)
                    .await?;
                Ok(())
            } else {
                sqlx::query($sql)
                    $( .bind($bind) )*
                    .execute(&$storage.write_pool)
                    .await?;
                Ok(())
            }
        }};
    }

    impl StorageSqlx<Postgres> {
        pub(crate) async fn insert_user_impl(
            &self,
            user: &User,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO users ({}, {}, {}, {}) VALUES ($1, $2, $3, $4) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("identityKey"),
                qc("activeStorage"),
                qc("userId")
            );
            let created = user.created_at;
            let updated = user.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                &user.identity_key,
                &user.active_storage
            )
        }

        pub(crate) async fn insert_certificate_impl(
            &self,
            cert: &Certificate,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO certificates ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("type"),
                qc("serialNumber"),
                qc("certifier"),
                qc("subject"),
                qc("verifier"),
                qc("revocationOutpoint"),
                qc("signature"),
                qc("isDeleted"),
                qc("certificateId")
            );
            let created = cert.created_at;
            let updated = cert.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                cert.user_id,
                &cert.cert_type,
                &cert.serial_number,
                &cert.certifier,
                &cert.subject,
                &cert.verifier,
                &cert.revocation_outpoint,
                &cert.signature,
                cert.is_deleted
            )
        }

        pub(crate) async fn insert_certificate_field_impl(
            &self,
            field: &CertificateField,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = format!(
                "INSERT INTO certificate_fields ({}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("certificateId"),
                qc("fieldName"),
                qc("fieldValue"),
                qc("masterKey")
            );
            let created = field.created_at;
            let updated = field.updated_at;
            exec_insert_no_id_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                field.user_id,
                field.certificate_id,
                &field.field_name,
                &field.field_value,
                &field.master_key
            )
        }

        pub(crate) async fn insert_commission_impl(
            &self,
            c: &Commission,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO commissions ({}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("transactionId"),
                qc("satoshis"),
                qc("keyOffset"),
                qc("isRedeemed"),
                qc("lockingScript"),
                qc("commissionId")
            );
            let created = c.created_at;
            let updated = c.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                c.user_id,
                c.transaction_id,
                c.satoshis,
                &c.key_offset,
                c.is_redeemed,
                &c.locking_script
            )
        }

        pub(crate) async fn insert_monitor_event_impl(
            &self,
            e: &MonitorEvent,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO monitor_events ({}, {}, {}, {}) VALUES ($1, $2, $3, $4) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("event"),
                qc("details"),
                qc("id")
            );
            let created = e.created_at;
            let updated = e.updated_at;
            exec_insert_pg!(self, &sql, trx, created, updated, &e.event, &e.details)
        }

        pub(crate) async fn insert_output_basket_impl(
            &self,
            b: &OutputBasket,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO output_baskets ({}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("name"),
                qc("numberOfDesiredUTXOs"),
                qc("minimumDesiredUTXOValue"),
                qc("isDeleted"),
                qc("basketId")
            );
            let created = b.created_at;
            let updated = b.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                b.user_id,
                &b.name,
                b.number_of_desired_utxos,
                b.minimum_desired_utxo_value,
                b.is_deleted
            )
        }

        pub(crate) async fn insert_output_tag_impl(
            &self,
            t: &OutputTag,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO output_tags ({}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("tag"),
                qc("isDeleted"),
                qc("outputTagId")
            );
            let created = t.created_at;
            let updated = t.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                t.user_id,
                &t.tag,
                t.is_deleted
            )
        }

        pub(crate) async fn insert_output_tag_map_impl(
            &self,
            m: &OutputTagMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = format!(
                "INSERT INTO output_tags_map ({}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5)",
                qc("created_at"),
                qc("updated_at"),
                qc("outputTagId"),
                qc("outputId"),
                qc("isDeleted")
            );
            let created = m.created_at;
            let updated = m.updated_at;
            exec_insert_no_id_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                m.output_tag_id,
                m.output_id,
                m.is_deleted
            )
        }

        pub(crate) async fn insert_output_impl(
            &self,
            o: &Output,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO outputs ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("transactionId"),
                qc("basketId"),
                qc("spendable"),
                qc("change"),
                qc("vout"),
                qc("satoshis"),
                qc("providedBy"),
                qc("purpose"),
                qc("type"),
                qc("outputDescription"),
                qc("txid"),
                qc("senderIdentityKey"),
                qc("derivationPrefix"),
                qc("derivationSuffix"),
                qc("customInstructions"),
                qc("spentBy"),
                qc("sequenceNumber"),
                qc("spendingDescription"),
                qc("scriptLength"),
                qc("scriptOffset"),
                qc("lockingScript"),
                qc("outputId")
            );
            let created = o.created_at;
            let updated = o.updated_at;
            let provided_by_str = o.provided_by.to_string();
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                o.user_id,
                o.transaction_id,
                o.basket_id,
                o.spendable,
                o.change,
                o.vout,
                o.satoshis,
                &provided_by_str,
                &o.purpose,
                &o.output_type,
                &o.output_description,
                &o.txid,
                &o.sender_identity_key,
                &o.derivation_prefix,
                &o.derivation_suffix,
                &o.custom_instructions,
                o.spent_by,
                o.sequence_number,
                &o.spending_description,
                o.script_length,
                o.script_offset,
                &o.locking_script
            )
        }

        pub(crate) async fn insert_proven_tx_impl(
            &self,
            p: &ProvenTx,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO proven_txs ({}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("txid"),
                qc("height"),
                qc("index"),
                qc("merklePath"),
                qc("rawTx"),
                qc("blockHash"),
                qc("merkleRoot"),
                qc("provenTxId")
            );
            let created = p.created_at;
            let updated = p.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                &p.txid,
                p.height,
                p.index,
                &p.merkle_path,
                &p.raw_tx,
                &p.block_hash,
                &p.merkle_root
            )
        }

        pub(crate) async fn insert_proven_tx_req_impl(
            &self,
            r: &ProvenTxReq,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO proven_tx_reqs ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("provenTxId"),
                qc("status"),
                qc("attempts"),
                qc("notified"),
                qc("txid"),
                qc("batch"),
                qc("history"),
                qc("notify"),
                qc("rawTx"),
                qc("inputBEEF"),
                qc("provenTxReqId")
            );
            let created = r.created_at;
            let updated = r.updated_at;
            let status_str = r.status.to_string();
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                r.proven_tx_id,
                &status_str,
                r.attempts,
                r.notified,
                &r.txid,
                &r.batch,
                &r.history,
                &r.notify,
                &r.raw_tx,
                &r.input_beef
            )
        }

        pub(crate) async fn insert_transaction_impl(
            &self,
            t: &crate::tables::Transaction,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO transactions ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("provenTxId"),
                qc("status"),
                qc("reference"),
                qc("isOutgoing"),
                qc("satoshis"),
                qc("description"),
                qc("version"),
                qc("lockTime"),
                qc("txid"),
                qc("inputBEEF"),
                qc("rawTx"),
                qc("transactionId")
            );
            let created = t.created_at;
            let updated = t.updated_at;
            let status_str = t.status.to_string();
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                t.user_id,
                t.proven_tx_id,
                &status_str,
                &t.reference,
                t.is_outgoing,
                t.satoshis,
                &t.description,
                t.version,
                t.lock_time,
                &t.txid,
                &t.input_beef,
                &t.raw_tx
            )
        }

        pub(crate) async fn insert_tx_label_impl(
            &self,
            l: &TxLabel,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO tx_labels ({}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("label"),
                qc("isDeleted"),
                qc("txLabelId")
            );
            let created = l.created_at;
            let updated = l.updated_at;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                l.user_id,
                &l.label,
                l.is_deleted
            )
        }

        pub(crate) async fn insert_tx_label_map_impl(
            &self,
            m: &TxLabelMap,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = format!(
                "INSERT INTO tx_labels_map ({}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5)",
                qc("created_at"),
                qc("updated_at"),
                qc("txLabelId"),
                qc("transactionId"),
                qc("isDeleted")
            );
            let created = m.created_at;
            let updated = m.updated_at;
            exec_insert_no_id_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                m.tx_label_id,
                m.transaction_id,
                m.is_deleted
            )
        }

        pub(crate) async fn insert_sync_state_impl(
            &self,
            ss: &SyncState,
            trx: Option<&TrxToken>,
        ) -> WalletResult<i64> {
            let sql = format!(
                "INSERT INTO sync_states ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING {}",
                qc("created_at"),
                qc("updated_at"),
                qc("userId"),
                qc("storageIdentityKey"),
                qc("storageName"),
                qc("status"),
                qc("init"),
                qc("refNum"),
                qc("syncMap"),
                qc("when"),
                qc("satoshis"),
                qc("errorLocal"),
                qc("errorOther"),
                qc("syncStateId")
            );
            let created = ss.created_at;
            let updated = ss.updated_at;
            let status_str = ss.status.to_string();
            let when_value = ss.when;
            exec_insert_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                ss.user_id,
                &ss.storage_identity_key,
                &ss.storage_name,
                &status_str,
                ss.init,
                &ss.ref_num,
                &ss.sync_map,
                when_value,
                ss.satoshis,
                &ss.error_local,
                &ss.error_other
            )
        }

        pub(crate) async fn insert_settings_impl(
            &self,
            s: &Settings,
            trx: Option<&TrxToken>,
        ) -> WalletResult<()> {
            let sql = format!(
                "INSERT INTO settings ({}, {}, {}, {}, {}, {}, {}) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                qc("created_at"),
                qc("updated_at"),
                qc("storageIdentityKey"),
                qc("storageName"),
                qc("chain"),
                qc("dbtype"),
                qc("maxOutputScript")
            );
            let created = s.created_at;
            let updated = s.updated_at;
            let chain_str = s.chain.to_string();
            exec_insert_no_id_pg!(
                self,
                &sql,
                trx,
                created,
                updated,
                &s.storage_identity_key,
                &s.storage_name,
                &chain_str,
                &s.dbtype,
                s.max_output_script
            )
        }
    }
}
