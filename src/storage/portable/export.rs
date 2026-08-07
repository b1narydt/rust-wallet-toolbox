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
