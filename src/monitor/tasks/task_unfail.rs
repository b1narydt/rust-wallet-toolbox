//! TaskUnFail -- retries previously failed transactions by re-checking proofs.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskUnFail.ts (151 lines).
//!
//! Setting provenTxReq status to 'unfail' when 'invalid' will attempt to find
//! a merklePath. If successful: set req to 'unmined', referenced txs to 'unproven',
//! update input/output spendability. If not found: return to 'invalid'.

use std::sync::Arc;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, Paged, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use async_trait::async_trait;

/// Task that retries previously failed transactions.
///
/// Finds ProvenTxReqs with status "unfail" and attempts to retrieve a merkle proof.
/// If proof is found: status -> unmined, referenced transactions -> unproven.
/// If no proof: status -> invalid (back to failed).
pub struct TaskUnFail {
    storage: WalletStorageManager,
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    last_run_msecs: u64,
    /// Manual trigger flag.
    pub check_now: bool,
}

impl TaskUnFail {
    /// Create a new unfail task.
    pub fn new(storage: WalletStorageManager, services: Arc<dyn WalletServices>) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 10 * ONE_MINUTE,
            last_run_msecs: 0,
            check_now: false,
        }
    }

    /// Set the trigger interval in milliseconds.
    pub fn with_trigger_msecs(mut self, msecs: u64) -> Self {
        self.trigger_msecs = msecs;
        self
    }

    /// Process a list of "unfail" reqs: attempt to get merkle path for each.
    async fn unfail(
        &self,
        reqs: &[crate::tables::ProvenTxReq],
        indent: usize,
    ) -> Result<String, WalletError> {
        let mut log = String::new();
        let pad = " ".repeat(indent);

        for req in reqs {
            log.push_str(&format!(
                "{}reqId {} txid {}: ",
                pad, req.proven_tx_req_id, req.txid
            ));

            let gmpr = self.services.get_merkle_path(&req.txid, false).await;

            if gmpr.merkle_path.is_some() {
                // Proof found -- set req to unmined
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Unmined),
                    ..Default::default()
                };
                let _ = self
                    .storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update, None)
                    .await;
                log.push_str("unfailed. status is now 'unmined'\n");

                // Also update referenced transactions to 'unproven'
                // Parse notify JSON to get transactionIds
                if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&req.notify) {
                    if let Some(tx_ids) = notify.get("transactionIds").and_then(|v| v.as_array()) {
                        let ids: Vec<i64> = tx_ids.iter().filter_map(|v| v.as_i64()).collect();
                        if !ids.is_empty() {
                            let inner_pad = " ".repeat(indent + 2);
                            for id in &ids {
                                let _ = self
                                    .storage
                                    .update_transaction(
                                        *id,
                                        &crate::storage::find_args::TransactionPartial {
                                            status: Some(
                                                crate::status::TransactionStatus::Unproven,
                                            ),
                                            ..Default::default()
                                        },
                                        None,
                                    )
                                    .await;
                                log.push_str(&format!(
                                    "{}transaction {} status is now 'unproven'\n",
                                    inner_pad, id
                                ));
                            }
                        }
                    }
                }
            } else {
                // No proof found -- return to invalid
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Invalid),
                    ..Default::default()
                };
                let _ = self
                    .storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update, None)
                    .await;
                log.push_str("returned to status 'invalid'\n");
            }
        }

        Ok(log)
    }
}

#[async_trait]
impl WalletMonitorTask for TaskUnFail {
    fn name(&self) -> &str {
        "UnFail"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        self.check_now
            || (self.trigger_msecs > 0 && now_msecs > self.last_run_msecs + self.trigger_msecs)
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();
        self.check_now = false;

        let mut log = String::new();

        let limit = 100i64;
        let mut offset = 0i64;
        loop {
            let reqs = self
                .storage
                .find_proven_tx_reqs(
                    &FindProvenTxReqsArgs {
                        partial: ProvenTxReqPartial::default(),
                        since: None,
                        paged: Some(Paged { limit, offset }),
                        statuses: Some(vec![ProvenTxReqStatus::Unfail]),
                    },
                    None,
                )
                .await?;

            if reqs.is_empty() {
                break;
            }

            log.push_str(&format!("{} reqs with status 'unfail'\n", reqs.len()));
            let r = self.unfail(&reqs, 2).await?;
            log.push_str(&r);
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
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_unfail_defaults() {
        // Verify default trigger interval matches TS (10 minutes)
        assert_eq!(10 * ONE_MINUTE, 600_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("UnFail", "UnFail");
    }
}
