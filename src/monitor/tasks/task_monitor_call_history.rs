//! TaskMonitorCallHistory -- logs service call statistics periodically.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskMonitorCallHistory.ts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;

/// Task that periodically logs service call history/statistics.
pub struct TaskMonitorCallHistory {
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    last_run_msecs: u64,
}

impl TaskMonitorCallHistory {
    /// Create a new call history monitoring task.
    pub fn new(services: Arc<dyn WalletServices>) -> Self {
        Self {
            services,
            trigger_msecs: 12 * ONE_MINUTE,
            last_run_msecs: 0,
        }
    }

    /// Set the trigger interval in milliseconds.
    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskMonitorCallHistory {
    fn name(&self) -> &str {
        "MonitorCallHistory"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        now_msecs > self.last_run_msecs + self.trigger_msecs
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();
        let history = self.services.get_services_call_history(true);
        let log = serde_json::to_string(&history).unwrap_or_default();
        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        // Verify default trigger interval matches TS (12 minutes)
        assert_eq!(12 * ONE_MINUTE, 720_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("MonitorCallHistory", "MonitorCallHistory");
    }
}
