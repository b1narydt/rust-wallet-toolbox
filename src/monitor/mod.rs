//! Monitor module -- background task scheduler for transaction lifecycle management.
//!
//! Translated from wallet-toolbox/src/monitor/Monitor.ts.
//! Provides the Monitor struct, MonitorBuilder, WalletMonitorTask trait,
//! and supporting types for automatic transaction broadcasting, proof collection,
//! chain reorg detection, and data cleanup.

/// Shared helper functions for monitor tasks.
pub mod helpers;
/// The WalletMonitorTask trait for implementing custom tasks.
pub mod task_trait;
/// Built-in monitor task implementations.
pub mod tasks;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use tracing::{error, info, warn};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::services::types::BlockHeader;
use crate::storage::find_args::PurgeParams;
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;

use self::helpers::{log_event, now_msecs};
use self::task_trait::WalletMonitorTask;

// ---------------------------------------------------------------------------
// Time constants (in milliseconds)
// ---------------------------------------------------------------------------

/// One second in milliseconds.
pub const ONE_SECOND: u64 = 1000;
/// One minute in milliseconds.
pub const ONE_MINUTE: u64 = 60 * ONE_SECOND;
/// One hour in milliseconds.
pub const ONE_HOUR: u64 = 60 * ONE_MINUTE;
/// One day in milliseconds.
pub const ONE_DAY: u64 = 24 * ONE_HOUR;
/// One week in milliseconds.
pub const ONE_WEEK: u64 = 7 * ONE_DAY;

// ---------------------------------------------------------------------------
// Callback type aliases
// ---------------------------------------------------------------------------

/// Type alias for async callback closures used by the Monitor.
pub type AsyncCallback<T> =
    Arc<dyn Fn(T) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A fallible async callback used to place a durability boundary after a task.
pub type AsyncResultCallback<T> =
    Arc<dyn Fn(T) -> Pin<Box<dyn Future<Output = WalletResult<()>> + Send>> + Send + Sync>;

// ---------------------------------------------------------------------------
// MonitorOptions
// ---------------------------------------------------------------------------

/// Configuration options for the Monitor.
///
/// Matches the TS `MonitorOptions` interface from Monitor.ts.
pub struct MonitorOptions {
    /// Which chain this monitor operates on.
    pub chain: Chain,

    /// How many msecs to wait after each task run iteration.
    pub task_run_wait_msecs: u64,

    /// How many msecs before a transaction is considered abandoned.
    pub abandoned_msecs: u64,

    /// How many msecs to wait between merkle proof service requests.
    pub msecs_wait_per_merkle_proof_service_req: u64,

    /// Max unproven attempts before giving up (testnet).
    pub unproven_attempts_limit_test: u32,

    /// Max unproven attempts before giving up (mainnet).
    pub unproven_attempts_limit_main: u32,

    /// Stable callback token for ARC SSE event streaming.
    pub callback_token: Option<String>,

    /// Hook called when a transaction is broadcasted.
    pub on_tx_broadcasted: Option<AsyncCallback<String>>,

    /// Hook called when a transaction is proven.
    pub on_tx_proven: Option<AsyncCallback<String>>,

    /// Hook called when a transaction status changes.
    pub on_tx_status_changed: Option<AsyncCallback<(String, String)>>,

    /// Fallible hook run after a monitor task finishes and before the scheduler
    /// starts another task. Failures pause scheduling and retry
    /// with backoff until the hook succeeds or the monitor is stopped.
    pub after_task: Option<AsyncResultCallback<String>>,
}

impl Default for MonitorOptions {
    fn default() -> Self {
        Self {
            chain: Chain::Main,
            task_run_wait_msecs: 5000,
            abandoned_msecs: ONE_MINUTE * 5,
            msecs_wait_per_merkle_proof_service_req: 500,
            unproven_attempts_limit_test: 10,
            unproven_attempts_limit_main: 144,
            callback_token: None,
            on_tx_broadcasted: None,
            on_tx_proven: None,
            on_tx_status_changed: None,
            after_task: None,
        }
    }
}

// ---------------------------------------------------------------------------
// DeactivatedHeader
// ---------------------------------------------------------------------------

/// A deactivated block header from a reorg event, queued for aging before processing.
#[derive(Debug, Clone)]
pub struct DeactivatedHeader {
    /// Timestamp (epoch ms) when the deactivation was received.
    /// Used to control aging of notification before pursuing updated proof data.
    pub when_msecs: u64,
    /// Number of attempts made to process the header.
    /// Supports returning deactivation notification to the queue if proof data
    /// is not yet available.
    pub tries: u32,
    /// The deactivated block header.
    pub header: BlockHeader,
}

// ---------------------------------------------------------------------------
// Monitor struct
// ---------------------------------------------------------------------------

/// Background task scheduler that manages transaction lifecycle.
///
/// Automatically handles broadcasting pending transactions, collecting merkle proofs,
/// detecting chain reorgs, failing stale transactions, and cleaning up old data.
///
/// Translated from TS `Monitor` class in wallet-toolbox/src/monitor/Monitor.ts.
///
/// NOTE: Tasks run sequentially within each polling iteration, matching the TS pattern.
/// Concurrent task execution is a future optimization.
pub struct Monitor {
    /// Configuration options.
    pub options: MonitorOptions,

    /// Storage manager for persistence operations (shared via Arc).
    pub storage: Arc<WalletStorageManager>,

    /// Network services for broadcasting and proof retrieval.
    pub services: Arc<dyn WalletServices>,

    /// The chain this monitor is configured for.
    pub chain: Chain,

    /// Scheduled tasks -- run by the polling loop and also by `run_task`.
    tasks: Vec<Box<dyn WalletMonitorTask>>,

    /// Other tasks -- only run by `run_task`, not by the scheduler.
    other_tasks: Vec<Box<dyn WalletMonitorTask>>,

    /// Flag indicating whether the polling loop is running.
    running: Arc<AtomicBool>,

    /// Flag to nudge proof checking (set by processNewBlockHeader).
    /// Shared with TaskCheckForProofs.
    pub check_now: Arc<AtomicBool>,

    /// Shared last header height for max acceptable height guard.
    /// u32::MAX is the sentinel for "no height known".
    /// Shared with TaskCheckForProofs and updated in process_new_block_header.
    pub last_new_header_height: Arc<AtomicU32>,

    /// The last new block header received.
    pub last_new_header: Option<BlockHeader>,

    /// When the last new header was received (epoch ms).
    pub last_new_header_when: Option<u64>,

    /// Queue of deactivated headers from reorg events, awaiting processing.
    pub deactivated_headers: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>>,

    /// Handle to the spawned polling loop task.
    join_handle: Option<tokio::task::JoinHandle<()>>,

    /// Whether async_setup needs to run on first iteration.
    run_async_setup: bool,
}

/// Run one monitor task with PANIC ISOLATION (F3, monitor parity audit
/// 2026-07-24). A task that unwinds must degrade to one error log line and let
/// the loop continue — an unwound loop future leaves `running == true` while
/// nothing broadcasts ever again, which is the rust-mpc#147 "wallet silently
/// never posts a tx" symptom for every embedder now that the monitor is on by
/// default. Mirrors the per-task try/catch in TS Monitor.runTask.
async fn run_task_isolated(
    task: &mut Box<dyn WalletMonitorTask>,
) -> Result<String, crate::error::WalletError> {
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(task.run_task())).await {
        Ok(result) => result,
        Err(_) => Err(crate::error::WalletError::Internal(format!(
            "monitor task {} PANICKED — isolated; the monitor loop continues",
            task.name()
        ))),
    }
}

/// Wait for an embedding's durable-write barrier after every task outcome.
/// A task may commit part of its work before returning an error, and monitor
/// event logging itself writes storage, so the barrier intentionally runs only
/// after both have completed.
async fn await_after_task(
    callback: &AsyncResultCallback<String>,
    task_name: &str,
    running: &Arc<AtomicBool>,
) -> bool {
    let mut retry_delay_secs = 1_u64;
    while running.load(Ordering::SeqCst) {
        match callback(task_name.to_owned()).await {
            Ok(()) => return true,
            Err(e) => {
                error!(
                    "monitor after_task callback failed after {task_name}: {e}; retrying in {retry_delay_secs}s"
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay_secs)).await;
                retry_delay_secs = retry_delay_secs.saturating_mul(2).min(5);
            }
        }
    }
    false
}

/// Truncate `log` to at most `max` BYTES on a char boundary. `&log[..1024]` was
/// a latent panic (F3): provider error strings are interpolated into task logs,
/// and a multi-byte char straddling the cut would unwind the monitor loop.
fn truncate_log(log: &str, max: usize) -> &str {
    if log.len() <= max {
        return log;
    }
    let mut end = max;
    while end > 0 && !log.is_char_boundary(end) {
        end -= 1;
    }
    &log[..end]
}

/// Default purge parameters matching the TS Monitor.defaultPurgeParams.
pub fn default_purge_params() -> PurgeParams {
    PurgeParams {
        purge_spent: false,
        purge_completed: false,
        purge_failed: true,
        purge_spent_age: 2 * ONE_WEEK,
        purge_completed_age: 2 * ONE_WEEK,
        purge_failed_age: 5 * ONE_DAY,
        purge_monitor_events: true,
        purge_monitor_events_age: 30 * ONE_DAY,
    }
}

impl Monitor {
    /// Returns a new MonitorBuilder for fluent construction.
    pub fn builder() -> MonitorBuilder {
        MonitorBuilder::new()
    }

    /// Start the polling loop in a background tokio task.
    ///
    /// Validates that the monitor is not already running, then spawns the loop.
    pub fn start_tasks(&mut self) -> WalletResult<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(WalletError::BadRequest(
                "monitor tasks are already running".to_string(),
            ));
        }

        self.running.store(true, Ordering::SeqCst);
        self.run_async_setup = true;

        // Move task state into the spawned future.
        // We need to take ownership of tasks for the spawn.
        let running = self.running.clone();
        let _check_now = self.check_now.clone();
        let _deactivated_headers = self.deactivated_headers.clone();
        let task_run_wait_msecs = self.options.task_run_wait_msecs;
        let after_task = self.options.after_task.clone();

        // Take tasks out of self for the spawned future.
        // They will be returned when stop_tasks is called.
        let mut tasks: Vec<Box<dyn WalletMonitorTask>> = std::mem::take(&mut self.tasks);
        let storage = self.storage.clone();

        let handle = tokio::spawn(async move {
            // Ensure the monitor's storage manager is initialized before tasks run.
            // Tasks created by the builder get fresh WalletStorageManager clones that
            // need make_available() before they can query the database.
            if let Err(e) = storage.make_available().await {
                warn!("Monitor storage make_available failed: {e}");
            }

            // First iteration: run async_setup on all tasks
            for task in tasks.iter_mut() {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(e) = task.async_setup().await {
                    let details = format!("monitor task {} asyncSetup error: {}", task.name(), e);
                    warn!("{}", details);
                    let _ = log_event(&storage, "error0", &details).await;
                }
            }

            // Main polling loop
            while running.load(Ordering::SeqCst) {
                let now = now_msecs();

                // Collect indices of triggered tasks
                let mut triggered_indices = Vec::new();
                for (i, task) in tasks.iter_mut().enumerate() {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        task.trigger(now)
                    })) {
                        Ok(should_run) => {
                            if should_run {
                                triggered_indices.push(i);
                            }
                        }
                        Err(_) => {
                            let details = format!("monitor task {} trigger panicked", task.name());
                            error!("{}", details);
                            let _ = log_event(&storage, "error0", &details).await;
                        }
                    }
                }

                // Run triggered tasks sequentially
                for idx in triggered_indices {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let task = &mut tasks[idx];
                    match run_task_isolated(task).await {
                        Ok(log) => {
                            if !log.is_empty() {
                                info!("Task {} {}", task.name(), truncate_log(&log, 1024));
                                let _ = log_event(&storage, task.name(), &log).await;
                            }
                        }
                        Err(e) => {
                            let details =
                                format!("monitor task {} runTask error: {}", task.name(), e);
                            error!("{}", details);
                            let _ = log_event(&storage, "error1", &details).await;
                        }
                    }
                    if let Some(ref callback) = after_task {
                        if !await_after_task(callback, task.name(), &running).await {
                            break;
                        }
                    }
                }

                // Sleep before next iteration
                tokio::time::sleep(tokio::time::Duration::from_millis(task_run_wait_msecs)).await;
            }

            info!("Monitor polling loop stopped");
        });

        self.join_handle = Some(handle);
        Ok(())
    }

    /// Stop the polling loop and wait for it to complete.
    pub async fn stop_tasks(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }

    /// Destroy the monitor: stops tasks and cleans up subscriptions.
    pub async fn destroy(&mut self) {
        self.stop_tasks().await;
        // Future: unsubscribe from chaintracks events if subscribed.
    }

    /// Run one iteration of the polling loop (for testing).
    ///
    /// Runs async_setup on first call, then triggers and runs tasks once.
    pub async fn run_once(&mut self) -> WalletResult<()> {
        if self.run_async_setup {
            for task in self.tasks.iter_mut() {
                if let Err(e) = task.async_setup().await {
                    let details = format!("monitor task {} asyncSetup error: {}", task.name(), e);
                    warn!("{}", details);
                    let _ = log_event(&self.storage, "error0", &details).await;
                }
            }
            self.run_async_setup = false;
        }

        let now = now_msecs();

        // Collect triggered tasks
        let mut triggered_indices = Vec::new();
        for (i, task) in self.tasks.iter_mut().enumerate() {
            if task.trigger(now) {
                triggered_indices.push(i);
            }
        }

        // Run triggered tasks
        for idx in triggered_indices {
            let task = &mut self.tasks[idx];
            match run_task_isolated(task).await {
                Ok(log) => {
                    if !log.is_empty() {
                        info!("Task {} {}", task.name(), truncate_log(&log, 1024));
                        let _ = log_event(&self.storage, task.name(), &log).await;
                    }
                }
                Err(e) => {
                    let details = format!("monitor task {} runTask error: {}", task.name(), e);
                    error!("{}", details);
                    let _ = log_event(&self.storage, "error1", &details).await;
                }
            }
        }

        Ok(())
    }

    /// Run a specific task by name (from _tasks or _otherTasks).
    pub async fn run_task(&mut self, name: &str) -> WalletResult<String> {
        // Search in scheduled tasks first
        for task in self.tasks.iter_mut() {
            if task.name() == name {
                task.async_setup().await?;
                return task.run_task().await;
            }
        }
        // Search in other tasks
        for task in self.other_tasks.iter_mut() {
            if task.name() == name {
                task.async_setup().await?;
                return task.run_task().await;
            }
        }
        Err(WalletError::InvalidParameter {
            parameter: "name".to_string(),
            must_be: format!("an existing task name, '{name}' not found"),
        })
    }

    /// Process a new block header event received from Chaintracks.
    ///
    /// Stores the header and nudges the proof checker to try again.
    pub fn process_new_block_header(&mut self, header: BlockHeader) {
        // Update the shared last header height for the max acceptable height guard.
        self.last_new_header_height
            .store(header.height, Ordering::SeqCst);
        self.last_new_header = Some(header);
        self.last_new_header_when = Some(now_msecs());
        // Nudge the proof checker to try again.
        self.check_now.store(true, Ordering::SeqCst);
    }

    /// Process a reorg event received from Chaintracks.
    ///
    /// Reorgs can move recent transactions to new blocks at new index positions.
    /// Affected transaction proofs become invalid and must be updated.
    pub async fn process_reorg(
        &self,
        _depth: u32,
        _old_tip: &BlockHeader,
        _new_tip: &BlockHeader,
        deactivated: Option<&[BlockHeader]>,
    ) {
        if let Some(headers) = deactivated {
            let mut queue = self.deactivated_headers.lock().await;
            for header in headers {
                queue.push(DeactivatedHeader {
                    when_msecs: now_msecs(),
                    tries: 0,
                    header: header.clone(),
                });
            }
        }
    }

    /// Handler for new header events from Chaintracks.
    ///
    /// To minimize reorg processing, new headers are aged before processing
    /// via TaskNewHeader. Therefore this handler is intentionally a no-op.
    pub fn process_header(&self, _header: &BlockHeader) {
        // Intentional no-op -- headers aged via TaskNewHeader polling.
    }

    // -----------------------------------------------------------------------
    // Callback helpers
    // -----------------------------------------------------------------------

    /// Call the on_tx_broadcasted callback if registered.
    pub async fn call_on_broadcasted_transaction(&self, broadcast_result: &str) {
        if let Some(ref cb) = self.options.on_tx_broadcasted {
            cb(broadcast_result.to_string()).await;
        }
    }

    /// Call the on_tx_proven callback if registered.
    pub async fn call_on_proven_transaction(&self, tx_status: &str) {
        if let Some(ref cb) = self.options.on_tx_proven {
            cb(tx_status.to_string()).await;
        }
    }

    /// Call the on_tx_status_changed callback if registered.
    pub async fn call_on_transaction_status_changed(&self, txid: &str, new_status: &str) {
        if let Some(ref cb) = self.options.on_tx_status_changed {
            cb((txid.to_string(), new_status.to_string())).await;
        }
    }

    /// Returns whether the monitor is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Add a task to the scheduled task list.
    pub fn add_task(&mut self, task: Box<dyn WalletMonitorTask>) -> WalletResult<()> {
        let name = task.name().to_string();
        if self.tasks.iter().any(|t| t.name() == name) {
            return Err(WalletError::BadRequest(format!(
                "task {name} has already been added"
            )));
        }
        self.tasks.push(task);
        Ok(())
    }

    /// Remove a task from the scheduled task list by name.
    pub fn remove_task(&mut self, name: &str) {
        self.tasks.retain(|t| t.name() != name);
    }
}

// ---------------------------------------------------------------------------
// MonitorBuilder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a Monitor instance.
///
/// Required fields: chain, storage, services.
/// Optional: task presets, individual tasks, callback hooks.
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # fn example(
/// #     storage: Arc<bsv_wallet_toolbox::storage::manager::WalletStorageManager>,
/// #     services: Arc<dyn bsv_wallet_toolbox::services::traits::WalletServices>,
/// # ) -> bsv_wallet_toolbox::WalletResult<()> {
/// use bsv_wallet_toolbox::monitor::Monitor;
/// use bsv_wallet_toolbox::types::Chain;
///
/// let monitor = Monitor::builder()
///     .chain(Chain::Test)
///     .storage(storage)
///     .services(services)
///     .default_tasks()
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct MonitorBuilder {
    chain: Option<Chain>,
    storage: Option<Arc<WalletStorageManager>>,
    services: Option<Arc<dyn WalletServices>>,
    options: MonitorOptions,
    default_tasks: bool,
    multi_user_tasks: bool,
    extra_tasks: Vec<Box<dyn WalletMonitorTask>>,
    removed_task_names: Vec<String>,
}

impl MonitorBuilder {
    fn new() -> Self {
        Self {
            chain: None,
            storage: None,
            services: None,
            options: MonitorOptions::default(),
            default_tasks: false,
            multi_user_tasks: false,
            extra_tasks: Vec::new(),
            removed_task_names: Vec::new(),
        }
    }

    /// Set the chain (required).
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the storage manager (required).
    pub fn storage(mut self, storage: Arc<WalletStorageManager>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Set the services provider (required).
    pub fn services(mut self, services: Arc<dyn WalletServices>) -> Self {
        self.services = Some(services);
        self
    }

    /// Set the task run wait interval in milliseconds.
    pub fn task_run_wait_msecs(mut self, msecs: u64) -> Self {
        self.options.task_run_wait_msecs = msecs;
        self
    }

    /// Set the abandoned transaction threshold in milliseconds.
    pub fn abandoned_msecs(mut self, msecs: u64) -> Self {
        self.options.abandoned_msecs = msecs;
        self
    }

    /// Set the callback token for ARC SSE streaming.
    pub fn callback_token(mut self, token: String) -> Self {
        self.options.callback_token = Some(token);
        self
    }

    /// Use default tasks preset (single-user with sync) — 16 tasks.
    ///
    /// Wires: TaskClock, TaskMonitorCallHistory, TaskNewHeader, TaskSendWaiting,
    /// TaskCheckForProofs, TaskCheckNoSends, TaskFailAbandoned, TaskUnFail,
    /// TaskReviewStatus, TaskReviewDoubleSpends, TaskReviewProvenTxs,
    /// TaskReviewUtxos, TaskReorg, TaskArcSse, TaskSyncWhenIdle, TaskPurge.
    /// (TaskMineBlock exists but is wired by NO preset.)
    pub fn default_tasks(mut self) -> Self {
        self.default_tasks = true;
        self
    }

    /// Use multi-user tasks preset (without sync) — the default preset minus
    /// TaskArcSse and TaskSyncWhenIdle (14 tasks).
    pub fn multi_user_tasks(mut self) -> Self {
        self.multi_user_tasks = true;
        self
    }

    /// Add an additional task to the scheduled task list.
    pub fn add_task(mut self, task: Box<dyn WalletMonitorTask>) -> Self {
        self.extra_tasks.push(task);
        self
    }

    /// Remove a task by name from the preset task list.
    pub fn remove_task(mut self, name: &str) -> Self {
        self.removed_task_names.push(name.to_string());
        self
    }

    /// Set the on_tx_broadcasted callback.
    pub fn on_tx_broadcasted(mut self, cb: AsyncCallback<String>) -> Self {
        self.options.on_tx_broadcasted = Some(cb);
        self
    }

    /// Set the on_tx_proven callback.
    pub fn on_tx_proven(mut self, cb: AsyncCallback<String>) -> Self {
        self.options.on_tx_proven = Some(cb);
        self
    }

    /// Set the on_tx_status_changed callback.
    pub fn on_tx_status_changed(mut self, cb: AsyncCallback<(String, String)>) -> Self {
        self.options.on_tx_status_changed = Some(cb);
        self
    }

    /// Set a fallible hook that runs after each completed task.
    ///
    /// The hook runs before the scheduler starts another task. An error pauses
    /// scheduling and retries with backoff, so embeddings fail closed without
    /// permanently turning off background processing when a durable replica or
    /// external acknowledgement is temporarily unavailable.
    pub fn after_task(mut self, callback: AsyncResultCallback<String>) -> Self {
        self.options.after_task = Some(callback);
        self
    }

    /// Build the Monitor instance.
    ///
    /// Validates that required fields (chain, storage, services) are set.
    /// Constructs tasks based on the selected preset.
    pub fn build(mut self) -> WalletResult<Monitor> {
        let chain = self
            .chain
            .ok_or_else(|| WalletError::MissingParameter("chain".to_string()))?;
        let storage = self
            .storage
            .ok_or_else(|| WalletError::MissingParameter("storage".to_string()))?;
        let services = self
            .services
            .ok_or_else(|| WalletError::MissingParameter("services".to_string()))?;

        self.options.chain = chain.clone();

        // Shared state for task coordination
        let check_now = Arc::new(AtomicBool::new(false));
        let last_new_header_height = Arc::new(AtomicU32::new(u32::MAX));
        let deactivated_headers: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        // All tasks share the same Arc<WalletStorageManager>
        let make_storage =
            |s: &Arc<WalletStorageManager>| -> Arc<WalletStorageManager> { s.clone() };

        // Unproven attempt limits based on chain
        let unproven_limit = match chain {
            Chain::Test => self.options.unproven_attempts_limit_test,
            _ => self.options.unproven_attempts_limit_main,
        };

        // Build task list based on preset.
        let mut tasks: Vec<Box<dyn WalletMonitorTask>> = Vec::new();

        if self.default_tasks || self.multi_user_tasks {
            // -- Plan 03 tasks --
            tasks.push(Box::new(tasks::task_clock::TaskClock::new()));
            tasks.push(Box::new(
                tasks::task_monitor_call_history::TaskMonitorCallHistory::new(services.clone()),
            ));

            // -- Plan 02 tasks --
            tasks.push(Box::new(tasks::task_new_header::TaskNewHeader::new(
                make_storage(&storage),
                services.clone(),
                check_now.clone(),
                last_new_header_height.clone(),
            )));
            tasks.push(Box::new(tasks::task_send_waiting::TaskSendWaiting::new(
                make_storage(&storage),
                services.clone(),
                chain.clone(),
                self.options.on_tx_broadcasted.clone(),
            )));
            tasks.push(Box::new(
                tasks::task_check_for_proofs::TaskCheckForProofs::new(
                    make_storage(&storage),
                    services.clone(),
                    chain.clone(),
                    check_now.clone(),
                    unproven_limit,
                    self.options.on_tx_proven.clone(),
                    last_new_header_height.clone(),
                ),
            ));
            tasks.push(Box::new(tasks::task_check_no_sends::TaskCheckNoSends::new(
                make_storage(&storage),
                services.clone(),
                chain.clone(),
                unproven_limit,
                last_new_header_height.clone(),
            )));
            tasks.push(Box::new(
                tasks::task_fail_abandoned::TaskFailAbandoned::new(
                    make_storage(&storage),
                    self.options.abandoned_msecs,
                ),
            ));
            tasks.push(Box::new(tasks::task_unfail::TaskUnFail::new(
                make_storage(&storage),
                services.clone(),
            )));
            tasks.push(Box::new(tasks::task_review_status::TaskReviewStatus::new(
                make_storage(&storage),
            )));
            tasks.push(Box::new(
                tasks::task_review_double_spends::TaskReviewDoubleSpends::new(
                    make_storage(&storage),
                    services.clone(),
                ),
            ));
            tasks.push(Box::new(
                tasks::task_review_proven_txs::TaskReviewProvenTxs::new(
                    make_storage(&storage),
                    services.clone(),
                ),
            ));
            tasks.push(Box::new(tasks::task_review_utxos::TaskReviewUtxos::new(
                make_storage(&storage),
            )));
            tasks.push(Box::new(tasks::task_reorg::TaskReorg::new(
                make_storage(&storage),
                services.clone(),
                deactivated_headers.clone(),
            )));

            // Default tasks preset includes ARC SSE and SyncWhenIdle
            if self.default_tasks {
                tasks.push(Box::new(tasks::task_arc_sse::TaskArcSse::new(
                    make_storage(&storage),
                    services.clone(),
                    self.options.callback_token.clone(),
                    self.options.on_tx_status_changed.clone(),
                )));
                tasks.push(Box::new(tasks::task_sync_when_idle::TaskSyncWhenIdle::new()));
            }

            // TaskPurge: present in presets with long interval (TS comments it out in default)
            tasks.push(Box::new(tasks::task_purge::TaskPurge::new(
                make_storage(&storage),
                default_purge_params(),
            )));
        }

        // Remove tasks by name
        for name in &self.removed_task_names {
            tasks.retain(|t| t.name() != name.as_str());
        }

        // Add extra tasks
        tasks.append(&mut self.extra_tasks);

        Ok(Monitor {
            options: self.options,
            storage,
            services,
            chain,
            tasks,
            other_tasks: Vec::new(),
            running: Arc::new(AtomicBool::new(false)),
            check_now,
            last_new_header_height,
            last_new_header: None,
            last_new_header_when: None,
            deactivated_headers,
            join_handle: None,
            run_async_setup: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::types::BlockHeader;
    use std::sync::atomic::AtomicUsize;

    // A minimal mock WalletServices for testing.
    // Only constructed indirectly via trait-object boxing in tests below
    // that exercise default-derived wiring; the explicit constructor is
    // intentionally kept for future unit tests.
    #[allow(dead_code)]
    struct MockServices {
        chain: Chain,
    }

    #[async_trait::async_trait]
    impl WalletServices for MockServices {
        fn chain(&self) -> Chain {
            self.chain.clone()
        }
        async fn get_chain_tracker(
            &self,
        ) -> WalletResult<Box<dyn bsv::transaction::chain_tracker::ChainTracker>> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        async fn get_merkle_path(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> crate::services::types::GetMerklePathResult {
            crate::services::types::GetMerklePathResult::default()
        }
        async fn get_raw_tx(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> crate::services::types::GetRawTxResult {
            crate::services::types::GetRawTxResult::default()
        }
        async fn post_beef(
            &self,
            _beef: &[u8],
            _txids: &[String],
        ) -> Vec<crate::services::types::PostBeefResult> {
            vec![]
        }
        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<crate::services::types::GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> crate::services::types::GetUtxoStatusResult {
            crate::services::types::GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                is_utxo: None,
                details: vec![],
            }
        }
        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> crate::services::types::GetStatusForTxidsResult {
            crate::services::types::GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                results: vec![],
            }
        }
        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> crate::services::types::GetScriptHashHistoryResult {
            crate::services::types::GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("mock".to_string()),
                history: vec![],
            }
        }
        async fn hash_to_header(&self, _hash: &str) -> WalletResult<BlockHeader> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        async fn get_height(&self) -> WalletResult<u32> {
            Ok(800000)
        }
        async fn n_lock_time_is_final(
            &self,
            _input: crate::services::types::NLockTimeInput,
        ) -> WalletResult<bool> {
            Ok(true)
        }
        async fn get_bsv_exchange_rate(
            &self,
        ) -> WalletResult<crate::services::types::BsvExchangeRate> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }
        async fn get_fiat_exchange_rates(
            &self,
            _target: &[String],
        ) -> WalletResult<crate::services::types::FiatExchangeRates> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        fn get_services_call_history(
            &self,
            _reset: bool,
        ) -> crate::services::types::ServicesCallHistory {
            crate::services::types::ServicesCallHistory { services: vec![] }
        }
        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<bsv::transaction::Beef> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }
        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            _txid: &str,
            _vout: u32,
        ) -> WalletResult<bool> {
            Ok(false)
        }
    }

    // We cannot easily construct a real WalletStorageManager in unit tests
    // (it requires a real StorageProvider), so we test what we can without one.

    #[test]
    fn test_monitor_builder_validates_required_fields() {
        // Missing chain
        let result = MonitorBuilder::new().build();
        assert!(result.is_err());
        match result {
            Err(e) => assert!(
                e.to_string().contains("chain"),
                "Expected chain error, got: {e}"
            ),
            Ok(_) => panic!("Expected error for missing chain"),
        }

        // Missing storage (chain set but no storage)
        let result = MonitorBuilder::new().chain(Chain::Test).build();
        assert!(result.is_err());
        match result {
            Err(e) => assert!(
                e.to_string().contains("storage"),
                "Expected storage error, got: {e}"
            ),
            Ok(_) => panic!("Expected error for missing storage"),
        }
    }

    #[test]
    fn test_time_constants() {
        assert_eq!(ONE_SECOND, 1000);
        assert_eq!(ONE_MINUTE, 60_000);
        assert_eq!(ONE_HOUR, 3_600_000);
        assert_eq!(ONE_DAY, 86_400_000);
        assert_eq!(ONE_WEEK, 604_800_000);
    }

    #[test]
    fn test_default_purge_params() {
        let params = default_purge_params();
        assert!(!params.purge_spent);
        assert!(!params.purge_completed);
        assert!(params.purge_failed);
        assert_eq!(params.purge_spent_age, 2 * ONE_WEEK);
        assert_eq!(params.purge_completed_age, 2 * ONE_WEEK);
        assert_eq!(params.purge_failed_age, 5 * ONE_DAY);
    }

    #[test]
    fn test_deactivated_header() {
        let header = BlockHeader {
            version: 1,
            previous_hash: "0000".to_string(),
            merkle_root: "abcd".to_string(),
            time: 1234567890,
            bits: 0x1d00ffff,
            nonce: 42,
            height: 100,
            hash: "blockhash".to_string(),
        };
        let dh = DeactivatedHeader {
            when_msecs: 1000,
            tries: 0,
            header: header.clone(),
        };
        assert_eq!(dh.when_msecs, 1000);
        assert_eq!(dh.tries, 0);
        assert_eq!(dh.header.height, 100);
    }

    #[tokio::test]
    async fn a_panicking_task_is_isolated_and_the_loop_survives() {
        // F3 (parity audit 2026-07-24): a task that unwinds must come back as an
        // Err, not kill the caller — a dead loop with running==true is a wallet
        // that silently never broadcasts again.
        struct PanicTask;
        #[async_trait::async_trait]
        impl WalletMonitorTask for PanicTask {
            fn trigger(&mut self, _now: u64) -> bool {
                true
            }
            async fn run_task(&mut self) -> crate::WalletResult<String> {
                panic!("boom");
            }
            fn name(&self) -> &'static str {
                "PanicTask"
            }
        }
        let mut task: Box<dyn WalletMonitorTask> = Box::new(PanicTask);
        let out = run_task_isolated(&mut task).await;
        let err = out.expect_err("a panic must surface as an Err, not an unwind");
        assert!(err.to_string().contains("PANICKED"), "got: {err}");
    }

    #[tokio::test]
    async fn after_task_retries_a_transient_durability_failure_without_stopping() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_callback = Arc::clone(&attempts);
        let callback: AsyncResultCallback<String> = Arc::new(move |_| {
            let attempts = Arc::clone(&attempts_for_callback);
            Box::pin(async move {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(WalletError::Internal("temporary artifact outage".to_string()))
                } else {
                    Ok(())
                }
            })
        });
        let running = Arc::new(AtomicBool::new(true));

        assert!(await_after_task(&callback, "test-task", &running).await);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(running.load(Ordering::SeqCst));
    }

    #[test]
    fn truncate_log_never_cuts_mid_char() {
        // The old `&log[..1024]` slice panicked when byte 1024 fell inside a
        // multi-byte char — provider error strings make that reachable.
        let log = "é".repeat(1000); // 2 bytes per char => byte 1024 is mid-char
        let cut = truncate_log(&log, 1024);
        assert!(cut.len() <= 1024);
        assert!(cut.chars().all(|c| c == 'é'));
        assert_eq!(truncate_log("short", 1024), "short");
    }

    #[test]
    fn test_now_msecs_returns_reasonable_value() {
        let now = now_msecs();
        // Should be after 2020-01-01 in milliseconds
        assert!(now > 1_577_836_800_000);
    }

    #[tokio::test]
    async fn test_process_reorg_adds_deactivated_headers() {
        // We need a Monitor to test process_reorg.
        // Create one with mock services -- requires a storage manager though.
        // Instead, test the DeactivatedHeader queue logic directly.
        let queue: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let header = BlockHeader {
            version: 1,
            previous_hash: "0000".to_string(),
            merkle_root: "abcd".to_string(),
            time: 1234567890,
            bits: 0x1d00ffff,
            nonce: 42,
            height: 100,
            hash: "blockhash".to_string(),
        };

        // Simulate process_reorg logic
        {
            let mut q = queue.lock().await;
            q.push(DeactivatedHeader {
                when_msecs: now_msecs(),
                tries: 0,
                header: header.clone(),
            });
        }

        let q = queue.lock().await;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].header.height, 100);
        assert_eq!(q[0].tries, 0);
    }
}
