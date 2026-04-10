//! WalletMonitorTask trait -- the interface for all monitor background tasks.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/WalletMonitorTask.ts.
//! Each task implements trigger (sync, fast check) and run_task (async work).

use async_trait::async_trait;

use crate::error::WalletError;
use crate::storage::manager::WalletStorageManager;

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

    /// Returns a reference to the task's storage manager, if it has one.
    ///
    /// The `Monitor::builder` pattern hands each task its own fresh
    /// `WalletStorageManager` instance (they share the underlying
    /// `Arc<dyn WalletStorageProvider>` but each has its own state —
    /// user cache, partition index, is_available flag). Each of those
    /// fresh managers must have `make_available()` called on it before
    /// the task can use methods that gate on `is_available_flag`
    /// (e.g. `update_transaction`, `review_status`, `purge_data`).
    ///
    /// The default `async_setup()` implementation below uses this hook
    /// to call `make_available()` on the task's storage before the task
    /// runs for the first time. Tasks with storage SHOULD override this
    /// to return `Some(&self.storage)`; tasks without storage can leave
    /// the default `None`.
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        None
    }

    /// Override to handle async task setup configuration.
    /// Called before the first call to `trigger`.
    ///
    /// Default implementation: calls `make_available()` on the task's
    /// `storage_manager()` (if it has one). `make_available()` is
    /// idempotent, so this is safe to call unconditionally.
    ///
    /// Tasks that override this MUST still make their storage available
    /// — either by calling `self.storage_manager()` and chaining to
    /// `make_available().await?` themselves, or by calling the default
    /// via a trait-specific delegation (see `task_arc_sse.rs` for an
    /// example of a custom override that preserves the default behavior).
    async fn async_setup(&mut self) -> Result<(), WalletError> {
        if let Some(storage) = self.storage_manager() {
            storage.make_available().await?;
        }
        Ok(())
    }

    /// Return true if `run_task` needs to be called now.
    /// This is NOT async -- it must be a fast, synchronous check.
    fn trigger(&mut self, now_msecs: u64) -> bool;

    /// Execute the task's work. Returns a log string describing what was done.
    async fn run_task(&mut self) -> Result<String, WalletError>;
}
