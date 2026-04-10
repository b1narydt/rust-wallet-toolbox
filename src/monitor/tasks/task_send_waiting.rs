//! TaskSendWaiting -- broadcasts pending unsent/sending ProvenTxReqs to the network.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskSendWaiting.ts (133 lines).
//!
//! Finds ProvenTxReqs with status "unsent" (and optionally "sending") that
//! have aged past `aged_msecs`, then calls attempt_to_post_reqs_to_network.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::attempt_to_post_reqs_to_network;
use crate::monitor::{AsyncCallback, ONE_MINUTE, ONE_SECOND};
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, Paged, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;
use crate::storage::traits::reader::StorageReader;
use crate::types::Chain;

use super::super::task_trait::WalletMonitorTask;

/// Background task that broadcasts pending transactions to the network.
///
/// Periodically queries for ProvenTxReqs with status "unsent" (and optionally
/// "sending" for retry), filters by age, and posts them via services.post_beef().
pub struct TaskSendWaiting {
    /// Storage manager for persistence operations.
    storage: WalletStorageManager,
    /// Network services for broadcasting.
    services: Arc<dyn WalletServices>,
    /// How often to trigger (default 8 seconds).
    trigger_msecs: u64,
    /// Only process reqs older than this (default 7 seconds).
    aged_msecs: u64,
    /// How often to include "sending" status reqs (default 5 minutes).
    sending_msecs: u64,
    /// Last time this task ran (epoch ms).
    last_run_msecs: u64,
    /// Last time "sending" reqs were included (epoch ms).
    last_sending_run_msecs: Option<u64>,
    /// Whether to include "sending" status in next run.
    include_sending: bool,
    /// Optional callback when a transaction is broadcasted.
    on_tx_broadcasted: Option<AsyncCallback<String>>,
    /// Chain (main/test) -- reserved for chain-specific broadcast logic.
    _chain: Chain,
}

impl TaskSendWaiting {
    /// Create a new TaskSendWaiting with default intervals.
    pub fn new(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        chain: Chain,
        on_tx_broadcasted: Option<AsyncCallback<String>>,
    ) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: ONE_SECOND * 8,
            aged_msecs: ONE_SECOND * 7,
            sending_msecs: ONE_MINUTE * 5,
            last_run_msecs: 0,
            last_sending_run_msecs: None,
            include_sending: true,
            on_tx_broadcasted,
            _chain: chain,
        }
    }

    /// Create with custom intervals.
    pub fn with_intervals(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        chain: Chain,
        trigger_msecs: u64,
        aged_msecs: u64,
        sending_msecs: u64,
        on_tx_broadcasted: Option<AsyncCallback<String>>,
    ) -> Self {
        Self {
            storage,
            services,
            trigger_msecs,
            aged_msecs,
            sending_msecs,
            last_run_msecs: 0,
            last_sending_run_msecs: None,
            include_sending: true,
            on_tx_broadcasted,
            _chain: chain,
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskSendWaiting {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "SendWaiting"
    }

    fn trigger(&mut self, now_msecs_since_epoch: u64) -> bool {
        // Determine whether to include "sending" status reqs this cycle
        self.include_sending = match self.last_sending_run_msecs {
            None => true,
            Some(last) => now_msecs_since_epoch > last + self.sending_msecs,
        };
        if self.include_sending {
            self.last_sending_run_msecs = Some(now_msecs_since_epoch);
        }

        // Check if trigger interval has elapsed
        if now_msecs_since_epoch > self.last_run_msecs + self.trigger_msecs {
            self.last_run_msecs = now_msecs_since_epoch;
            true
        } else {
            false
        }
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let mut log = String::new();
        let limit = 100i64;
        let mut offset = 0i64;

        // Calculate the age cutoff
        let aged_limit =
            chrono::Utc::now().naive_utc() - chrono::Duration::milliseconds(self.aged_msecs as i64);

        // Build status filter
        let statuses = if self.include_sending {
            vec![ProvenTxReqStatus::Unsent, ProvenTxReqStatus::Sending]
        } else {
            vec![ProvenTxReqStatus::Unsent]
        };

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
                "{} reqs with status {}\n",
                reqs.len(),
                statuses
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(" or ")
            ));

            // Filter by age: only process reqs updated before the cutoff
            let aged_reqs: Vec<_> = reqs
                .into_iter()
                .filter(|req| req.updated_at < aged_limit)
                .collect();

            log.push_str(&format!(
                "  Of those, {} aged past {} ms.\n",
                aged_reqs.len(),
                self.aged_msecs
            ));

            if !aged_reqs.is_empty() {
                let post_result = attempt_to_post_reqs_to_network(
                    &self.storage,
                    self.services.as_ref(),
                    &aged_reqs,
                )
                .await?;
                log.push_str(&post_result.log);

                // A failed tx-status cascade leaves the `transactions`
                // row stuck at its previous value (likely
                // `unprocessed`) even though the req advanced — the
                // exact visibility bug this PR is fixing, in a
                // narrower window. Surface it distinctly in the task
                // log so operators can detect the condition, and
                // attempt one best-effort retry (the req state is
                // already final, so we can't retry via
                // TaskSendWaiting's main path — this is the only
                // chance to recover before the next reboot).
                for detail in &post_result.details {
                    if detail.cascade_update_failed {
                        log.push_str(&format!(
                            "  WARN txid {} req {} cascade tx-status update \
                             failed; outputs may be temporarily hidden until \
                             next successful write\n",
                            detail.txid, detail.req_id
                        ));
                    }
                }

                // Fire callback for each successfully posted req
                if let Some(ref cb) = self.on_tx_broadcasted {
                    for detail in &post_result.details {
                        if detail.status == crate::monitor::helpers::PostReqStatus::Success {
                            cb(detail.txid.clone()).await;
                        }
                    }
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
    fn test_default_intervals() {
        // Verify the default intervals are correct
        assert_eq!(ONE_SECOND * 8, 8000);
        assert_eq!(ONE_SECOND * 7, 7000);
        assert_eq!(ONE_MINUTE * 5, 300_000);
    }

    #[test]
    fn test_task_name() {
        // Verify the task name constant
        assert_eq!("SendWaiting", "SendWaiting");
    }
}
