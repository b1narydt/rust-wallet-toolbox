//! TaskCheckNoSends -- processes nosend transactions by collecting proofs.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskCheckNoSends.ts.
//!
//! Unlike intentionally processed transactions, 'nosend' transactions are fully valid
//! transactions which have not been processed by the wallet. This task periodically
//! checks if any 'nosend' transaction has managed to get mined externally.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_DAY;
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, Paged, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;

/// Task that retrieves merkle proofs for 'nosend' transactions.
pub struct TaskCheckNoSends {
    storage: WalletStorageManager,
    services: Arc<dyn WalletServices>,
    chain: Chain,
    trigger_msecs: u64,
    last_run_msecs: u64,
    unproven_attempts_limit: u32,
    /// Manual trigger flag.
    pub check_now: bool,
    /// Shared last header height (u32::MAX = unknown). Same as TaskCheckForProofs.
    last_new_header_height: Arc<AtomicU32>,
}

impl TaskCheckNoSends {
    /// Create a new no-sends checking task.
    pub fn new(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        chain: Chain,
        unproven_attempts_limit: u32,
        last_new_header_height: Arc<AtomicU32>,
    ) -> Self {
        Self {
            storage,
            services,
            chain,
            trigger_msecs: ONE_DAY,
            last_run_msecs: 0,
            unproven_attempts_limit,
            check_now: false,
            last_new_header_height,
        }
    }

    /// Set the trigger interval in milliseconds.
    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }
}

#[async_trait]
impl WalletMonitorTask for TaskCheckNoSends {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "CheckNoSends"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        self.check_now
            || (self.trigger_msecs > 0 && now_msecs > self.last_run_msecs + self.trigger_msecs)
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();
        let counts_as_attempt = self.check_now;
        self.check_now = false;

        let mut log = String::new();

        // Match TS: skip proof checking when no header height is known.
        let raw_height = self.last_new_header_height.load(Ordering::SeqCst);
        let max_acceptable_height = if raw_height == u32::MAX {
            return Ok(log);
        } else {
            Some(raw_height)
        };

        let limit = 100i64;
        let mut offset = 0i64;
        loop {
            let reqs = self
                .storage
                .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial::default(),
                    since: None,
                    paged: Some(Paged { limit, offset }),
                    statuses: Some(vec![ProvenTxReqStatus::Nosend]),
                })
                .await?;

            if reqs.is_empty() {
                break;
            }

            log.push_str(&format!("{} reqs with status 'nosend'\n", reqs.len()));

            // Call get_proofs helper to attempt proof collection
            let r = crate::monitor::helpers::get_proofs(
                &self.storage,
                &*self.services,
                &reqs,
                &self.chain,
                self.unproven_attempts_limit,
                counts_as_attempt,
                max_acceptable_height,
            )
            .await?;
            log.push_str(&r.log);
            log.push('\n');

            if (reqs.len() as i64) < limit {
                break;
            }
            offset += limit;
        }

        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use crate::monitor::ONE_DAY;

    #[test]
    fn test_check_no_sends_defaults() {
        assert_eq!(ONE_DAY, 86_400_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("CheckNoSends", "CheckNoSends");
    }
}
