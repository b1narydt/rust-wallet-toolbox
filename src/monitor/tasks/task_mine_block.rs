//! TaskMineBlock -- test-only task for integration testing.
//!
//! This entire module is behind #[cfg(test)].
//! Advances the chain so other tasks (CheckForProofs, NewHeader) can
//! exercise their logic in integration tests.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_SECOND;

/// Test-only task that mines a block on a mock/regtest chain.
///
/// This is a test utility for integration tests, not production code.
pub struct TaskMineBlock {
    trigger_msecs: u64,
    last_run_msecs: u64,
}

impl TaskMineBlock {
    pub fn new() -> Self {
        Self {
            trigger_msecs: 10 * ONE_SECOND,
            last_run_msecs: 0,
        }
    }

    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskMineBlock {
    fn name(&self) -> &str {
        "MineBlock"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        now_msecs > self.last_run_msecs + self.trigger_msecs
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();
        // In a real test environment, this would call a regtest/mockchain API
        // to mine a block. For now, it is a minimal stub.
        Ok("mine_block: stub".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::ONE_SECOND;

    #[test]
    fn test_mine_block_trigger() {
        let mut task = TaskMineBlock::new();
        assert!(task.trigger(10 * ONE_SECOND + 1)); // should trigger
        assert!(!task.trigger(5 * ONE_SECOND)); // too early
    }

    #[tokio::test]
    async fn test_mine_block_run_task() {
        let mut task = TaskMineBlock::new();
        let result = task.run_task().await.unwrap();
        assert!(result.contains("mine_block"));
    }

    #[test]
    fn test_name() {
        let task = TaskMineBlock::new();
        assert_eq!(task.name(), "MineBlock");
    }
}
