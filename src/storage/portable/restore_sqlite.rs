//! Id-preserving restore inserts for SQLite (TS `restoreBRC38`).
//!
//! The regular insert path autoincrements primary keys and truncates
//! timestamps to whole seconds; a restore must preserve both exactly so that
//! re-exporting a restored wallet reproduces the original document. These
//! inserts write every column, including primary keys and
//! millisecond-precision timestamps, and only ever run inside the restore
//! transaction against a verified-empty target.

use async_trait::async_trait;
use chrono::NaiveDateTime;
use sqlx::Sqlite;

use crate::error::{WalletError, WalletResult};
use crate::storage::sqlx_impl::{SqliteStorage, StorageSqlx};
use crate::storage::TrxToken;

use super::import::{DecodedBrc38, PortableStorage};

/// Format a timestamp for the TEXT columns with milliseconds preserved.
fn dt(value: &NaiveDateTime) -> String {
    value.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn opt_dt(value: &Option<NaiveDateTime>) -> Option<String> {
    value.as_ref().map(dt)
}

macro_rules! exec {
    ($trx:expr, $sql:expr, $( $bind:expr ),* $(,)?) => {{
        let inner = StorageSqlx::<Sqlite>::extract_sqlite_trx($trx)?;
        let mut guard = inner.lock().await;
        let tx = guard.as_mut().ok_or_else(|| {
            WalletError::Internal("Transaction already consumed".to_string())
        })?;
        sqlx::query($sql)
            $( .bind($bind) )*
            .execute(&mut **tx)
            .await?;
    }};
}

#[async_trait]
impl PortableStorage for SqliteStorage {
    async fn restore_brc38_rows(
        &self,
        decoded: &DecodedBrc38,
        trx: &TrxToken,
    ) -> WalletResult<()> {
        let u = &decoded.user;
        exec!(
            trx,
            "INSERT INTO users (created_at, updated_at, userId, identityKey, activeStorage) \
             VALUES (?, ?, ?, ?, ?)",
            dt(&u.created_at),
            dt(&u.updated_at),
            u.user_id,
            &u.identity_key,
            &u.active_storage,
        );
        for r in &decoded.proven_txs {
            exec!(
                trx,
                "INSERT INTO proven_txs (created_at, updated_at, provenTxId, txid, height, \
                 \"index\", merklePath, rawTx, blockHash, merkleRoot) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.proven_tx_id,
                &r.txid,
                r.height,
                r.index,
                &r.merkle_path,
                &r.raw_tx,
                &r.block_hash,
                &r.merkle_root,
            );
        }
        for r in &decoded.output_baskets {
            exec!(
                trx,
                "INSERT INTO output_baskets (created_at, updated_at, basketId, userId, name, \
                 numberOfDesiredUTXOs, minimumDesiredUTXOValue, isDeleted) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.basket_id,
                r.user_id,
                &r.name,
                r.number_of_desired_utxos,
                r.minimum_desired_utxo_value,
                r.is_deleted,
            );
        }
        for r in &decoded.output_tags {
            exec!(
                trx,
                "INSERT INTO output_tags (created_at, updated_at, outputTagId, userId, tag, \
                 isDeleted) VALUES (?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.output_tag_id,
                r.user_id,
                &r.tag,
                r.is_deleted,
            );
        }
        for r in &decoded.tx_labels {
            exec!(
                trx,
                "INSERT INTO tx_labels (created_at, updated_at, txLabelId, userId, label, \
                 isDeleted) VALUES (?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.tx_label_id,
                r.user_id,
                &r.label,
                r.is_deleted,
            );
        }
        for r in &decoded.transactions {
            exec!(
                trx,
                "INSERT INTO transactions (created_at, updated_at, transactionId, userId, \
                 provenTxId, status, reference, isOutgoing, satoshis, version, lockTime, \
                 description, txid, inputBEEF, rawTx) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.transaction_id,
                r.user_id,
                r.proven_tx_id,
                r.status.to_string(),
                &r.reference,
                r.is_outgoing,
                r.satoshis,
                r.version,
                r.lock_time,
                &r.description,
                &r.txid,
                &r.input_beef,
                &r.raw_tx,
            );
        }
        for r in &decoded.outputs {
            exec!(
                trx,
                "INSERT INTO outputs (created_at, updated_at, outputId, userId, transactionId, \
                 basketId, spendable, change, vout, satoshis, providedBy, purpose, type, \
                 outputDescription, txid, senderIdentityKey, derivationPrefix, derivationSuffix, \
                 customInstructions, spentBy, sequenceNumber, spendingDescription, scriptLength, \
                 scriptOffset, lockingScript) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.output_id,
                r.user_id,
                r.transaction_id,
                r.basket_id,
                r.spendable,
                r.change,
                r.vout,
                r.satoshis,
                r.provided_by.to_string(),
                &r.purpose,
                &r.output_type,
                &r.output_description,
                &r.txid,
                &r.sender_identity_key,
                &r.derivation_prefix,
                &r.derivation_suffix,
                &r.custom_instructions,
                r.spent_by,
                r.sequence_number,
                &r.spending_description,
                r.script_length,
                r.script_offset,
                &r.locking_script,
            );
        }
        for r in &decoded.tx_label_maps {
            exec!(
                trx,
                "INSERT INTO tx_labels_map (created_at, updated_at, txLabelId, transactionId, \
                 isDeleted) VALUES (?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.tx_label_id,
                r.transaction_id,
                r.is_deleted,
            );
        }
        for r in &decoded.output_tag_maps {
            exec!(
                trx,
                "INSERT INTO output_tags_map (created_at, updated_at, outputTagId, outputId, \
                 isDeleted) VALUES (?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.output_tag_id,
                r.output_id,
                r.is_deleted,
            );
        }
        for r in &decoded.certificates {
            exec!(
                trx,
                "INSERT INTO certificates (created_at, updated_at, certificateId, userId, \
                 serialNumber, type, certifier, subject, verifier, revocationOutpoint, \
                 signature, isDeleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.certificate_id,
                r.user_id,
                &r.serial_number,
                &r.cert_type,
                &r.certifier,
                &r.subject,
                &r.verifier,
                &r.revocation_outpoint,
                &r.signature,
                r.is_deleted,
            );
        }
        for r in &decoded.certificate_fields {
            exec!(
                trx,
                "INSERT INTO certificate_fields (created_at, updated_at, userId, certificateId, \
                 fieldName, fieldValue, masterKey) VALUES (?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.user_id,
                r.certificate_id,
                &r.field_name,
                &r.field_value,
                &r.master_key,
            );
        }
        for r in &decoded.commissions {
            exec!(
                trx,
                "INSERT INTO commissions (created_at, updated_at, commissionId, userId, \
                 transactionId, satoshis, keyOffset, isRedeemed, lockingScript) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.commission_id,
                r.user_id,
                r.transaction_id,
                r.satoshis,
                &r.key_offset,
                r.is_redeemed,
                &r.locking_script,
            );
        }
        for r in &decoded.proven_tx_reqs {
            exec!(
                trx,
                "INSERT INTO proven_tx_reqs (created_at, updated_at, provenTxReqId, provenTxId, \
                 status, attempts, notified, txid, batch, history, notify, rawTx, inputBEEF) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.proven_tx_req_id,
                r.proven_tx_id,
                r.status.to_string(),
                r.attempts,
                r.notified,
                &r.txid,
                &r.batch,
                &r.history,
                &r.notify,
                &r.raw_tx,
                &r.input_beef,
            );
        }
        for r in &decoded.sync_states {
            exec!(
                trx,
                "INSERT INTO sync_states (created_at, updated_at, syncStateId, userId, \
                 storageIdentityKey, storageName, status, init, refNum, syncMap, \"when\", \
                 satoshis, errorLocal, errorOther) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                dt(&r.created_at),
                dt(&r.updated_at),
                r.sync_state_id,
                r.user_id,
                &r.storage_identity_key,
                &r.storage_name,
                r.status.to_string(),
                r.init,
                &r.ref_num,
                &r.sync_map,
                opt_dt(&r.when),
                r.satoshis,
                &r.error_local,
                &r.error_other,
            );
        }
        Ok(())
    }
}
