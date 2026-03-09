//! TaskClock -- periodic heartbeat at minute boundaries.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskClock.ts.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;

/// A simple heartbeat task that fires at minute boundaries.
///
/// Returns the current timestamp as a log string each time it runs.
pub struct TaskClock {
    /// Next minute boundary (epoch ms).
    next_minute: u64,
}

impl TaskClock {
    /// Create a new clock task.
    pub fn new() -> Self {
        Self {
            next_minute: Self::get_next_minute(),
        }
    }

    fn get_next_minute() -> u64 {
        let now = now_msecs();
        // Ceiling division to next minute boundary
        ((now / ONE_MINUTE) + 1) * ONE_MINUTE
    }
}

#[async_trait]
impl WalletMonitorTask for TaskClock {
    fn name(&self) -> &str {
        "Clock"
    }

    fn trigger(&mut self, _now_msecs: u64) -> bool {
        now_msecs() > self.next_minute
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let dt =
            chrono::DateTime::from_timestamp_millis(self.next_minute as i64).unwrap_or_default();
        let log = dt.to_rfc3339();
        self.next_minute = Self::get_next_minute();
        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_trigger_before_next_minute() {
        let mut clock = TaskClock::new();
        // next_minute is in the future, so trigger should be false now
        assert!(
            clock.next_minute > now_msecs(),
            "next_minute should be in the future"
        );
        assert!(!clock.trigger(now_msecs()));
    }

    #[tokio::test]
    async fn test_clock_run_task_returns_timestamp() {
        let mut clock = TaskClock::new();
        let result = clock.run_task().await.unwrap();
        // Should be an ISO timestamp string
        assert!(!result.is_empty());
        assert!(
            result.contains("T"),
            "Expected ISO timestamp, got: {}",
            result
        );
    }

    #[test]
    fn test_clock_name() {
        let clock = TaskClock::new();
        assert_eq!(clock.name(), "Clock");
    }
}
