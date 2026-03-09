//! WalletMonitorTask trait -- the interface for all monitor background tasks.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/WalletMonitorTask.ts.
//! Each task implements trigger (sync, fast check) and run_task (async work).

use async_trait::async_trait;

use crate::error::WalletError;

/// A monitor task performs some periodic or state-triggered maintenance function
/// on the data managed by a wallet.
///
/// The monitor maintains a collection of tasks. It runs each task's non-async
/// `trigger` to determine if `run_task` needs to run. Tasks that need to be run
/// are executed consecutively by awaiting their async `run_task` method.
///
/// Tasks may use the monitor_events table to persist their execution history
/// via the storage object.
#[async_trait]
pub trait WalletMonitorTask: Send + Sync {
    /// Returns the name of this task (used for logging and lookup).
    fn name(&self) -> &str;

    /// Override to handle async task setup configuration.
    /// Called before the first call to `trigger`.
    async fn async_setup(&mut self) -> Result<(), WalletError> {
        Ok(())
    }

    /// Return true if `run_task` needs to be called now.
    /// This is NOT async -- it must be a fast, synchronous check.
    fn trigger(&mut self, now_msecs: u64) -> bool;

    /// Execute the task's work. Returns a log string describing what was done.
    async fn run_task(&mut self) -> Result<String, WalletError>;
}
