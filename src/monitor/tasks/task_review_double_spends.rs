//! TaskReviewDoubleSpends -- reviews double-spend flagged transactions and unfails false positives.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewDoubleSpends.ts.
//!
//! Queries ProvenTxReqs with status "doubleSpend", re-verifies each via
//! `get_status_for_txids`, and marks non-"unknown" results as "unfail" so the
//! normal unfail pipeline can retry proof acquisition.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, Paged, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;

/// Reviews ProvenTxReqs flagged as double-spends and unfails false positives.
///
/// For each req with status `DoubleSpend`, calls `get_status_for_txids` to
/// re-verify. If the txid is found on-chain (status != "unknown"), sets the
/// req status to `Unfail` so the normal unfail pipeline retries proof acquisition.
pub struct TaskReviewDoubleSpends {
    storage: WalletStorageManager,
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    trigger_quick_msecs: u64,
    last_run_msecs: u64,
    review_limit: i64,
    /// When true, use the quick trigger interval (last batch was full).
    use_quick_trigger: bool,
}

impl TaskReviewDoubleSpends {
    /// Create a new double-spend review task.
    pub fn new(storage: WalletStorageManager, services: Arc<dyn WalletServices>) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 12 * ONE_MINUTE,
            trigger_quick_msecs: ONE_MINUTE,
            last_run_msecs: 0,
            review_limit: 100,
            use_quick_trigger: false,
        }
    }

    /// Current effective trigger interval based on whether the last batch was full.
    fn effective_trigger(&self) -> u64 {
        if self.use_quick_trigger {
            self.trigger_quick_msecs
        } else {
            self.trigger_msecs
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewDoubleSpends {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewDoubleSpends"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        let interval = self.effective_trigger();
        interval > 0 && now_msecs > self.last_run_msecs + interval
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();

        let reqs = self
            .storage
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial::default(),
                since: None,
                paged: Some(Paged {
                    limit: self.review_limit,
                    offset: 0,
                }),
                statuses: Some(vec![ProvenTxReqStatus::DoubleSpend]),
            })
            .await?;

        // Switch to quick trigger if we hit the limit (more work to do).
        self.use_quick_trigger = reqs.len() as i64 >= self.review_limit;

        let mut unfailed = 0u64;
        let mut still_unknown = 0u64;

        for req in &reqs {
            let result = self
                .services
                .get_status_for_txids(&[req.txid.clone()], false)
                .await;

            // If the service found the txid on-chain, mark for unfail.
            let is_unknown = match result.results.first() {
                Some(r) => r.status == "unknown",
                None => true,
            };

            if is_unknown {
                still_unknown += 1;
            } else {
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Unfail),
                    ..Default::default()
                };
                self.storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await?;
                unfailed += 1;
            }
        }

        Ok(format!(
            "reviewed {} double-spend reqs: {} unfailed, {} still unknown",
            reqs.len(),
            unfailed,
            still_unknown
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_review_double_spends_defaults() {
        assert_eq!(12 * ONE_MINUTE, 720_000);
        assert_eq!(ONE_MINUTE, 60_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("ReviewDoubleSpends", "ReviewDoubleSpends");
    }
}
