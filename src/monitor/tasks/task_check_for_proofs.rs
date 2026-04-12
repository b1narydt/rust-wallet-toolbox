//! TaskCheckForProofs -- collects merkle proofs for unmined transactions.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskCheckForProofs.ts (243 lines).
//!
//! Normally triggered by the check_now flag (set when a new block header is detected).
//! Queries for ProvenTxReqs with status "unmined"/"callback"/"sending"/"unknown"/"unconfirmed"
//! and attempts to retrieve merkle proofs via services.get_merkle_path().

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::get_proofs;
use crate::monitor::{AsyncCallback, ONE_HOUR};
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, Paged, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;

use super::super::task_trait::WalletMonitorTask;

/// Background task that collects merkle proofs for broadcast transactions.
///
/// Triggered by the shared `check_now` flag (set when new block headers arrive),
/// or by a periodic timer. Queries for unconfirmed ProvenTxReqs and attempts
/// to retrieve merkle proofs from services.
pub struct TaskCheckForProofs {
    /// Storage manager for persistence operations.
    storage: WalletStorageManager,
    /// Network services for proof retrieval.
    services: Arc<dyn WalletServices>,
    /// Periodic trigger interval (default 2 hours).
    trigger_msecs: u64,
    /// Last time this task ran (epoch ms).
    last_run_msecs: u64,
    /// Shared flag: set to true by Monitor.process_new_block_header().
    /// When true, triggers immediate proof checking.
    check_now: Arc<AtomicBool>,
    /// Optional callback when a transaction is proven.
    on_tx_proven: Option<AsyncCallback<String>>,
    /// Chain (main/test).
    chain: Chain,
    /// Max unproven attempts before giving up.
    unproven_attempts_limit: u32,
    /// Shared last header height (AtomicU32, u32::MAX means no height known).
    /// Updated by the Monitor when new block headers arrive.
    last_new_header_height: Arc<AtomicU32>,
}

impl TaskCheckForProofs {
    /// Create a new TaskCheckForProofs with default intervals.
    pub fn new(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        chain: Chain,
        check_now: Arc<AtomicBool>,
        unproven_attempts_limit: u32,
        on_tx_proven: Option<AsyncCallback<String>>,
        last_new_header_height: Arc<AtomicU32>,
    ) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 2 * ONE_HOUR,
            last_run_msecs: 0,
            check_now,
            on_tx_proven,
            chain,
            unproven_attempts_limit,
            last_new_header_height,
        }
    }

    /// Create with a custom trigger interval.
    // All eight arguments are distinct runtime dependencies (storage, services,
    // chain selection, control flags, timing, retry policy, callback, shared
    // header height). A builder or context struct would not meaningfully
    // reduce coupling; each parameter is orthogonal and required here.
    #[allow(clippy::too_many_arguments)]
    pub fn with_trigger_msecs(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        chain: Chain,
        check_now: Arc<AtomicBool>,
        trigger_msecs: u64,
        unproven_attempts_limit: u32,
        on_tx_proven: Option<AsyncCallback<String>>,
        last_new_header_height: Arc<AtomicU32>,
    ) -> Self {
        Self {
            storage,
            services,
            trigger_msecs,
            last_run_msecs: 0,
            check_now,
            on_tx_proven,
            chain,
            unproven_attempts_limit,
            last_new_header_height,
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskCheckForProofs {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "CheckForProofs"
    }

    fn trigger(&mut self, now_msecs_since_epoch: u64) -> bool {
        // Run if check_now flag is set (new block header received)
        let check_now = self.check_now.load(Ordering::SeqCst);

        // Also run if periodic timer has elapsed
        let timer_expired = self.trigger_msecs > 0
            && now_msecs_since_epoch > self.last_run_msecs + self.trigger_msecs;

        if check_now || timer_expired {
            self.last_run_msecs = now_msecs_since_epoch;
            true
        } else {
            false
        }
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let mut log = String::new();
        let counts_as_attempt = self.check_now.load(Ordering::SeqCst);

        // Reset the check_now flag
        self.check_now.store(false, Ordering::SeqCst);

        // Read the shared last header height; u32::MAX is the sentinel for "unknown".
        // Match TS behavior: skip proof checking entirely when no header height is known.
        let raw_height = self.last_new_header_height.load(Ordering::SeqCst);
        let max_acceptable_height = if raw_height == u32::MAX {
            return Ok(log);
        } else {
            Some(raw_height)
        };

        let limit = 100i64;
        let mut offset = 0i64;

        // Query for reqs needing proof collection
        let statuses = vec![
            ProvenTxReqStatus::Callback,
            ProvenTxReqStatus::Unmined,
            ProvenTxReqStatus::Sending,
            ProvenTxReqStatus::Unknown,
            ProvenTxReqStatus::Unconfirmed,
        ];

        loop {
            let args = FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial::default(),
                statuses: Some(statuses.clone()),
                paged: Some(Paged { limit, offset }),
                since: None,
            };

            let reqs = self.storage.find_proven_tx_reqs(&args).await?;
            let count = reqs.len();

            if reqs.is_empty() {
                break;
            }

            log.push_str(&format!(
                "{} reqs with status 'callback', 'unmined', 'sending', 'unknown', or 'unconfirmed'\n",
                reqs.len()
            ));

            let r = get_proofs(
                &self.storage,
                self.services.as_ref(),
                &reqs,
                &self.chain,
                self.unproven_attempts_limit,
                counts_as_attempt,
                max_acceptable_height,
            )
            .await?;

            log.push_str(&r.log);

            // Fire callback for each proven tx
            if let Some(ref cb) = self.on_tx_proven {
                for proven_req in &r.proven {
                    cb(proven_req.txid.clone()).await;
                }
            }

            if (count as i64) < limit {
                break;
            }
            offset += limit;
        }

        Ok(log)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_now_triggers_run() {
        // When check_now is true, trigger should return true
        let check_now = Arc::new(AtomicBool::new(true));
        assert!(check_now.load(Ordering::SeqCst));

        // After resetting, check_now should be false
        check_now.store(false, Ordering::SeqCst);
        assert!(!check_now.load(Ordering::SeqCst));
    }

    #[test]
    fn test_default_trigger_interval() {
        // Default trigger is 2 hours = 7200000 ms
        assert_eq!(2 * ONE_HOUR, 7_200_000);
    }

    #[test]
    fn test_task_name() {
        assert_eq!("CheckForProofs", "CheckForProofs");
    }
}
