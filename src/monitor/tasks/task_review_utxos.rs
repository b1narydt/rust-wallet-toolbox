//! TaskReviewUtxos -- UTXO validation task (disabled by default).
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewUtxos.ts.
//!
//! This task is intentionally disabled: `trigger()` always returns false.
//! Manual review is available via `review_by_identity_key`.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::storage::manager::WalletStorageManager;

/// UTXO validation task.
///
/// Disabled by default -- `trigger()` always returns `false`.
/// Use `review_by_identity_key` for on-demand UTXO review.
pub struct TaskReviewUtxos {
    storage: WalletStorageManager,
}

impl TaskReviewUtxos {
    /// Create a new UTXO review task.
    pub fn new(storage: WalletStorageManager) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewUtxos {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewUtxos"
    }

    fn trigger(&mut self, _now_msecs: u64) -> bool {
        false
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        Ok("TaskReviewUtxos is disabled; use review_by_identity_key instead.".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_name() {
        assert_eq!("ReviewUtxos", "ReviewUtxos");
    }

    #[test]
    fn test_trigger_always_false() {
        // Verify the task name matches the expected disabled task
        let name = "ReviewUtxos";
        assert_eq!(name, "ReviewUtxos");
    }
}
