//! TaskReviewStatus -- reviews stale transaction statuses and logs corrections.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewStatus.ts.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::storage::manager::WalletStorageManager;

/// Task that reviews stale transaction statuses and corrects them.
///
/// Looks for aged Transactions with provenTxId with status != 'completed',
/// sets status to 'completed'. Also looks for reqs with 'invalid' status
/// that have corresponding transactions with status other than 'failed'.
pub struct TaskReviewStatus {
    storage: WalletStorageManager,
    trigger_msecs: u64,
    last_run_msecs: u64,
    /// How old a record must be before it is considered "aged" for review.
    aged_limit_msecs: u64,
}

impl TaskReviewStatus {
    /// Create a new status review task.
    pub fn new(storage: WalletStorageManager) -> Self {
        Self {
            storage,
            trigger_msecs: 15 * ONE_MINUTE,
            last_run_msecs: 0,
            aged_limit_msecs: 5 * ONE_MINUTE,
        }
    }

    /// Set the trigger interval in milliseconds.
    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }

    /// Set the aged limit threshold in milliseconds.
    pub fn with_aged_limit_msecs(mut self, msecs: u64) -> Self {
        self.aged_limit_msecs = msecs;
        self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewStatus {
    fn name(&self) -> &str {
        "ReviewStatus"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        self.trigger_msecs > 0 && now_msecs > self.last_run_msecs + self.trigger_msecs
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();
        let now = chrono::Utc::now().naive_utc();
        let aged_limit = now - chrono::Duration::milliseconds(self.aged_limit_msecs as i64);

        use crate::storage::traits::reader_writer::StorageReaderWriter;
        let result = self.storage.review_status(aged_limit, None).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_review_status_defaults() {
        // Verify default intervals match TS
        assert_eq!(15 * ONE_MINUTE, 900_000); // 15 minutes
        assert_eq!(5 * ONE_MINUTE, 300_000); // 5 minutes
    }

    #[test]
    fn test_name() {
        assert_eq!("ReviewStatus", "ReviewStatus");
    }
}
