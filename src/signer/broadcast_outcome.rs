//! BroadcastOutcome classifier for post_beef results.
//!
//! Translates raw per-provider broadcast results into a single semantic
//! outcome that the signer can act on. Matches the classification logic
//! from the Calhooon Rust port and TS/Go wallet-toolbox behavior.

use crate::services::types::PostBeefResult;

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
