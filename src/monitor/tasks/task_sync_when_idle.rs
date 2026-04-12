//! TaskSyncWhenIdle -- stub task that always returns false from trigger.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskSyncWhenIdle.ts.
//! The TS implementation is a TODO/stub -- trigger always returns false.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::task_trait::WalletMonitorTask;

/// Stub task for future sync-when-idle functionality.
///
/// Trigger always returns false -- this task never runs.
pub struct TaskSyncWhenIdle;

impl Default for TaskSyncWhenIdle {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSyncWhenIdle {
    /// Create a new sync-when-idle task (currently a no-op stub).
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskSyncWhenIdle {
    fn name(&self) -> &str {
        "SyncWhenIdle"
    }

    fn trigger(&mut self, _now_msecs: u64) -> bool {
        false
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        Ok("sync not implemented".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_when_idle_never_triggers() {
        let mut task = TaskSyncWhenIdle::new();
        assert!(!task.trigger(0));
        assert!(!task.trigger(u64::MAX));
    }

    #[tokio::test]
    async fn test_sync_when_idle_run_task() {
        let mut task = TaskSyncWhenIdle::new();
        let result = task.run_task().await.unwrap();
        assert_eq!(result, "sync not implemented");
    }

    #[test]
    fn test_name() {
        let task = TaskSyncWhenIdle::new();
        assert_eq!(task.name(), "SyncWhenIdle");
    }
}
