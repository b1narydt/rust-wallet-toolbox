//! Standalone abort_action for WalletStorageProvider blanket impl.
//!
//! Extracted from `WalletStorageManager::abort_action` so it can be called
//! from the blanket impl over `StorageProvider` without holding a manager lock.
//! The logic is identical to the manager version, operating directly on the
//! single provider rather than routing through active/backup.

use base64::Engine as _;
use bsv::wallet::interfaces::{AbortActionArgs, AbortActionResult};
use std::io::Cursor;

use crate::error::{WalletError, WalletResult};
use crate::status::{ProvenTxReqStatus, TransactionStatus};
use crate::storage::find_args::{
    FindOutputsArgs, FindProvenTxReqsArgs, FindTransactionsArgs, OutputPartial, ProvenTxReqPartial,
    TransactionPartial,
};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

/// Abort (cancel) a transaction by reference or txid.
///
/// Finds the transaction by reference, validates it is in an abortable status,
/// releases its inputs, retires its outputs, fails the transaction, and
/// invalidates the associated proof request.
pub async fn abort_action(
    storage: &(dyn StorageProvider + Send + Sync),
    auth: &str,
    args: &AbortActionArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<AbortActionResult> {
    let user = storage
        .find_user_by_identity_key(auth, trx)
        .await?
        .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;

    // BRC-100 transports `reference` as Base64String. The SDK deserializes it
    // to bytes, while createAction stores the base64 text as its reference.
    // Re-encode the bytes for the storage lookup; lossy UTF-8 here made a
    // wire-level abort unable to find an action it had just created.
    let reference = base64::engine::general_purpose::STANDARD.encode(&args.reference);

    let mut txs = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user.user_id),
                    reference: Some(reference.clone()),
                    ..Default::default()
                },
                no_raw_tx: false,
                ..Default::default()
            },
            trx,
        )
        .await?;

    // A TypeScript client can supply a 64-character txid in the Base64String
    // reference field. Serde has decoded that wire text to 48 bytes, so recover
    // it from the canonical re-encoding. Keep accepting ASCII txids from direct
    // Rust callers as well. In either case, reference lookup retains precedence.
    let txid_fallback = (reference.len() == 64)
        .then(|| reference.clone())
        .or_else(|| {
            std::str::from_utf8(&args.reference)
                .ok()
                .filter(|value| value.len() == 64)
                .map(str::to_owned)
        });
    if txs.is_empty() {
        if let Some(txid) = &txid_fallback {
            txs = storage
                .find_transactions(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            user_id: Some(user.user_id),
                            txid: Some(txid.clone()),
                            ..Default::default()
                        },
                        no_raw_tx: false,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
        }
    }

    let tx = if txs.is_empty() {
        return Err(WalletError::InvalidParameter {
            parameter: "reference".to_string(),
            must_be: "an existing action reference".to_string(),
        });
    } else {
        txs.into_iter().next().unwrap()
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
            must_be:
                "an inprocess, outgoing action that has not been signed and shared to the network"
                    .to_string(),
        });
    }

    // Resolve inputs both from spentBy and from the signed raw transaction. A
    // persisted spentBy link may be absent when an off-chain nosend is aborted.
    let mut inputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user.user_id),
                    spent_by: Some(tx.transaction_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if let Some(raw_tx) = &tx.raw_tx {
        let parsed =
            bsv::transaction::transaction::Transaction::from_binary(&mut Cursor::new(raw_tx))
                .map_err(|e| {
                    WalletError::Internal(format!(
                        "abort_action: transaction {} has invalid rawTx: {e}",
                        tx.transaction_id
                    ))
                })?;
        for raw_input in parsed.inputs {
            let Some(source_txid) = raw_input.source_txid else {
                continue;
            };
            let matches = storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user.user_id),
                            txid: Some(source_txid),
                            vout: Some(raw_input.source_output_index as i32),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for output in matches {
                if !inputs
                    .iter()
                    .any(|existing| existing.output_id == output.output_id)
                {
                    inputs.push(output);
                }
            }
        }
    }

    for input in &inputs {
        storage
            .update_output(
                input.output_id,
                &OutputPartial {
                    spendable: Some(true),
                    spent_by: Some(0), // 0 = unset
                    ..Default::default()
                },
                trx,
            )
            .await?;
    }

    // Outputs created by the failed transaction cannot remain spendable, and
    // no later transaction may remain recorded as their spender.
    let created_outputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user.user_id),
                    transaction_id: Some(tx.transaction_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;
    for output in created_outputs {
        if output.spendable || output.spent_by.is_some() {
            storage
                .update_output(
                    output.output_id,
                    &OutputPartial {
                        spendable: Some(false),
                        spent_by: Some(0),
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
        }
    }

    // Update transaction status to failed
    storage
        .update_transaction(
            tx.transaction_id,
            &TransactionPartial {
                status: Some(TransactionStatus::Failed),
                ..Default::default()
            },
            trx,
        )
        .await?;

    if let Some(txid) = &tx.txid {
        let reqs = storage
            .find_proven_tx_reqs(
                &FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial {
                        txid: Some(txid.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trx,
            )
            .await?;
        if let Some(req) = reqs.into_iter().next() {
            let original_reference = txid_fallback.as_deref().unwrap_or(&reference);
            let mut history: serde_json::Value =
                serde_json::from_str(&req.history).unwrap_or_else(|_| serde_json::json!({}));
            let history_object = history.as_object_mut().ok_or_else(|| {
                WalletError::Internal(format!(
                    "abort_action: proven request {} history is not an object",
                    req.proven_tx_req_id
                ))
            })?;
            let notes = history_object
                .entry("notes")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or_else(|| {
                    WalletError::Internal(format!(
                        "abort_action: proven request {} history notes is not an array",
                        req.proven_tx_req_id
                    ))
                })?;
            let when = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            notes.push(serde_json::json!({
                "what": "abortAction",
                "reference": original_reference,
                "when": when,
            }));
            if req.status != ProvenTxReqStatus::Invalid {
                notes.push(serde_json::json!({
                    "what": "status",
                    "status_was": req.status.to_string(),
                    "status_now": ProvenTxReqStatus::Invalid.to_string(),
                    "when": when,
                }));
            }
            storage
                .update_proven_tx_req(
                    req.proven_tx_req_id,
                    &ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Invalid),
                        history: Some(history.to_string()),
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
        }
    }

    Ok(AbortActionResult { aborted: true })
}
