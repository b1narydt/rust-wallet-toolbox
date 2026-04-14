//! BroadcastOutcome classifier and post-broadcast state updater.
//!
//! Translates raw per-provider broadcast results into a single semantic
//! outcome, then drives DB state transitions: marking outputs unspendable
//! on permanent failure, restoring inputs, and reconciling against the
//! chain before applying permanent state. Matches the TS `aggregateActionResults`
//! and Calhooon `update_transaction_status_after_broadcast` patterns.

use crate::error::WalletResult;
use crate::services::traits::WalletServices;
use crate::services::types::PostBeefResult;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
};
use crate::storage::manager::WalletStorageManager;
use crate::storage::TrxToken;

/// Semantic outcome of a broadcast attempt across all providers.
#[derive(Debug, Clone)]
pub enum BroadcastOutcome {
    /// At least one provider accepted the transaction.
    Success,
    /// All providers returned transient service errors (timeout, network, etc.).
    ServiceError { details: Vec<String> },
    /// At least one provider rejected the transaction as invalid.
    InvalidTx { details: Vec<String> },
    /// At least one provider detected a true double-spend (DOUBLE_SPEND_ATTEMPTED).
    DoubleSpend {
        competing_txs: Vec<String>,
        details: Vec<String>,
    },
    /// At least one provider reported SEEN_IN_ORPHAN_MEMPOOL — parent tx not
    /// yet propagated. This is transient and should be retried.
    OrphanMempool { details: Vec<String> },
}

/// Classify broadcast results from all providers into a single outcome.
///
/// Priority (matching TS/Go):
/// 1. Any provider reports success -> Success
/// 2. Any double_spend flag -> DoubleSpend
/// 3. Any definitive rejection (status "error" without service_error/orphan flags) -> InvalidTx
/// 4. Any orphan_mempool flag -> OrphanMempool
/// 5. Otherwise -> ServiceError
pub fn classify_broadcast_results(results: &[PostBeefResult]) -> BroadcastOutcome {
    let mut has_success = false;
    let mut has_double_spend = false;
    let mut has_orphan = false;
    let mut has_invalid = false;
    let mut competing_txs = Vec::new();
    let mut details = Vec::new();

    for provider_result in results {
        if provider_result.status == "success" {
            has_success = true;
        }

        for txid_result in &provider_result.txid_results {
            if txid_result.status == "success" {
                has_success = true;
                continue;
            }

            let detail = format!(
                "{}: {} (txid={})",
                provider_result.name, txid_result.status, txid_result.txid
            );
            details.push(detail);

            if txid_result.double_spend == Some(true) {
                has_double_spend = true;
                if let Some(ref ctxs) = txid_result.competing_txs {
                    competing_txs.extend(ctxs.iter().cloned());
                }
            } else if txid_result.orphan_mempool == Some(true) {
                has_orphan = true;
            } else if txid_result.service_error != Some(true) {
                // Not a service error, not orphan, not double-spend — definitive rejection
                has_invalid = true;
            }
        }
    }

    // Priority: success > double-spend > invalid > orphan > service-error
    if has_success {
        BroadcastOutcome::Success
    } else if has_double_spend {
        BroadcastOutcome::DoubleSpend {
            competing_txs,
            details,
        }
    } else if has_invalid {
        BroadcastOutcome::InvalidTx { details }
    } else if has_orphan {
        BroadcastOutcome::OrphanMempool { details }
    } else {
        BroadcastOutcome::ServiceError { details }
    }
}

/// Reconcile a broadcast outcome against the chain before applying permanent failure.
///
/// If any provider says the txid is "mined" or "known", override the outcome to Success.
/// This prevents false negatives from provider inconsistency (e.g. TAAL says REJECTED
/// but WoC still has the tx in mempool or mined).
async fn reconcile_tx_status(
    services: &(dyn WalletServices + Send + Sync),
    txid: &str,
    outcome: &BroadcastOutcome,
) -> BroadcastOutcome {
    // Only reconcile permanent failures — Success, ServiceError, OrphanMempool don't need it.
    if matches!(
        outcome,
        BroadcastOutcome::Success
            | BroadcastOutcome::ServiceError { .. }
            | BroadcastOutcome::OrphanMempool { .. }
    ) {
        return outcome.clone();
    }

    let status_result = services
        .get_status_for_txids(&[txid.to_string()], false)
        .await;

    for r in &status_result.results {
        if r.status == "mined" || r.status == "known" {
            tracing::info!(
                txid = %txid,
                chain_status = %r.status,
                "reconcile_tx_status: chain says tx is valid, overriding failure to Success"
            );
            return BroadcastOutcome::Success;
        }
    }

    outcome.clone()
}

/// Returns output IDs of consumed inputs whose specific outpoints are verified as
/// still unspent on chain. Used for DoubleSpend recovery: only inputs confirmed as
/// unspent (not consumed by a competing transaction) are safe to restore.
///
/// Per-outpoint check: for each consumed input we call
/// `services.is_utxo(locking_script, parent_txid, vout)`. Only outputs where that
/// returns `Ok(true)` are eligible for restoration. Inputs that fail the check
/// (or error) are left NOT restored, which is safer than falsely restoring a
/// spent input.
async fn utxo_verified_input_ids(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    failed_transaction_id: i64,
    trx: Option<&TrxToken>,
) -> WalletResult<Vec<i64>> {
    // Find outputs consumed as inputs by the failed transaction. These Output rows
    // represent the PARENT utxos that this tx spent — their `txid`, `vout`, and
    // `locking_script` identify the parent outpoint on chain.
    let consumed_inputs = storage
        .find_outputs_trx(
            &FindOutputsArgs {
                partial: OutputPartial {
                    spent_by: Some(failed_transaction_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if consumed_inputs.is_empty() {
        return Ok(Vec::new());
    }

    let mut verified: Vec<i64> = Vec::new();
    for output in &consumed_inputs {
        // Resolve the parent txid hex. Prefer the denormalized column on the
        // output row; fall back to looking up the parent transaction when
        // absent (some older rows may not have `txid` populated).
        let parent_txid: String = if let Some(ref t) = output.txid {
            t.clone()
        } else {
            let txs = storage
                .find_transactions_trx(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            transaction_id: Some(output.transaction_id),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            match txs.first().and_then(|t| t.txid.clone()) {
                Some(t) => t,
                None => {
                    tracing::warn!(
                        output_id = output.output_id,
                        transaction_id = output.transaction_id,
                        "utxo_verified_input_ids: parent tx has no txid, skipping (not restoring)"
                    );
                    continue;
                }
            }
        };

        let locking_script = match output.locking_script.as_deref() {
            Some(s) if !s.is_empty() => s,
            _ => {
                tracing::warn!(
                    output_id = output.output_id,
                    parent_txid = %parent_txid,
                    "utxo_verified_input_ids: missing locking_script, skipping (not restoring)"
                );
                continue;
            }
        };

        let vout = output.vout as u32;
        match services.is_utxo(locking_script, &parent_txid, vout).await {
            Ok(true) => {
                verified.push(output.output_id);
            }
            Ok(false) => {
                tracing::info!(
                    output_id = output.output_id,
                    parent_txid = %parent_txid,
                    vout = vout,
                    "utxo_verified_input_ids: parent outpoint no longer unspent, not restoring"
                );
            }
            Err(e) => {
                // On a per-outpoint chain error, err on the side of safety and do
                // not restore this input. Log and continue with the rest.
                tracing::warn!(
                    output_id = output.output_id,
                    parent_txid = %parent_txid,
                    vout = vout,
                    error = %e,
                    "utxo_verified_input_ids: is_utxo check errored, not restoring"
                );
            }
        }
    }

    Ok(verified)
}

/// Apply the post-broadcast state transition for Success / OrphanMempool outcomes.
///
/// Shared by `create_action` and `sign_action` to avoid drift between the two
/// broadcast paths. Only handles the non-permanent arms; DoubleSpend / InvalidTx
/// go through `handle_permanent_broadcast_failure` instead, and ServiceError
/// intentionally leaves state untouched so the monitor will retry.
///
/// Behavior:
/// - **Success**: Transaction -> Unproven, ProvenTxReq -> Unmined
/// - **OrphanMempool**: Transaction stays Sending (parent not yet propagated —
///   monitor will retry), ProvenTxReq -> Sending
///
/// Both the Transaction.status and ProvenTxReq.status are branched so the
/// monitor's re-broadcast contract (module doc above, "OrphanMempool/
/// ServiceError: tx stays Sending") is upheld. Earlier revisions set
/// Transaction -> Unproven unconditionally, which was the bug surfaced by PR
/// review §"IMPORTANT #5".
pub async fn apply_success_or_orphan_outcome(
    storage: &WalletStorageManager,
    txid: &str,
    outcome: &BroadcastOutcome,
) -> WalletResult<()> {
    let is_orphan = matches!(outcome, BroadcastOutcome::OrphanMempool { .. });

    // OrphanMempool: leave Transaction at Sending so the monitor retries once
    // the parent tx propagates. Success: advance to Unproven (accepted,
    // awaiting proof).
    let new_tx_status = if is_orphan {
        crate::status::TransactionStatus::Sending
    } else {
        crate::status::TransactionStatus::Unproven
    };
    let _ = storage.update_transaction_status(txid, new_tx_status).await;

    let reqs = storage
        .find_proven_tx_reqs(&crate::storage::find_args::FindProvenTxReqsArgs {
            partial: crate::storage::find_args::ProvenTxReqPartial {
                txid: Some(txid.to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .unwrap_or_default();

    let new_status = if is_orphan {
        crate::status::ProvenTxReqStatus::Sending
    } else {
        crate::status::ProvenTxReqStatus::Unmined
    };

    for req in &reqs {
        let _ = storage
            .update_proven_tx_req(
                req.proven_tx_req_id,
                &crate::storage::find_args::ProvenTxReqPartial {
                    status: Some(new_status.clone()),
                    ..Default::default()
                },
            )
            .await;
    }

    Ok(())
}

/// Update transaction and output state after a broadcast, based on the classified outcome.
///
/// - **Success**: tx -> Unproven, req -> Unmined (already handled in caller)
/// - **OrphanMempool/ServiceError**: tx stays Sending, req -> Sending (already handled in caller)
/// - **DoubleSpend**: tx -> Failed, req -> DoubleSpend, mark this tx's outputs unspendable,
///   restore inputs that are verified as still unspent on chain
/// - **InvalidTx**: tx -> Failed, req -> Invalid, mark this tx's outputs unspendable,
///   restore all inputs (safe — tx was malformed, inputs not consumed on chain)
///
/// This function handles ONLY the DoubleSpend and InvalidTx paths. Success/OrphanMempool/ServiceError
/// are handled inline in the signer methods.
pub async fn handle_permanent_broadcast_failure(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    txid: &str,
    outcome: &BroadcastOutcome,
) -> WalletResult<BroadcastOutcome> {
    // Reconcile against chain first (P4). Do this BEFORE opening a DB
    // transaction to avoid holding write locks across a network call.
    let effective = reconcile_tx_status(services, txid, outcome).await;

    // If chain reconciliation overrode to Success, return — caller will handle
    // Success path. No writes needed.
    if matches!(effective, BroadcastOutcome::Success) {
        return Ok(effective);
    }

    // Only DoubleSpend / InvalidTx require the failure-path state transitions.
    let (new_tx_status, req_status) = match &effective {
        BroadcastOutcome::DoubleSpend { .. } => (
            crate::status::TransactionStatus::Failed,
            crate::status::ProvenTxReqStatus::DoubleSpend,
        ),
        BroadcastOutcome::InvalidTx { .. } => (
            crate::status::TransactionStatus::Failed,
            crate::status::ProvenTxReqStatus::Invalid,
        ),
        _ => return Ok(effective),
    };

    // DoubleSpend path needs a per-outpoint UTXO check on the chain. Do those
    // network calls BEFORE opening the DB transaction so we don't hold write
    // locks over network I/O. The safe_output_ids result is then applied as a
    // pure set of DB writes under the atomic transaction.
    //
    // We resolve the tx.transaction_id here too, by opening a short-lived read
    // (no trx) — a small read outside the transaction is cheaper than hoisting
    // all the chain I/O into the trx window.
    let tx_lookup = storage
        .find_transactions_trx(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;

    let tx = match tx_lookup.first() {
        Some(t) => t.clone(),
        None => {
            tracing::warn!(
                txid = %txid,
                "handle_permanent_broadcast_failure: tx not found in storage"
            );
            return Ok(effective);
        }
    };

    let double_spend_safe_ids: Option<Vec<i64>> =
        if matches!(effective, BroadcastOutcome::DoubleSpend { .. }) {
            Some(utxo_verified_input_ids(storage, services, tx.transaction_id, None).await?)
        } else {
            None
        };

    // Open the atomic DB transaction. All writes below go through Some(&db_trx)
    // so that a single `?` failure discards ALL prior writes via sqlx rollback
    // when db_trx is dropped.
    let db_trx = storage.begin_transaction().await?;

    // 1. Transaction -> Failed
    storage
        .update_transaction_status_trx(txid, new_tx_status, Some(&db_trx))
        .await?;

    // 2. ProvenTxReq(s) -> DoubleSpend / Invalid
    let reqs = storage
        .find_proven_tx_reqs_trx(
            &crate::storage::find_args::FindProvenTxReqsArgs {
                partial: crate::storage::find_args::ProvenTxReqPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            Some(&db_trx),
        )
        .await?;
    for req in &reqs {
        storage
            .update_proven_tx_req_trx(
                req.proven_tx_req_id,
                &crate::storage::find_args::ProvenTxReqPartial {
                    status: Some(req_status.clone()),
                    ..Default::default()
                },
                Some(&db_trx),
            )
            .await?;
    }

    // 3. Mark this transaction's OWN outputs unspendable.
    let outputs = storage
        .find_outputs_trx(
            &FindOutputsArgs {
                partial: OutputPartial {
                    transaction_id: Some(tx.transaction_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            Some(&db_trx),
        )
        .await?;
    for output in &outputs {
        storage
            .update_output_trx(
                output.output_id,
                &OutputPartial {
                    spendable: Some(false),
                    ..Default::default()
                },
                Some(&db_trx),
            )
            .await?;
    }

    // 4. Restore consumed inputs to spendable.
    //    InvalidTx: tx was malformed and never hit the chain — restore all inputs.
    //    DoubleSpend: only inputs whose outpoints were verified still-unspent
    //                 (computed above BEFORE the transaction).
    let restored_count: usize = match &effective {
        BroadcastOutcome::DoubleSpend { .. } => {
            let safe_ids = double_spend_safe_ids.as_deref().unwrap_or(&[]);
            for output_id in safe_ids {
                storage
                    .update_output_trx(
                        *output_id,
                        &OutputPartial {
                            spendable: Some(true),
                            ..Default::default()
                        },
                        Some(&db_trx),
                    )
                    .await?;
            }
            safe_ids.len()
        }
        BroadcastOutcome::InvalidTx { .. } => {
            let consumed_inputs = storage
                .find_outputs_trx(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            spent_by: Some(tx.transaction_id),
                            spendable: Some(false),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    Some(&db_trx),
                )
                .await?;
            for input_output in &consumed_inputs {
                storage
                    .update_output_trx(
                        input_output.output_id,
                        &OutputPartial {
                            spendable: Some(true),
                            ..Default::default()
                        },
                        Some(&db_trx),
                    )
                    .await?;
            }
            consumed_inputs.len()
        }
        _ => 0,
    };

    // Commit atomically. If any step above returned Err via `?`, db_trx was
    // dropped without commit and sqlx rolls back automatically.
    storage.commit_transaction(db_trx).await?;

    match &effective {
        BroadcastOutcome::DoubleSpend { .. } => {
            tracing::info!(
                txid = %txid,
                restored_inputs = restored_count,
                "handle_permanent_broadcast_failure: DoubleSpend — restored chain-verified inputs to spendable"
            );
        }
        BroadcastOutcome::InvalidTx { .. } => {
            tracing::info!(
                txid = %txid,
                restored_inputs = restored_count,
                "handle_permanent_broadcast_failure: InvalidTx — restored consumed inputs to spendable"
            );
        }
        _ => {}
    }

    tracing::warn!(
        txid = %txid,
        outcome = ?effective,
        outputs_marked_unspendable = outputs.len(),
        "handle_permanent_broadcast_failure: tx marked failed, outputs marked unspendable"
    );

    Ok(effective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::types::{PostBeefResult, PostTxResultForTxid};

    fn success_result(provider: &str, txid: &str) -> PostBeefResult {
        PostBeefResult {
            name: provider.to_string(),
            status: "success".to_string(),
            error: None,
            txid_results: vec![PostTxResultForTxid {
                txid: txid.to_string(),
                status: "success".to_string(),
                already_known: None,
                double_spend: None,
                block_hash: None,
                block_height: None,
                competing_txs: None,
                service_error: None,
                orphan_mempool: None,
            }],
        }
    }

    fn double_spend_result(provider: &str, txid: &str) -> PostBeefResult {
        PostBeefResult {
            name: provider.to_string(),
            status: "error".to_string(),
            error: Some("DOUBLE_SPEND_ATTEMPTED".to_string()),
            txid_results: vec![PostTxResultForTxid {
                txid: txid.to_string(),
                status: "error".to_string(),
                already_known: None,
                double_spend: Some(true),
                block_hash: None,
                block_height: None,
                competing_txs: Some(vec!["competing_tx_123".to_string()]),
                service_error: None,
                orphan_mempool: None,
            }],
        }
    }

    fn orphan_result(provider: &str, txid: &str) -> PostBeefResult {
        PostBeefResult {
            name: provider.to_string(),
            status: "error".to_string(),
            error: Some("SEEN_IN_ORPHAN_MEMPOOL".to_string()),
            txid_results: vec![PostTxResultForTxid {
                txid: txid.to_string(),
                status: "error".to_string(),
                already_known: None,
                double_spend: None,
                block_hash: None,
                block_height: None,
                competing_txs: None,
                service_error: None,
                orphan_mempool: Some(true),
            }],
        }
    }

    fn service_error_result(provider: &str, txid: &str) -> PostBeefResult {
        PostBeefResult {
            name: provider.to_string(),
            status: "error".to_string(),
            error: Some("timeout".to_string()),
            txid_results: vec![PostTxResultForTxid {
                txid: txid.to_string(),
                status: "error".to_string(),
                already_known: None,
                double_spend: None,
                block_hash: None,
                block_height: None,
                competing_txs: None,
                service_error: Some(true),
                orphan_mempool: None,
            }],
        }
    }

    #[test]
    fn test_success_wins_over_errors() {
        let results = vec![
            service_error_result("ARC", "tx1"),
            success_result("WoC", "tx1"),
        ];
        assert!(matches!(
            classify_broadcast_results(&results),
            BroadcastOutcome::Success
        ));
    }

    #[test]
    fn test_double_spend_takes_precedence() {
        let results = vec![
            double_spend_result("ARC", "tx1"),
            service_error_result("WoC", "tx1"),
        ];
        match classify_broadcast_results(&results) {
            BroadcastOutcome::DoubleSpend {
                competing_txs,
                details,
            } => {
                assert_eq!(competing_txs, vec!["competing_tx_123"]);
                assert!(!details.is_empty());
            }
            other => panic!("Expected DoubleSpend, got {:?}", other),
        }
    }

    #[test]
    fn test_orphan_mempool_classified_correctly() {
        let results = vec![orphan_result("ARC", "tx1")];
        assert!(matches!(
            classify_broadcast_results(&results),
            BroadcastOutcome::OrphanMempool { .. }
        ));
    }

    #[test]
    fn test_all_service_errors() {
        let results = vec![
            service_error_result("ARC", "tx1"),
            service_error_result("WoC", "tx1"),
        ];
        assert!(matches!(
            classify_broadcast_results(&results),
            BroadcastOutcome::ServiceError { .. }
        ));
    }

    #[test]
    fn test_success_wins_over_double_spend() {
        let results = vec![
            double_spend_result("ARC", "tx1"),
            success_result("WoC", "tx1"),
        ];
        assert!(matches!(
            classify_broadcast_results(&results),
            BroadcastOutcome::Success
        ));
    }

    #[test]
    fn test_orphan_not_classified_as_double_spend() {
        let results = vec![orphan_result("ARC", "tx1")];
        match classify_broadcast_results(&results) {
            BroadcastOutcome::DoubleSpend { .. } => {
                panic!("Orphan mempool should NOT be classified as double-spend")
            }
            BroadcastOutcome::OrphanMempool { .. } => {} // correct
            other => panic!("Expected OrphanMempool, got {:?}", other),
        }
    }
}
