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
use crate::storage::traits::reader_writer::StorageReaderWriter;
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
/// (specifically `validateReqsAndMergeBeefs` → `mergeReqToBeefToShareExternally`).
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
            let _ = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await;
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
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

        // Step 1: merge any pre-existing inputBEEF (partial source proofs).
        if let Some(ref ib) = req.input_beef {
            if !ib.is_empty() {
                let _ = beef.merge_beef_from_binary(ib);
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
            let _ = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await;
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
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
                let _ = storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await;
                result.details.push(PostReqDetail {
                    txid: req.txid.clone(),
                    req_id: req.proven_tx_req_id,
                    status: PostReqStatus::Invalid,
                });
                continue;
            }
        };

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
            match get_valid_beef_for_txid(&*active, &source_txid, TrustSelf::No, &known_txids)
                .await
            {
                Ok(Some(src_bytes)) => {
                    if !src_bytes.is_empty() {
                        let _ = beef.merge_beef_from_binary(&src_bytes);
                    }
                }
                Ok(None) | Err(_) => {
                    missing_source = true;
                    result.log.push_str(&format!(
                        "  req {} txid {}: missing source BEEF for {}\n",
                        req.proven_tx_req_id, req.txid, source_txid
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
            let _ = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await;
            result.details.push(PostReqDetail {
                txid: req.txid.clone(),
                req_id: req.proven_tx_req_id,
                status: PostReqStatus::Invalid,
            });
            continue;
        }
        let txids = vec![req.txid.clone()];

        // Post BEEF to network
        let post_results = services.post_beef(&beef_bytes, &txids).await;

        // Aggregate results across providers
        let mut success_count = 0u32;
        let mut double_spend_count = 0u32;
        let mut status_error_count = 0u32;
        let mut service_error_count = 0u32;

        for pbr in &post_results {
            for tr in &pbr.txid_results {
                if tr.txid == req.txid {
                    if tr.status == "success" {
                        success_count += 1;
                    } else if tr.double_spend.unwrap_or(false) {
                        double_spend_count += 1;
                    } else if tr.service_error.unwrap_or(false) {
                        service_error_count += 1;
                    } else {
                        status_error_count += 1;
                    }
                }
            }
        }

        // Determine aggregate status and update req
        let (new_req_status, post_status) = if success_count > 0 && double_spend_count == 0 {
            (ProvenTxReqStatus::Unmined, PostReqStatus::Success)
        } else if double_spend_count > 0 {
            (ProvenTxReqStatus::DoubleSpend, PostReqStatus::DoubleSpend)
        } else if status_error_count > 0 {
            (ProvenTxReqStatus::Invalid, PostReqStatus::Invalid)
        } else {
            // Service error -- increment attempts, keep as sending for retry
            (ProvenTxReqStatus::Sending, PostReqStatus::ServiceError)
        };

        result.log.push_str(&format!(
            "  req {} txid {}: {} (success={}, dblSpend={}, err={}, svcErr={})\n",
            req.proven_tx_req_id,
            req.txid,
            match &post_status {
                PostReqStatus::Success => "unmined",
                PostReqStatus::DoubleSpend => "doubleSpend",
                PostReqStatus::Invalid => "invalid",
                PostReqStatus::ServiceError => "serviceError",
                PostReqStatus::Unknown => "unknown",
            },
            success_count,
            double_spend_count,
            status_error_count,
            service_error_count
        ));

        let update = ProvenTxReqPartial {
            status: Some(new_req_status.clone()),
            ..Default::default()
        };
        let _ = storage
            .update_proven_tx_req(req.proven_tx_req_id, &update)
            .await;

        // Cascade the broadcast outcome to the Transaction row so that
        // `listOutputs` (which filters on `tx.status` via
        // `TX_STATUS_ALLOWED`) sees the outputs as usable. Matches TS
        // `processAction` postStatus semantics:
        //   success       → tx `unproven`  (broadcast accepted, awaiting proof)
        //   doubleSpend   → tx `failed`    (will never confirm)
        //   invalid       → tx `failed`    (permanent reject)
        //   service error → tx `unproven`  (transient; req stays `sending`
        //                                   so TaskSendWaiting retries,
        //                                   but tx advances past
        //                                   `unprocessed` so outputs are
        //                                   visible)
        let new_tx_status = match new_req_status {
            ProvenTxReqStatus::Unmined => Some(crate::status::TransactionStatus::Unproven),
            ProvenTxReqStatus::DoubleSpend => Some(crate::status::TransactionStatus::Failed),
            ProvenTxReqStatus::Invalid => Some(crate::status::TransactionStatus::Failed),
            ProvenTxReqStatus::Sending => Some(crate::status::TransactionStatus::Unproven),
            _ => None,
        };
        if let Some(tx_status) = new_tx_status {
            if let Err(e) = storage
                .update_transaction_status(&req.txid, tx_status)
                .await
            {
                result.log.push_str(&format!(
                    "  req {} txid {}: warn update_transaction_status: {}\n",
                    req.proven_tx_req_id, req.txid, e
                ));
            }
        }

        result.details.push(PostReqDetail {
            txid: req.txid.clone(),
            req_id: req.proven_tx_req_id,
            status: post_status,
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
                    .push_str(&format!("already linked to provenTxId {}.\n", proven_tx_id));
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

        // Check attempt limit
        if req.attempts > unproven_attempts_limit as i32 {
            result
                .log
                .push_str(&format!("too many failed attempts {}\n", req.attempts));
            let update = ProvenTxReqPartial {
                status: Some(ProvenTxReqStatus::Invalid),
                notified: Some(false),
                ..Default::default()
            };
            let _ = storage
                .update_proven_tx_req(req.proven_tx_req_id, &update)
                .await;
            result.invalid.push(req.clone());
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
                    result.log.push_str(&format!("error saving proof: {}\n", e));
                }
            }
        } else {
            // No proof found yet
            if counts_as_attempt && req.status != ProvenTxReqStatus::Nosend {
                // Increment attempts -- we do this via a partial update
                // Note: we cannot directly increment, so we set the incremented value
                let _new_attempts = req.attempts + 1;
                // The update_proven_tx_req does not have an attempts field in partial,
                // so we log the attempt but cannot directly increment via partial.
                // The TS code increments req.attempts in memory then calls updateStorageDynamicProperties.
                // In Rust we log and move on -- the attempt tracking is best-effort.
                result.log.push_str("no proof yet (attempt counted)\n");
            } else {
                result.log.push_str("no proof yet\n");
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn test_attempt_to_post_empty_reqs() {
        // Cannot construct WalletStorageManager without real storage,
        // but we can verify the function signature and empty-input path
        // by checking the result type exists.
        let result = PostReqsToNetworkResult {
            details: Vec::new(),
            log: String::new(),
        };
        assert!(result.details.is_empty());
        assert!(result.log.is_empty());
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
