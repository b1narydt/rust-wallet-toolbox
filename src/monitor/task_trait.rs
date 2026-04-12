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
    ///
    /// ⚠️ WARNING ⚠️: a task that owns a `WalletStorageManager` but
    /// forgets to override this method silently re-regresses the
    /// `WalletStorageManager not yet available` bug class — every
    /// operation that gates on `is_available_flag` (update_transaction,
    /// review_status, purge_data, fail_abandoned, …) will error on
    /// every tick of that task. This failure mode is invisible unless
    /// you specifically log the monitor error stream, so please
    /// override for any new task that owns storage.
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        None
    }

    /// Override to handle async task setup configuration.
    /// Called before the first call to `trigger`.
    ///
    /// Default implementation: delegates to `default_async_setup`,
    /// which calls `make_available()` on the task's `storage_manager()`
    /// (if it has one). `make_available()` is idempotent, so this is
    /// safe to call unconditionally.
    ///
    /// Tasks that override this MUST still invoke `default_async_setup`
    /// (NOT a hand-copied `make_available().await?`) so that if the
    /// default ever grows (logging, metrics, additional state prep)
    /// overriding tasks automatically inherit the new behavior instead
    /// of silently skipping it.
    async fn async_setup(&mut self) -> Result<(), WalletError> {
        default_async_setup(self).await
    }

    /// Return true if `run_task` needs to be called now.
    /// This is NOT async -- it must be a fast, synchronous check.
    fn trigger(&mut self, now_msecs: u64) -> bool;

    /// Execute the task's work. Returns a log string describing what was done.
    async fn run_task(&mut self) -> Result<String, WalletError>;
}

/// Default `async_setup` body, exposed as a free function so that
/// tasks that override `async_setup` can still delegate to it by
/// calling `default_async_setup(self).await?` rather than hand-copying
/// the `make_available` call. Any future additions to setup (logging,
/// metrics, state prep) land here once and are picked up automatically
/// by every overriding task.
pub async fn default_async_setup<T: WalletMonitorTask + ?Sized>(
    task: &T,
) -> Result<(), WalletError> {
    if let Some(storage) = task.storage_manager() {
        storage.make_available().await?;
    }
    Ok(())
}
