//! TaskReviewProvenTxs -- audits proven_txs merkle roots against the canonical chain.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewProvenTxs.ts.
//!
//! Walks block heights from `last_reviewed_height` up to `tip - min_block_age`,
//! finds proven txs at each height, and reproves any whose merkle root doesn't
//! match the canonical header via the chain tracker.

use std::sync::Arc;

use async_trait::async_trait;
use bsv::transaction::chain_tracker::ChainTracker;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::storage::find_args::{FindProvenTxsArgs, ProvenTxPartial};
use crate::storage::manager::WalletStorageManager;

/// Audits proven transaction merkle roots against the canonical chain.
///
/// Walks block heights in batches, comparing stored merkle roots to canonical
/// block headers via `ChainTracker::is_valid_root_for_height`. Mismatched
/// transactions are reproved via `reprove_proven`.
pub struct TaskReviewProvenTxs {
    storage: WalletStorageManager,
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    trigger_quick_msecs: u64,
    last_run_msecs: u64,
    /// The last block height that was fully reviewed.
    last_reviewed_height: u32,
    /// Maximum number of heights to process per run.
    max_heights_per_run: u32,
    /// Minimum number of blocks behind the tip before a height is eligible.
    min_block_age: u32,
    /// When true, use the quick trigger interval.
    use_quick_trigger: bool,
}

impl TaskReviewProvenTxs {
    /// Create a new proven-tx review task.
    pub fn new(storage: WalletStorageManager, services: Arc<dyn WalletServices>) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 10 * ONE_MINUTE,
            trigger_quick_msecs: ONE_MINUTE,
            last_run_msecs: 0,
            last_reviewed_height: 0,
            max_heights_per_run: 100,
            min_block_age: 100,
            use_quick_trigger: false,
        }
    }

    /// Current effective trigger interval.
    fn effective_trigger(&self) -> u64 {
        if self.use_quick_trigger {
            self.trigger_quick_msecs
        } else {
            self.trigger_msecs
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewProvenTxs {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewProvenTxs"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        let interval = self.effective_trigger();
        interval > 0 && now_msecs > self.last_run_msecs + interval
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();

        let tip = self.services.get_height().await?;
        let max_eligible = tip.saturating_sub(self.min_block_age);

        if max_eligible == 0 || self.last_reviewed_height >= max_eligible {
            self.use_quick_trigger = false;
            return Ok(format!(
                "tip={tip}, max_eligible={max_eligible}, nothing to review"
            ));
        }

        let start = self.last_reviewed_height + 1;
        let end = max_eligible.min(start + self.max_heights_per_run - 1);

        // Obtain chain tracker for merkle root validation.
        let chain_tracker = self.services.get_chain_tracker().await?;

        let mut reviewed_heights = 0u32;
        let mut mismatched_heights = 0u32;
        let mut affected_txs = 0u32;
        let mut reproved_txs = 0u32;
        let mut log = String::new();

        for height in start..=end {
            reviewed_heights += 1;

            // Find all proven txs stored at this height.
            let ptxs = self
                .storage
                .find_proven_txs(&FindProvenTxsArgs {
                    partial: ProvenTxPartial {
                        height: Some(height as i32),
                        ..Default::default()
                    },
                    since: None,
                    paged: None,
                })
                .await?;

            if ptxs.is_empty() {
                continue;
            }

            // Check each proven tx's merkle root against the canonical chain.
            for ptx in &ptxs {
                let is_valid = chain_tracker
                    .is_valid_root_for_height(&ptx.merkle_root, height)
                    .await
                    .unwrap_or(false);

                if is_valid {
                    continue;
                }

                // Merkle root doesn't match canonical — reprove this tx.
                affected_txs += 1;
                if mismatched_heights == 0
                    || log
                        .lines()
                        .last()
                        .map_or(true, |l| !l.contains(&format!("height {height}")))
                {
                    mismatched_heights += 1;
                }

                let result = self.storage.reprove_proven(&ptx, false).await?;
                if result.updated.is_some() {
                    reproved_txs += 1;
                    log += &format!("  reproved txid={} at height {height}\n", ptx.txid);
                } else if result.unavailable {
                    log += &format!("  unavailable txid={} at height {height}\n", ptx.txid);
                }
            }
        }

        self.last_reviewed_height = end;
        self.use_quick_trigger = end < max_eligible;

        Ok(format!(
            "tip={tip}, reviewed heights {start}..={end}: \
             {reviewed_heights} checked, {mismatched_heights} mismatched, \
             {affected_txs} affected, {reproved_txs} reproved\n{log}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_review_proven_txs_defaults() {
        assert_eq!(10 * ONE_MINUTE, 600_000);
        assert_eq!(ONE_MINUTE, 60_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("ReviewProvenTxs", "ReviewProvenTxs");
    }
}
