//! BRC-38 export (TS `exportBRC38` / `exportBRC38Json`).
//!
//! Reads one user's complete wallet state and produces the canonical
//! BRC-38 document. Rows not reachable from the user are excluded:
//! provenTxReqs are limited to the user's transaction txids, provenTxs to
//! those referenced by transactions or reqs, and the map tables to rows whose
//! endpoints are both exported.

use chrono::Utc;
use serde_json::{json, Map, Value};

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::*;
use crate::storage::traits::provider::StorageProvider;

use super::canonical::{canonicalize, iso_date};
use super::row::portable_row;
use super::validate::{sort_brc38_tables, validate_brc38, Brc38WalletData, BRC38_TITLE};

/// Export a user's wallet state as a validated BRC-38 document.
pub async fn export_brc38(
    storage: &dyn StorageProvider,
    identity_key: &str,
) -> WalletResult<Brc38WalletData> {
    let source_storage = storage.make_available().await?;
    let user = storage
        .find_user_by_identity_key(identity_key, None)
        .await?
        .ok_or_else(|| {
            WalletError::BadRequest(format!("BRC-38 export: unknown identityKey {identity_key}"))
        })?;
    let user_id = user.user_id;
    let for_user = || FindForUserSincePagedArgs {
        user_id,
        since: None,
        paged: None,
    };

    let transactions = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let transaction_ids: std::collections::HashSet<i64> =
        transactions.iter().map(|t| t.transaction_id).collect();
    let transaction_txids: std::collections::HashSet<&str> = transactions
        .iter()
        .filter_map(|t| t.txid.as_deref())
        .collect();

    let proven_tx_reqs: Vec<_> = storage
        .get_proven_tx_reqs_for_user(&for_user(), None)
        .await?
        .into_iter()
        .filter(|r| transaction_txids.contains(r.txid.as_str()))
        .collect();

    let mut proven_tx_ids: Vec<i64> = transactions
        .iter()
        .filter_map(|t| t.proven_tx_id)
        .chain(proven_tx_reqs.iter().filter_map(|r| r.proven_tx_id))
        .collect();
    proven_tx_ids.sort_unstable();
    proven_tx_ids.dedup();

    let mut proven_txs = Vec::new();
    for proven_tx_id in proven_tx_ids {
        let mut found = storage
            .find_proven_txs(
                &FindProvenTxsArgs {
                    partial: ProvenTxPartial {
                        proven_tx_id: Some(proven_tx_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await?;
        if found.len() > 1 {
            return Err(WalletError::Internal(format!(
                "BRC-38 export: multiple provenTxs for id {proven_tx_id}"
            )));
        }
        if let Some(proven) = found.pop() {
            proven_txs.push(proven);
        }
    }

    let output_baskets = storage
        .find_output_baskets(
            &FindOutputBasketsArgs {
                partial: OutputBasketPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let commissions = storage
        .find_commissions(
            &FindCommissionsArgs {
                partial: CommissionPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let outputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    // BACKFILL OFFLOADED LOCKING SCRIPTS, exactly as TS does on read.
    //
    // TypeScript OFFLOADS a locking script whose length exceeds
    // `settings.maxOutputScript` — `processAction.ts:295` sets
    // `lockingScript = undefined` and keeps `scriptOffset`/`scriptLength`
    // pointing into the parent transaction's `rawTx`. Its readers then refill
    // it (`StorageProvider.validateOutputScript` -> `getRawTxOfKnownValidTransaction`)
    // before returning the row, so a TS export always carries the script.
    //
    // This port did not, and `lockingScript` is optional in the BRC-38 schema,
    // so an offloaded row serialized with the field simply ABSENT: the document
    // validated, imported, and reported success, having dropped the script.
    //
    // THE DEFAULT `maxOutputScript` IS 100 BYTES, so this is routine, not
    // exotic. A P2PKH script (25 bytes) stays inline; any OP_RETURN data
    // payload is offloaded immediately. On a BSV wallet that writes data —
    // which is most of them — the majority of large outputs take this path.
    // That is also why REFUSING to export here would be wrong: it would block
    // backup of exactly those wallets, and for an OP_RETURN output the "cannot
    // be spent afterwards" argument is vacuous, since it was never spendable.
    // Backfilling is the only answer that is both lossless and non-blocking.
    //
    // Everything needed is already in hand: `transactions` (fetched above) and
    // `proven_txs` (derived above) are both in the export set, and
    // `ProvenTx::raw_tx` is non-optional — it is the fallback for the case
    // where `processAction.ts:417` also cleared `transactions.rawTx` once the
    // transaction was proven.
    let mut outputs = outputs;
    backfill_offloaded_scripts(&mut outputs, &transactions, &proven_txs)?;

    let output_ids: std::collections::HashSet<i64> = outputs.iter().map(|o| o.output_id).collect();
    let output_tags = storage
        .find_output_tags(
            &FindOutputTagsArgs {
                partial: OutputTagPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let output_tag_maps: Vec<_> = storage
        .get_output_tag_maps_for_user(&for_user(), None)
        .await?
        .into_iter()
        .filter(|m| output_ids.contains(&m.output_id))
        .collect();
    let tx_labels = storage
        .find_tx_labels(
            &FindTxLabelsArgs {
                partial: TxLabelPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let tx_label_maps: Vec<_> = storage
        .get_tx_label_maps_for_user(&for_user(), None)
        .await?
        .into_iter()
        .filter(|m| transaction_ids.contains(&m.transaction_id))
        .collect();
    let certificates = storage
        .find_certificates(
            &FindCertificatesArgs {
                partial: CertificatePartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let certificate_fields = storage
        .find_certificate_fields(
            &FindCertificateFieldsArgs {
                partial: CertificateFieldPartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;
    let sync_states = storage
        .find_sync_states(
            &FindSyncStatesArgs {
                partial: SyncStatePartial {
                    user_id: Some(user_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;

    let mut tables = Map::new();
    tables.insert("provenTxs".into(), rows("provenTx", &proven_txs)?);
    tables.insert("provenTxReqs".into(), rows("provenTxReq", &proven_tx_reqs)?);
    tables.insert(
        "outputBaskets".into(),
        rows("outputBasket", &output_baskets)?,
    );
    tables.insert("transactions".into(), rows("transaction", &transactions)?);
    tables.insert("commissions".into(), rows("commission", &commissions)?);
    tables.insert("outputs".into(), rows("output", &outputs)?);
    tables.insert("outputTags".into(), rows("outputTag", &output_tags)?);
    tables.insert(
        "outputTagMaps".into(),
        rows("outputTagMap", &output_tag_maps)?,
    );
    tables.insert("txLabels".into(), rows("txLabel", &tx_labels)?);
    tables.insert("txLabelMaps".into(), rows("txLabelMap", &tx_label_maps)?);
    tables.insert("certificates".into(), rows("certificate", &certificates)?);
    tables.insert(
        "certificateFields".into(),
        rows("certificateField", &certificate_fields)?,
    );
    tables.insert("syncStates".into(), rows("syncState", &sync_states)?);
    sort_brc38_tables(&mut tables)?;

    let document = json!({
        "brc": 38,
        "title": BRC38_TITLE,
        "formatVersion": 1,
        "exportedAt": iso_date(&Utc::now().naive_utc()),
        "sourceStorage": Value::Object(portable_row("settings", &source_storage)?),
        "user": Value::Object(portable_row("user", &user)?),
        "tables": Value::Object(tables),
    });
    validate_brc38(document)
}

/// Export a user's wallet state as canonical BRC-38 JSON bytes.
pub async fn export_brc38_json(
    storage: &dyn StorageProvider,
    identity_key: &str,
) -> WalletResult<String> {
    canonicalize(export_brc38(storage, identity_key).await?.as_value())
}

fn rows<T: serde::Serialize>(kind: &str, items: &[T]) -> WalletResult<Value> {
    Ok(Value::Array(
        items
            .iter()
            .map(|item| Ok(Value::Object(portable_row(kind, item)?)))
            .collect::<WalletResult<_>>()?,
    ))
}

/// Restore locking scripts that were OFFLOADED into the parent transaction's
/// `rawTx`, so the exported document carries every script inline.
///
/// TypeScript offloads any script longer than `settings.maxOutputScript`
/// (`processAction.ts:295`): it clears `lockingScript` and leaves
/// `scriptOffset`/`scriptLength` pointing into the transaction. Its readers
/// refill the row before returning it (`StorageProvider.validateOutputScript`
/// -> `getRawTxOfKnownValidTransaction`), so a TS export always carries the
/// script. This port did not, and because `lockingScript` is optional in the
/// BRC-38 schema an offloaded row serialized with the field simply ABSENT —
/// validating, importing, and reporting success having dropped it.
///
/// **The default `maxOutputScript` is 100 bytes**, so this is the common path,
/// not an edge case: a P2PKH script (25 bytes) stays inline while any OP_RETURN
/// data payload is offloaded immediately. That is also why refusing to export
/// would be the wrong remedy — it would block backup of every wallet that
/// writes data, and for an OP_RETURN output the "cannot be spent afterwards"
/// argument is vacuous, since it was never spendable. Backfilling is the only
/// answer that is both lossless and non-blocking.
///
/// Fails loudly when the bytes are genuinely unavailable. Silently emitting a
/// row without its script is the one outcome this function exists to prevent.
fn backfill_offloaded_scripts(
    outputs: &mut [crate::tables::Output],
    transactions: &[crate::tables::Transaction],
    proven_txs: &[crate::tables::ProvenTx],
) -> WalletResult<()> {
    let mut raw_by_txid: std::collections::HashMap<&str, &[u8]> = std::collections::HashMap::new();
    for t in transactions {
        if let (Some(txid), Some(raw)) = (t.txid.as_deref(), t.raw_tx.as_deref()) {
            raw_by_txid.insert(txid, raw);
        }
    }
    // Proven transactions take precedence: `transactions.rawTx` is the copy TS
    // clears once a transaction is proven (`processAction.ts:417`), so the
    // proven row is the surviving source.
    for p in proven_txs {
        raw_by_txid.insert(p.txid.as_str(), p.raw_tx.as_slice());
    }

    for o in outputs
        .iter_mut()
        .filter(|o| o.locking_script.is_none() && o.script_offset.is_some())
    {
        let describe = format!(
            "output {} (transaction {}, txid {:?}, scriptOffset {:?}, scriptLength {:?})",
            o.output_id, o.transaction_id, o.txid, o.script_offset, o.script_length
        );
        let txid = o.txid.as_deref().ok_or_else(|| {
            WalletError::InvalidOperation(format!(
                "cannot restore the offloaded lockingScript for {describe}: the row carries no \
                 txid, so there is no transaction to read it back from"
            ))
        })?;
        let raw = raw_by_txid.get(txid).copied().ok_or_else(|| {
            WalletError::InvalidOperation(format!(
                "cannot restore the offloaded lockingScript for {describe}: neither the \
                 transaction nor a proven transaction in this export carries its rawTx. Exporting \
                 would silently drop the script."
            ))
        })?;
        // `try_from`, not `as`: a negative or oversized offset must fail loudly
        // rather than wrap into a valid-looking slice of the wrong bytes.
        let start = usize::try_from(o.script_offset.unwrap_or(-1)).map_err(|_| {
            WalletError::InvalidOperation(format!("{describe}: scriptOffset is not a valid index"))
        })?;
        let len = usize::try_from(o.script_length.unwrap_or(-1)).map_err(|_| {
            WalletError::InvalidOperation(format!("{describe}: scriptLength is not a valid length"))
        })?;
        let end = start.checked_add(len).ok_or_else(|| {
            WalletError::InvalidOperation(format!(
                "{describe}: scriptOffset + scriptLength overflows"
            ))
        })?;
        let script = raw.get(start..end).ok_or_else(|| {
            WalletError::InvalidOperation(format!(
                "{describe}: the script slice lies outside the {} byte rawTx — the offset is stale \
                 or the rawTx belongs to a different transaction",
                raw.len()
            ))
        })?;
        o.locking_script = Some(script.to_vec());
    }
    Ok(())
}

#[cfg(test)]
mod backfill_tests {
    use super::*;
    use crate::status::TransactionStatus;
    use crate::tables::{Output, ProvenTx, Transaction};
    use crate::types::StorageProvidedBy;
    use chrono::NaiveDateTime;

    fn t0() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-01-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
    }

    /// An OP_RETURN-shaped script: `OP_FALSE OP_RETURN PUSH(32)` + payload.
    /// Shaped like the data outputs that actually get offloaded — a P2PKH at 25
    /// bytes stays inline under the 100-byte default, these do not.
    fn op_return_script() -> Vec<u8> {
        let mut s = vec![0x00, 0x6a, 0x20];
        s.extend_from_slice(&[0xab; 32]);
        s
    }

    /// A rawTx with the script embedded at a known offset.
    fn raw_tx_containing(script: &[u8], offset: usize) -> Vec<u8> {
        let mut raw = vec![0x11; offset];
        raw.extend_from_slice(script);
        raw.extend_from_slice(&[0x22; 9]);
        raw
    }

    fn output(txid: &str, script: Option<Vec<u8>>, offset: i64, len: i64) -> Output {
        Output {
            created_at: t0(),
            updated_at: t0(),
            output_id: 1,
            user_id: 1,
            transaction_id: 7,
            spendable: false, // an OP_RETURN never was
            change: false,
            vout: 0,
            satoshis: 0,
            provided_by: StorageProvidedBy::You,
            purpose: String::new(),
            output_type: "custom".into(),
            txid: Some(txid.to_string()),
            locking_script: script,
            script_offset: Some(offset),
            script_length: Some(len),
            basket_id: None,
            output_description: None,
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
        }
    }

    fn transaction(txid: &str, raw_tx: Option<Vec<u8>>) -> Transaction {
        Transaction {
            created_at: t0(),
            updated_at: t0(),
            transaction_id: 7,
            user_id: 1,
            status: TransactionStatus::Completed,
            reference: "r".into(),
            is_outgoing: false,
            satoshis: 0,
            description: String::new(),
            txid: Some(txid.to_string()),
            raw_tx,
            proven_tx_id: None,
            version: None,
            lock_time: None,
            input_beef: None,
        }
    }

    fn proven(txid: &str, raw_tx: Vec<u8>) -> ProvenTx {
        ProvenTx {
            created_at: t0(),
            updated_at: t0(),
            proven_tx_id: 1,
            txid: txid.to_string(),
            height: 1,
            index: 0,
            merkle_path: vec![],
            raw_tx,
            block_hash: String::new(),
            merkle_root: String::new(),
        }
    }

    /// The success path: an offloaded OP_RETURN script is restored from the
    /// parent transaction's rawTx, byte for byte.
    #[test]
    fn an_offloaded_script_is_restored_from_the_parent_raw_tx() {
        let script = op_return_script();
        let txid = "aa".repeat(32);
        let mut outputs = vec![output(&txid, None, 12, script.len() as i64)];
        let tx = transaction(&txid, Some(raw_tx_containing(&script, 12)));

        backfill_offloaded_scripts(&mut outputs, &[tx], &[]).unwrap();

        assert_eq!(
            outputs[0].locking_script.as_deref(),
            Some(script.as_slice()),
            "the exported row must carry the script the rawTx holds"
        );
    }

    /// `provenTxs` is the fallback, because TS clears `transactions.rawTx` once
    /// a transaction is proven — for a proven transaction it is the ONLY source.
    #[test]
    fn a_proven_tx_supplies_the_script_when_the_transaction_row_lost_its_raw_tx() {
        let script = op_return_script();
        let txid = "bb".repeat(32);
        let mut outputs = vec![output(&txid, None, 5, script.len() as i64)];
        let tx = transaction(&txid, None); // cleared once proven
        let p = proven(&txid, raw_tx_containing(&script, 5));

        backfill_offloaded_scripts(&mut outputs, &[tx], &[p]).unwrap();

        assert_eq!(
            outputs[0].locking_script.as_deref(),
            Some(script.as_slice())
        );
    }

    /// When the bytes are genuinely unavailable we FAIL rather than emit a row
    /// without its script. Silent omission is the defect this exists to prevent.
    #[test]
    fn an_unrecoverable_script_fails_loudly_rather_than_exporting_without_it() {
        let txid = "cc".repeat(32);
        let mut outputs = vec![output(&txid, None, 12, 35)];

        let err = backfill_offloaded_scripts(&mut outputs, &[], &[])
            .expect_err("no rawTx anywhere — must not silently succeed");
        assert!(err.to_string().contains("rawTx"), "{err}");
        assert!(
            outputs[0].locking_script.is_none(),
            "a failed backfill must not half-write the row"
        );
    }

    /// A stale offset must not slice the wrong bytes into a locking script.
    #[test]
    fn an_out_of_range_slice_is_refused() {
        let txid = "dd".repeat(32);
        let mut outputs = vec![output(&txid, None, 12, 99)];
        let tx = transaction(&txid, Some(vec![0x11; 20]));

        let err = backfill_offloaded_scripts(&mut outputs, &[tx], &[])
            .expect_err("the slice runs past the end of rawTx");
        assert!(err.to_string().contains("outside"), "{err}");
    }

    /// Rows that already carry their script are untouched — the backfill must
    /// not rewrite an inline script from a possibly-stale offset.
    #[test]
    fn an_inline_script_is_left_alone() {
        let txid = "ee".repeat(32);
        let inline = vec![0x76, 0xa9, 0x14];
        let mut outputs = vec![output(&txid, Some(inline.clone()), 0, 3)];
        let tx = transaction(&txid, Some(vec![0x99; 64]));

        backfill_offloaded_scripts(&mut outputs, &[tx], &[]).unwrap();

        assert_eq!(outputs[0].locking_script, Some(inline));
    }
}
