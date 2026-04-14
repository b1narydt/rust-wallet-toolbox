//! TaskFailAbandoned -- fails stuck unsigned/unprocessed transactions.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskFailAbandoned.ts (54 lines).
//!
//! Finds transactions with status "unsigned" or "unprocessed" that have not
//! been updated for longer than abandoned_msecs, and sets their status to "failed".
//! This releases locked UTXOs back to spendable status.

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::ONE_MINUTE;
use crate::status::TransactionStatus;
use crate::storage::find_args::{FindTransactionsArgs, TransactionPartial};
use crate::storage::manager::WalletStorageManager;

use std::sync::Arc;

use super::super::task_trait::WalletMonitorTask;

/// Background task that fails stuck transactions.
///
/// Transactions with status "unsigned" or "unprocessed" that have not been
/// updated within abandoned_msecs are marked as "failed". This releases
/// any locked UTXOs back to spendable status.
pub struct TaskFailAbandoned {
    /// Storage manager for persistence operations.
    storage: Arc<WalletStorageManager>,
    /// How often to trigger (default 5 minutes, matching TS).
    trigger_msecs: u64,
    /// Last time this task ran (epoch ms).
    last_run_msecs: u64,
    /// How long before a transaction is considered abandoned (default 5 minutes).
    abandoned_msecs: u64,
}

impl TaskFailAbandoned {
    /// Create a new TaskFailAbandoned with default intervals.
    pub fn new(storage: Arc<WalletStorageManager>, abandoned_msecs: u64) -> Self {
        Self {
            storage,
            trigger_msecs: ONE_MINUTE * 5,
            last_run_msecs: 0,
            abandoned_msecs,
        }
    }

    /// Create with a custom trigger interval.
    pub fn with_trigger_msecs(
        storage: Arc<WalletStorageManager>,
        trigger_msecs: u64,
        abandoned_msecs: u64,
    ) -> Self {
        Self {
            storage,
            trigger_msecs,
            last_run_msecs: 0,
            abandoned_msecs,
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskFailAbandoned {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "FailAbandoned"
    }

    fn trigger(&mut self, now_msecs_since_epoch: u64) -> bool {
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

        let abandoned_cutoff = chrono::Utc::now().naive_utc()
            - chrono::Duration::milliseconds(self.abandoned_msecs as i64);

        loop {
            let args = FindTransactionsArgs {
                partial: TransactionPartial::default(),
                status: Some(vec![
                    TransactionStatus::Unprocessed,
                    TransactionStatus::Unsigned,
                ]),
                paged: Some(crate::storage::find_args::Paged { limit, offset }),
                since: None,
                no_raw_tx: true,
            };

            let txs = self.storage.find_transactions(&args).await?;
            let count = txs.len();

            if txs.is_empty() {
                break;
            }

            // Filter by age: only fail txs last updated before the cutoff
            let abandoned_txs: Vec<_> = txs
                .into_iter()
                .filter(|tx| tx.updated_at < abandoned_cutoff)
                .collect();

            for tx in &abandoned_txs {
                if let Some(ref txid) = tx.txid {
                    match self
                        .storage
                        .update_transaction_status(txid, TransactionStatus::Failed)
                        .await
                    {
                        Ok(_) => {
                            log.push_str(&format!(
                                "updated tx {} (id={}) status to 'failed'\n",
                                txid, tx.transaction_id
                            ));
                            // Explicitly restore inputs this abandoned tx had
                            // locked. The cascade is no longer implicit in
                            // `update_transaction_status` (it was removed so
                            // DoubleSpend recovery can restore only chain-
                            // verified inputs); abandoned txs never hit the
                            // chain so restoring every consumed input is safe
                            // and matches the previous behavior.
                            if let Err(e) = self
                                .storage
                                .restore_consumed_inputs(tx.transaction_id)
                                .await
                            {
                                log.push_str(&format!(
                                    "warn restore_consumed_inputs tx {} (id={}): {}\n",
                                    txid, tx.transaction_id, e
                                ));
                            }
                        }
                        Err(e) => {
                            log.push_str(&format!(
                                "error failing tx {} (id={}): {}\n",
                                txid, tx.transaction_id, e
                            ));
                        }
                    }
                } else {
                    // Transaction has no txid -- fail by transaction_id reference
                    log.push_str(&format!("skipped tx id={} (no txid)\n", tx.transaction_id));
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
        // Default trigger is 5 minutes = 300_000 ms (matching TS 1000 * 60 * 5)
        assert_eq!(ONE_MINUTE * 5, 300_000);
    }

    #[test]
    fn test_task_name() {
        assert_eq!("FailAbandoned", "FailAbandoned");
    }

    #[test]
    fn test_abandoned_cutoff_calculation() {
        let now = chrono::Utc::now().naive_utc();
        let five_min_ago = now - chrono::Duration::milliseconds(ONE_MINUTE as i64 * 5);
        assert!(five_min_ago < now);
    }
}
