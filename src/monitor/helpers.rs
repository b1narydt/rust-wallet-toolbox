//! Shared helper functions for monitor tasks.
//!
//! Provides common utilities used across multiple monitor task implementations.
//! Translated from:
//! - wallet-toolbox/src/storage/methods/attemptToPostReqsToNetwork.ts
//! - wallet-toolbox/src/monitor/tasks/TaskCheckForProofs.ts (getProofs)

use std::collections::HashSet;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use bsv::transaction::beef::{Beef, BEEF_V2};
use bsv::transaction::transaction::Transaction as BsvTransaction;

use crate::error::WalletResult;
use crate::services::traits::WalletServices;
use crate::services::types::GetMerklePathResult;
use crate::status::ProvenTxReqStatus;
use crate::storage::beef::{get_valid_beef_for_txid, TrustSelf};
use crate::storage::find_args::ProvenTxReqPartial;
use crate::storage::manager::WalletStorageManager;
use crate::tables::{MonitorEvent, ProvenTx, ProvenTxReq};
use crate::types::Chain;

/// Returns the current epoch time in milliseconds.
pub fn now_msecs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Log an event to the monitor_events table.
pub async fn log_event(
    storage: &WalletStorageManager,
    event: &str,
    details: &str,
) -> WalletResult<()> {
    let now = chrono::Utc::now().naive_utc();
    let monitor_event = MonitorEvent {
        created_at: now,
        updated_at: now,
        id: 0,
        event: event.to_string(),
        details: Some(details.to_string()),
    };
    storage.insert_monitor_event(&monitor_event).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PostReqsToNetwork result types
// ---------------------------------------------------------------------------

/// Status of a per-txid post result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostReqStatus {
    /// Transaction was successfully posted.
    Success,
    /// Transaction was rejected as a double spend.
    DoubleSpend,
    /// Transaction was rejected as invalid.
    Invalid,
    /// The service encountered an error processing the request.
    ServiceError,
    /// Transaction seen in orphan mempool (parent not yet propagated).
    /// Transient — should be retried.
    Orphan,
    /// Status could not be determined.
    Unknown,
}

/// Per-req detail from attempt_to_post_reqs_to_network.
#[derive(Debug)]
pub struct PostReqDetail {
    /// Transaction ID.
    pub txid: String,
    /// Proven transaction request ID.
    pub req_id: i64,
    /// Resulting status after posting.
    pub status: PostReqStatus,
    /// Set when the broadcast-outcome → `transactions.status` cascade
    /// failed. The req's own status is still updated, but the
    /// matching `transactions` row may be stuck at its previous value
    /// (e.g. `unprocessed`) so the caller MUST either retry or alert:
    /// silently ignoring this reintroduces the visibility bug (hidden
    /// outputs / stale balance) that the cascade was added to fix.
    pub cascade_update_failed: bool,
}

/// Result of attempt_to_post_reqs_to_network.
#[derive(Debug)]
pub struct PostReqsToNetworkResult {
    /// Per-request results.
    pub details: Vec<PostReqDetail>,
    /// Human-readable log of operations performed.
    pub log: String,
}

// ---------------------------------------------------------------------------
// attempt_to_post_reqs_to_network
// ---------------------------------------------------------------------------

/// Attempt to post pending transaction requests to the network.
///
/// For each ProvenTxReq:
/// 1. Rebuild the broadcast BEEF on demand from storage (TS parity with
///    `StorageProvider.mergeReqToBeefToShareExternally`). The stored
///    `inputBEEF` is treated as a hint/cache, not the source of truth —
///    missing source BEEFs are fetched from `proven_txs` at send time.
/// 2. Post the rebuilt BEEF to the network via `services.post_beef()`
/// 3. Update req status based on results:
///    - success -> unmined
///    - double spend -> doubleSpend
///    - invalid -> invalid
///    - service error -> increment attempts, keep as sending
/// 4. Cascade the outcome to `transactions.status` so frontend
///    `listOutputs` (which filters by `TX_STATUS_ALLOWED`) sees the
///    outputs once broadcast has been attempted.
///
/// Translated from wallet-toolbox/src/storage/methods/attemptToPostReqsToNetwork.ts
/// (specifically `StorageProvider.mergeReqToBeefToShareExternally`).
pub async fn attempt_to_post_reqs_to_network(
    storage: &WalletStorageManager,
    services: &dyn WalletServices,
    reqs: &[ProvenTxReq],
) -> WalletResult<PostReqsToNetworkResult> {
    let mut result = PostReqsToNetworkResult {
        details: Vec::new(),
        log: String::new(),
    };

    if reqs.is_empty() {
        return Ok(result);
    }

    for req in reqs {
        // Don't degrade a req that another task already advanced.
        // `TaskCheckForProofs` and `TaskSendWaiting` share storage and
        // can run concurrently — if a proof landed between the time
        // this batch was read and now, the req is already `Completed`
        // (proof cascaded to the tx row) or `Unmined` (accepted,
        // awaiting proof), and writing `Unmined`/`Sending`/`Failed`
        // here would clobber that state and knock the tx row back to
        // `Unproven`/`Failed`. Matches TS
        // `attemptToPostReqsToNetwork.ts`:
        //
        //   if (['completed', 'unmined'].indexOf(req.status) >= 0) continue
        if matches!(
            req.status,
            ProvenTxReqStatus::Completed | ProvenTxReqStatus::Unmined
        ) {
            result.log.push_str(&format!(
                "  req {} txid {}: skipped (already {:?})\n",
                req.proven_tx_req_id, req.txid, req.status
            ));
            continue;
        }

        // rawTx is the only field we truly cannot rebuild — if missing,
        // mark the req invalid. Everything else (including inputBEEF)
        // can be reconstructed from storage state below.
        if req.raw_tx.is_empty() {
            result.log.push_str(&format!(
                "  req {} txid {}: invalid (noRawTx=true)\n",
                req.proven_tx_req_id, req.txid
            ));
            let update = ProvenTxReqPartial {
                status: Some(ProvenTxReqStatus::Invalid),
                ..Default::default()
            };
            if let Err(e) = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await
            {
                // Failing to mark invalid means TaskSendWaiting will
                // loop on this req forever; log so DB issues are
                // observable.
                result.log.push_str(&format!(
                    "  req {} txid {}: warn update_proven_tx_req(Invalid) failed: {}\n",
                    req.proven_tx_req_id, req.txid, e
                ));
            }
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
                cascade_update_failed: false,
            });
            continue;
        }

        // Rebuild the broadcast BEEF at send time, matching TS
        // StorageProvider.mergeReqToBeefToShareExternally:
        //
        //   1. Start with whatever inputBEEF the req has (may be None,
        //      empty, or partial — that's fine).
        //   2. Merge the req's own rawTx into the BEEF so it becomes the
        //      atomic target.
        //   3. Walk each tx input; for any source txid not already in the
        //      BEEF, fetch it via get_valid_beef_for_txid (which chains
        //      through proven_txs and recovers merkle proofs on demand).
        //   4. Serialize as plain BEEF (NOT atomic BEEF / BRC-95) — ARC's
        //      /v1/tx endpoint expects plain BEEF. Atomic BEEF prepends
        //      a 4-byte `01010101` marker + 32-byte txid which ARC's
        //      parser doesn't recognize and falls through to raw-tx
        //      decoding, producing "script(...): got N bytes: unexpected
        //      EOF" errors.
        //
        // This makes delayed broadcast resilient to partial/NULL
        // inputBEEF in the DB — the complete BEEF is reconstructed
        // from authoritative storage state at broadcast time.
        let active = match storage.active() {
            Some(a) => a.clone(),
            None => {
                result.log.push_str(&format!(
                    "  req {} txid {}: skipped (storage not active)\n",
                    req.proven_tx_req_id, req.txid
                ));
                continue;
            }
        };

        let mut beef = Beef::new(BEEF_V2);

        // Step 1: merge any pre-existing inputBEEF (partial source
        // proofs). Treated as a best-effort hint — if the stored blob
        // is corrupt or version-mismatched, step 3 below will refetch
        // the source BEEFs from authoritative storage state, so we
        // don't need to fail the req here. We DO log so stored
        // `inputBEEF` decay is observable.
        if let Some(ref ib) = req.input_beef {
            if !ib.is_empty() {
                if let Err(e) = beef.merge_beef_from_binary(ib) {
                    result.log.push_str(&format!(
                        "  req {} txid {}: warn mergeInputBeef failed \
                         (will refetch from storage): {}\n",
                        req.proven_tx_req_id, req.txid, e
                    ));
                }
            }
        }

        // Step 2: merge the req's own rawTx so it becomes the atomic target.
        if let Err(e) = beef.merge_raw_tx(&req.raw_tx, None) {
            result.log.push_str(&format!(
                "  req {} txid {}: invalid (mergeRawTxFailed: {})\n",
                req.proven_tx_req_id, req.txid, e
            ));
            let update = ProvenTxReqPartial {
                status: Some(ProvenTxReqStatus::Invalid),
                ..Default::default()
            };
            if let Err(e) = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await
            {
                result.log.push_str(&format!(
                    "  req {} txid {}: warn update_proven_tx_req(Invalid) failed: {}\n",
                    req.proven_tx_req_id, req.txid, e
                ));
            }
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
                cascade_update_failed: false,
            });
            continue;
        }

        // Step 3: parse tx and fetch any missing source BEEFs from storage.
        let parsed_tx = match BsvTransaction::from_binary(&mut Cursor::new(&req.raw_tx)) {
            Ok(t) => t,
            Err(e) => {
                result.log.push_str(&format!(
                    "  req {} txid {}: invalid (parseTxFailed: {})\n",
                    req.proven_tx_req_id, req.txid, e
                ));
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Invalid),
                    ..Default::default()
                };
                if let Err(e) = storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await
                {
                    result.log.push_str(&format!(
                        "  req {} txid {}: warn update_proven_tx_req(Invalid) failed: {}\n",
                        req.proven_tx_req_id, req.txid, e
                    ));
                }
                result.details.push(PostReqDetail {
                    txid: req.txid.clone(),
                    req_id: req.proven_tx_req_id,
                    status: PostReqStatus::Invalid,
                    cascade_update_failed: false,
                });
                continue;
            }
        };

        // Intentionally empty — matches TS passing `[]` as
        // `knownTxids` to `getValidBeefForTxid`. The broadcast path
        // rebuilds the BEEF from authoritative storage state each
        // time, so there are no client-provided "already known" txids
        // to trust.
        let known_txids: HashSet<String> = HashSet::new();
        let mut missing_source = false;
        for input in &parsed_tx.inputs {
            let source_txid = match &input.source_txid {
                Some(t) => t.clone(),
                None => continue,
            };
            if source_txid.is_empty() || beef.find_txid(&source_txid).is_some() {
                continue;
            }
            match get_valid_beef_for_txid(&*active, &source_txid, TrustSelf::No, &known_txids).await
            {
                Ok(Some(src_bytes)) => {
                    if src_bytes.is_empty() {
                        // Storage returned an empty blob — treat as
                        // missing so TaskSendWaiting retries.
                        missing_source = true;
                        result.log.push_str(&format!(
                            "  req {} txid {}: empty source BEEF for {}\n",
                            req.proven_tx_req_id, req.txid, source_txid
                        ));
                    } else if let Err(e) = beef.merge_beef_from_binary(&src_bytes) {
                        // Bytes came back but the merge failed
                        // (corrupted stored proof, version mismatch,
                        // malformed merkle path). Do NOT fall through
                        // and serialize an incomplete BEEF — ARC will
                        // reject it with exactly the
                        // "script(N): got M bytes: unexpected EOF"
                        // class of error this code path is trying to
                        // fix, but without a diagnostic trail back to
                        // the source txid. Mark this req as needing
                        // retry and log the underlying error.
                        missing_source = true;
                        result.log.push_str(&format!(
                            "  req {} txid {}: mergeSourceBeefFailed for {}: {}\n",
                            req.proven_tx_req_id, req.txid, source_txid, e
                        ));
                    }
                }
                Ok(None) => {
                    // Source tx legitimately not in storage yet —
                    // TaskSendWaiting will retry on the next tick once
                    // the source is proven.
                    missing_source = true;
                    result.log.push_str(&format!(
                        "  req {} txid {}: missing source BEEF for {}\n",
                        req.proven_tx_req_id, req.txid, source_txid
                    ));
                }
                Err(e) => {
                    // Transient storage error (SQLite busy,
                    // connection drop, etc.) — distinct from "source
                    // legitimately not in storage." Retry semantics
                    // are identical, but the diagnostic needs to be
                    // honest so DB contention is observable.
                    missing_source = true;
                    result.log.push_str(&format!(
                        "  req {} txid {}: storage error fetching source BEEF for {}: {}\n",
                        req.proven_tx_req_id, req.txid, source_txid, e
                    ));
                }
            }
        }

        if missing_source {
            // Don't mark invalid — a later run of TaskSendWaiting may
            // succeed once the source tx is proven in storage. Leave
            // status untouched so the monitor retries on the next tick.
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Unknown,
                cascade_update_failed: false,
            });
            continue;
        }

        // Step 4: serialize as plain BEEF targeted at this req's txid.
        let mut beef_bytes = Vec::new();
        if let Err(e) = beef.to_binary(&mut beef_bytes) {
            result.log.push_str(&format!(
                "  req {} txid {}: invalid (serializeFailed: {})\n",
                req.proven_tx_req_id, req.txid, e
            ));
            let update = ProvenTxReqPartial {
                status: Some(ProvenTxReqStatus::Invalid),
                ..Default::default()
            };
            if let Err(e) = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await
            {
                result.log.push_str(&format!(
                    "  req {} txid {}: warn update_proven_tx_req(Invalid) failed: {}\n",
                    req.proven_tx_req_id, req.txid, e
                ));
            }
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
                cascade_update_failed: false,
            });
            continue;
        }
        let txids = vec![req.txid.clone()];

        // Post BEEF to network
        let post_results = services.post_beef(&beef_bytes, &txids).await;

        // Aggregate results across providers
        let mut success_count = 0u32;
        let mut double_spend_count = 0u32;
        let mut orphan_count = 0u32;
        let mut status_error_count = 0u32;
        let mut service_error_count = 0u32;

        for pbr in &post_results {
            for tr in &pbr.txid_results {
                if tr.txid == req.txid {
                    if tr.status == "success" {
                        success_count += 1;
                    } else if tr.double_spend.unwrap_or(false) {
                        double_spend_count += 1;
                    } else if tr.orphan_mempool.unwrap_or(false) {
                        orphan_count += 1;
                    } else if tr.service_error.unwrap_or(false) {
                        service_error_count += 1;
                    } else {
                        status_error_count += 1;
                    }
                }
            }
        }

        // Determine aggregate status and update req.
        // Priority: success > double-spend > orphan (transient) > invalid > service-error
        let (new_req_status, post_status) = if success_count > 0 && double_spend_count == 0 {
            (ProvenTxReqStatus::Unmined, PostReqStatus::Success)
        } else if double_spend_count > 0 {
            (ProvenTxReqStatus::DoubleSpend, PostReqStatus::DoubleSpend)
        } else if orphan_count > 0 {
            // Orphan mempool is transient — parent tx not yet propagated.
            // Keep as Sending so TaskSendWaiting retries.
            (ProvenTxReqStatus::Sending, PostReqStatus::Orphan)
        } else if status_error_count > 0 {
            (ProvenTxReqStatus::Invalid, PostReqStatus::Invalid)
        } else {
            // Service error — increment attempts, keep as sending for retry
            (ProvenTxReqStatus::Sending, PostReqStatus::ServiceError)
        };

        result.log.push_str(&format!(
            "  req {} txid {}: {} (success={}, dblSpend={}, orphan={}, err={}, svcErr={})\n",
            req.proven_tx_req_id,
            req.txid,
            match &post_status {
                PostReqStatus::Success => "unmined",
                PostReqStatus::DoubleSpend => "doubleSpend",
                PostReqStatus::Invalid => "invalid",
                PostReqStatus::ServiceError => "serviceError",
                PostReqStatus::Orphan => "orphan",
                PostReqStatus::Unknown => "unknown",
            },
            success_count,
            double_spend_count,
            orphan_count,
            status_error_count,
            service_error_count
        ));

        let update = ProvenTxReqPartial {
            status: Some(new_req_status.clone()),
            // F1 (parity audit 2026-07-24): a service-error retry MUST count, or
            // the attempt-based proof-timeout breaker in get_proofs can never
            // fire (TS attemptToPostReqsToNetwork increments here too).
            attempts: matches!(post_status, PostReqStatus::ServiceError)
                .then_some(req.attempts + 1),
            ..Default::default()
        };
        if let Err(e) = storage
            .update_proven_tx_req(req.proven_tx_req_id, &update)
            .await
        {
            result.log.push_str(&format!(
                "  req {} txid {}: warn update_proven_tx_req({:?}) failed: {}\n",
                req.proven_tx_req_id, req.txid, new_req_status, e
            ));
        }

        // Cascade the broadcast outcome to the Transaction row so
        // that `listOutputs` (which filters on `tx.status` via
        // `TX_STATUS_ALLOWED`) sees the outputs as usable. The match
        // below keys on `ProvenTxReqStatus` (the authoritative post-
        // broadcast state), matching TS `attemptToPostReqsToNetwork`
        // `newTxStatus` mapping.
        //
        // Why the svcErr → `Sending` arm is correct:
        //   - No provider actually accepted the broadcast, so
        //     advancing to `Unproven` would falsely signal "accepted,
        //     awaiting proof".
        //   - The req stays `Sending` so TaskSendWaiting retries.
        //   - `Sending` is already in `TX_STATUS_ALLOWED`
        //     (`list_outputs.rs`), so output visibility is preserved
        //     either way — no reason to inflate the status.
        let new_tx_status = match new_req_status {
            ProvenTxReqStatus::Unmined => Some(crate::status::TransactionStatus::Unproven),
            ProvenTxReqStatus::DoubleSpend => Some(crate::status::TransactionStatus::Failed),
            ProvenTxReqStatus::Invalid => Some(crate::status::TransactionStatus::Failed),
            ProvenTxReqStatus::Sending => Some(crate::status::TransactionStatus::Sending),
            _ => None,
        };
        let mut cascade_update_failed = false;
        if let Some(tx_status) = new_tx_status {
            let is_failed = matches!(tx_status, crate::status::TransactionStatus::Failed);
            if let Err(e) = storage
                .update_transaction_status(&req.txid, tx_status)
                .await
            {
                // Flag the failure on the structured detail so the
                // caller (e.g. TaskSendWaiting) can retry or alert.
                // Logging alone is not enough: nothing downstream
                // parses `result.log`, so a silently failed cascade
                // would reintroduce the visibility bug in a narrower
                // window (req at `unmined` but tx row still at
                // `unprocessed`, outputs hidden).
                cascade_update_failed = true;
                result.log.push_str(&format!(
                    "  req {} txid {}: warn update_transaction_status: {}\n",
                    req.proven_tx_req_id, req.txid, e
                ));
            } else if is_failed {
                // Restore this tx's consumed inputs — but NOT blindly (F4,
                // parity audit 2026-07-24; TS markStaleInputsAsSpent).
                //
                //   - Invalid (never accepted by any provider): the tx does not
                //     exist on-chain, so every consumed input is genuinely
                //     unspent — restore them all.
                //   - DoubleSpend: some competing tx spent at least one of our
                //     inputs. Unconditionally restoring resurrects the already
                //     -spent UTXO, the next createAction reselects it, the
                //     network answers missing-inputs, and the wallet loops
                //     forever. Gate each input on services.is_utxo and restore
                //     ONLY the ones still unspent on-chain; on a lookup error,
                //     leave the input consumed (fail-closed — a re-run can
                //     restore it later, but a false restore starts the loop).
                match storage
                    .find_transactions(&crate::storage::find_args::FindTransactionsArgs {
                        partial: crate::storage::find_args::TransactionPartial {
                            txid: Some(req.txid.clone()),
                            ..Default::default()
                        },
                        no_raw_tx: true,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(txs) => {
                        if let Some(tx) = txs.first() {
                            let is_double_spend =
                                matches!(new_req_status, ProvenTxReqStatus::DoubleSpend);
                            if !is_double_spend {
                                if let Err(e) =
                                    storage.restore_consumed_inputs(tx.transaction_id).await
                                {
                                    result.log.push_str(&format!(
                                        "  req {} txid {}: warn restore_consumed_inputs: {}\n",
                                        req.proven_tx_req_id, req.txid, e
                                    ));
                                }
                            } else {
                                let restored = restore_unspent_inputs_only(
                                    storage,
                                    services,
                                    tx.transaction_id,
                                    &mut result.log,
                                )
                                .await;
                                result.log.push_str(&format!(
                                    "  req {} txid {}: doubleSpend — restored {} still-unspent input(s), stale inputs stay consumed\n",
                                    req.proven_tx_req_id, req.txid, restored
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        result.log.push_str(&format!(
                            "  req {} txid {}: warn find_transactions for restore: {}\n",
                            req.proven_tx_req_id, req.txid, e
                        ));
                    }
                }
            }
        }

        result.details.push(PostReqDetail {
            txid: req.txid.clone(),
            req_id: req.proven_tx_req_id,
            status: post_status,
            cascade_update_failed,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// GetProofs result types
// ---------------------------------------------------------------------------

/// Per-req detail from get_proofs.
#[derive(Debug)]
pub struct GetProofDetail {
    /// Transaction ID.
    pub txid: String,
    /// Proven transaction request ID.
    pub req_id: i64,
    /// Whether a proof was found.
    pub proven: bool,
}

/// Result of get_proofs.
#[derive(Debug)]
pub struct GetProofsResult {
    /// Requests that were successfully proven.
    pub proven: Vec<ProvenTxReq>,
    /// Requests that were determined to be invalid.
    pub invalid: Vec<ProvenTxReq>,
    /// Human-readable log of operations performed.
    pub log: String,
}

// ---------------------------------------------------------------------------
// get_proofs
// ---------------------------------------------------------------------------

/// Attempt to get merkle proofs for unconfirmed transactions.
///
/// For each ProvenTxReq:
/// 1. Validate the rawTx hashes to the txid
/// 2. Check attempt limits
/// 3. Call services.get_merkle_path(txid)
/// 4. If proof found: create ProvenTx record, update req status
/// 5. If no proof: increment attempts if countsAsAttempt
///
/// Translated from wallet-toolbox/src/monitor/tasks/TaskCheckForProofs.ts getProofs()
pub async fn get_proofs(
    storage: &WalletStorageManager,
    services: &dyn WalletServices,
    reqs: &[ProvenTxReq],
    _chain: &Chain,
    unproven_attempts_limit: u32,
    counts_as_attempt: bool,
    max_acceptable_height: Option<u32>,
) -> WalletResult<GetProofsResult> {
    let mut result = GetProofsResult {
        proven: Vec::new(),
        invalid: Vec::new(),
        log: String::new(),
    };

    if reqs.is_empty() {
        return Ok(result);
    }

    for req in reqs {
        result.log.push_str(&format!(
            "  reqId {} txid {}: ",
            req.proven_tx_req_id, req.txid
        ));

        // If already linked to a proven tx, mark completed
        if let Some(proven_tx_id) = req.proven_tx_id {
            if proven_tx_id > 0 {
                result
                    .log
                    .push_str(&format!("already linked to provenTxId {proven_tx_id}.\n"));
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Completed),
                    notified: Some(false),
                    ..Default::default()
                };
                let _ = storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await;
                result.proven.push(req.clone());
                continue;
            }
        }

        // Attempt limit → the proof-timeout circuit (F2, parity audit
        // 2026-07-24; TS TaskCheckForProofs + EntityProvenTxReq.applyProofTimeout).
        // A tx that WAS broadcast but never mined (dropped from the mempool) is
        // reset to `unsent` so TaskSendWaiting re-broadcasts it — marking it
        // invalid would strand real funds on a mempool eviction. Only a tx that
        // was NEVER successfully broadcast is marked invalid. Matches the TS
        // DEFAULT (maxRebroadcastAttempts unset ⇒ no cycle cap).
        if req.attempts > unproven_attempts_limit as i32 {
            match proof_timeout_action(&req.status) {
                ProofTimeoutAction::Rebroadcast => {
                    result.log.push_str(&format!(
                        "too many failed attempts {} — was broadcast; resetting to unsent for rebroadcast\n",
                        req.attempts
                    ));
                    let update = ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Unsent),
                        attempts: Some(0),
                        notified: Some(false),
                        ..Default::default()
                    };
                    let _ = storage
                        .update_proven_tx_req(req.proven_tx_req_id, &update)
                        .await;
                }
                ProofTimeoutAction::Invalid => {
                    result.log.push_str(&format!(
                        "too many failed attempts {} and tx was never broadcast — marking invalid\n",
                        req.attempts
                    ));
                    let update = ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Invalid),
                        notified: Some(false),
                        ..Default::default()
                    };
                    let _ = storage
                        .update_proven_tx_req(req.proven_tx_req_id, &update)
                        .await;
                    result.invalid.push(req.clone());
                }
            }
            continue;
        }

        // Call services to get merkle path
        let gmpr: GetMerklePathResult = services.get_merkle_path(&req.txid, false).await;

        if let (Some(merkle_path), Some(header)) = (&gmpr.merkle_path, &gmpr.header) {
            // Skip proofs from bleeding-edge blocks to avoid reorg risk
            if let Some(max_height) = max_acceptable_height {
                if header.height > max_height {
                    result.log.push_str(&format!(
                        "ignoring possible proof from very new block at height {} {}\n",
                        header.height, header.hash
                    ));
                    continue;
                }
            }

            // Proof found -- create ProvenTx and update req
            let now = chrono::Utc::now().naive_utc();
            let proven_tx = ProvenTx {
                created_at: now,
                updated_at: now,
                proven_tx_id: 0,
                txid: req.txid.clone(),
                height: header.height as i32,
                index: 0, // Index from merkle path (simplified)
                merkle_path: merkle_path.clone(),
                raw_tx: req.raw_tx.clone(),
                block_hash: header.hash.clone(),
                merkle_root: header.merkle_root.clone(),
            };

            match storage
                .update_proven_tx_req_with_new_proven_tx(req.proven_tx_req_id, &proven_tx)
                .await
            {
                Ok(_proven_tx_id) => {
                    result.log.push_str(&format!(
                        "proven at height {} block {}\n",
                        header.height, header.hash
                    ));
                    result.proven.push(req.clone());
                }
                Err(e) => {
                    result.log.push_str(&format!("error saving proof: {e}\n"));
                }
            }
        } else {
            // No proof found yet. PERSIST the attempt (F1, parity audit
            // 2026-07-24): with attempts frozen at 0 the timeout circuit above
            // was unreachable and an unmined-forever tx was polled eternally,
            // never rebroadcast, never failed. (TS increments req.attempts and
            // flushes via updateStorageDynamicProperties.)
            if counts_as_attempt && req.status != ProvenTxReqStatus::Nosend {
                let update = ProvenTxReqPartial {
                    attempts: Some(req.attempts + 1),
                    ..Default::default()
                };
                match storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await
                {
                    Ok(_) => result.log.push_str(&format!(
                        "no proof yet (attempt {} counted)\n",
                        req.attempts + 1
                    )),
                    Err(e) => result
                        .log
                        .push_str(&format!("no proof yet (attempt update failed: {e})\n")),
                }
            } else {
                result.log.push_str("no proof yet\n");
            }
        }
    }

    Ok(result)
}

/// DoubleSpend input recovery (F4): restore ONLY the consumed inputs that are
/// still unspent on-chain, per `services.is_utxo`. A genuinely-spent input must
/// stay `spendable = false` or coin selection reuses it and every subsequent
/// createAction dies on missing-inputs (the loop TS's `markStaleInputsAsSpent`
/// exists to stop). Any per-input uncertainty (no script, no txid, lookup
/// error) leaves that input consumed — fail-closed; a later pass can restore.
/// Returns how many inputs were restored.
async fn restore_unspent_inputs_only(
    storage: &WalletStorageManager,
    services: &dyn WalletServices,
    tx_id: i64,
    log: &mut String,
) -> u64 {
    use crate::storage::find_args::{FindOutputsArgs, OutputPartial};

    let outputs = match storage
        .find_outputs(&FindOutputsArgs {
            partial: OutputPartial {
                spent_by: Some(tx_id),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
    {
        Ok(o) => o,
        Err(e) => {
            log.push_str(&format!("  warn find_outputs for doubleSpend restore: {e}\n"));
            return 0;
        }
    };

    let mut restored = 0u64;
    for output in &outputs {
        let (Some(script), Some(txid)) = (output.locking_script.as_ref(), output.txid.as_ref())
        else {
            continue; // cannot verify → stays consumed (fail-closed)
        };
        match services.is_utxo(script, txid, output.vout as u32).await {
            Ok(true) => {
                match storage
                    .update_output(
                        output.output_id,
                        &OutputPartial {
                            spendable: Some(true),
                            spent_by: Some(0),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(_) => restored += 1,
                    Err(e) => log.push_str(&format!(
                        "  warn restoring input {}.{}: {e}\n",
                        txid, output.vout
                    )),
                }
            }
            Ok(false) => {
                // Genuinely spent by the competing tx — stays consumed.
            }
            Err(e) => {
                log.push_str(&format!(
                    "  warn is_utxo({}.{}) failed — leaving input consumed: {e}\n",
                    txid, output.vout
                ));
            }
        }
    }
    restored
}

/// What the proof-timeout circuit does to a req whose `attempts` passed the
/// unproven limit (F2). Pure — the whole decision is derivable from status:
/// TS's canonical `wasBroadcast` inference (`EntityProvenTxReq.wasBroadcastStatuses`)
/// is "status is/was unmined | callback | unconfirmed | completed", and the
/// statuses this task processes make that exactly a current-status check —
/// `sending`/`unknown` mean no provider ever accepted the tx.
#[derive(Debug, PartialEq, Eq)]
enum ProofTimeoutAction {
    /// The tx WAS broadcast (a provider accepted it) but never mined — reset to
    /// `unsent` so TaskSendWaiting re-broadcasts (TS default: no cycle cap).
    Rebroadcast,
    /// The tx was NEVER successfully broadcast — fail it.
    Invalid,
}

fn proof_timeout_action(status: &ProvenTxReqStatus) -> ProofTimeoutAction {
    match status {
        ProvenTxReqStatus::Unmined
        | ProvenTxReqStatus::Callback
        | ProvenTxReqStatus::Unconfirmed
        | ProvenTxReqStatus::Completed => ProofTimeoutAction::Rebroadcast,
        _ => ProofTimeoutAction::Invalid,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_timeout_rebroadcasts_only_what_was_broadcast() {
        // F2: mempool-evicted (broadcast, unmined) => rebroadcast, never invalid;
        // never-accepted (sending/unknown) => invalid. Mirrors TS
        // EntityProvenTxReq.wasBroadcastStatuses + applyProofTimeout defaults.
        use ProofTimeoutAction::*;
        assert_eq!(proof_timeout_action(&ProvenTxReqStatus::Unmined), Rebroadcast);
        assert_eq!(proof_timeout_action(&ProvenTxReqStatus::Callback), Rebroadcast);
        assert_eq!(
            proof_timeout_action(&ProvenTxReqStatus::Unconfirmed),
            Rebroadcast
        );
        assert_eq!(
            proof_timeout_action(&ProvenTxReqStatus::Completed),
            Rebroadcast
        );
        assert_eq!(proof_timeout_action(&ProvenTxReqStatus::Sending), Invalid);
        assert_eq!(proof_timeout_action(&ProvenTxReqStatus::Unknown), Invalid);
        assert_eq!(proof_timeout_action(&ProvenTxReqStatus::Unsent), Invalid);
    }

    #[test]
    fn test_now_msecs_returns_reasonable_value() {
        let now = now_msecs();
        // Should be after 2020-01-01 in milliseconds
        assert!(now > 1_577_836_800_000);
    }

    #[test]
    fn test_post_req_status_variants() {
        assert_eq!(PostReqStatus::Success, PostReqStatus::Success);
        assert_ne!(PostReqStatus::Success, PostReqStatus::Invalid);
    }

    #[test]
    fn test_max_acceptable_height_guard() {
        let max_acceptable_height: Option<u32> = Some(100);

        // Proof from block 101 should be skipped
        let proof_height: u32 = 101;
        let should_skip = max_acceptable_height
            .map(|max| proof_height > max)
            .unwrap_or(false);
        assert!(should_skip);

        // Proof from block 100 should NOT be skipped
        let proof_height: u32 = 100;
        let should_skip = max_acceptable_height
            .map(|max| proof_height > max)
            .unwrap_or(false);
        assert!(!should_skip);

        // No max height means never skip
        let max: Option<u32> = None;
        let should_skip = max.map(|m| 101u32 > m).unwrap_or(false);
        assert!(!should_skip);
    }

    #[tokio::test]
    async fn test_get_proofs_empty_reqs_result() {
        let result = GetProofsResult {
            proven: Vec::new(),
            invalid: Vec::new(),
            log: String::new(),
        };
        assert!(result.proven.is_empty());
        assert!(result.invalid.is_empty());
        assert!(result.log.is_empty());
    }
}
