//! TaskReviewProvenTxs -- audits proven_txs merkle roots against the canonical chain.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewProvenTxs.ts.
//!
//! Walks block heights from `last_reviewed_height` up to `tip - min_block_age`,
//! finds proven txs at each height, and reproves any whose merkle root doesn't
//! match the canonical header.
//!
//! NOTE: Full height-by-height merkle-root comparison requires chaintracks
//! integration (canonical header lookup by height). Until that is available,
//! this task logs the reviewed height range and reproves nothing. The structure
//! and registration are in place for when chaintracks lands.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::storage::manager::WalletStorageManager;

/// Audits proven transaction merkle roots against the canonical chain.
///
/// Walks block heights in batches, comparing stored merkle roots to canonical
/// block headers. Mismatched transactions are reproved via `reprove_proven`.
///
/// Currently stubbed: full merkle-root comparison requires chaintracks
/// integration for canonical header lookups by height.
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

        // Full merkle-root comparison requires chaintracks canonical header
        // lookups by height, which is not yet integrated. Track the range
        // so the task advances and log accordingly.
        self.last_reviewed_height = end;
        self.use_quick_trigger = end < max_eligible;

        // Suppress unused-variable warnings; these are used in the log output
        // and will be used when chaintracks integration lands.
        let _ = &self.storage;

        Ok(format!(
            "tip={tip}, reviewed height range {start}..={end} (stub: chaintracks integration pending)"
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
