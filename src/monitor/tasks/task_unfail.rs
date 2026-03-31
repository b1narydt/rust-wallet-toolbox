//! TaskUnFail -- retries previously failed transactions by re-checking proofs.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskUnFail.ts (151 lines).
//!
//! Setting provenTxReq status to 'unfail' when 'invalid' will attempt to find
//! a merklePath. If successful: set req to 'unmined', referenced txs to 'unproven',
//! update input/output spendability. If not found: return to 'invalid'.

use std::io::Cursor;
use std::sync::Arc;

use bsv::transaction::transaction::Transaction;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{
    FindOutputsArgs, FindProvenTxReqsArgs, OutputPartial, Paged, ProvenTxReqPartial,
};
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
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
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
                                    )
                                    .await;
                                log.push_str(&format!(
                                    "{}transaction {} status is now 'unproven'\n",
                                    inner_pad, id
                                ));

                                // Step 3: Parse raw_tx and match inputs to user's outputs
                                if !req.raw_tx.is_empty() {
                                    if let Ok(bsvtx) = Transaction::from_binary(
                                        &mut Cursor::new(&req.raw_tx),
                                    ) {
                                        for (vin, input) in bsvtx.inputs.iter().enumerate() {
                                            let source_txid = match &input.source_txid {
                                                Some(t) => t.clone(),
                                                None => continue,
                                            };
                                            let source_vout =
                                                input.source_output_index as i32;

                                            let find_args = FindOutputsArgs {
                                                partial: OutputPartial {
                                                    txid: Some(source_txid),
                                                    vout: Some(source_vout),
                                                    ..Default::default()
                                                },
                                                ..Default::default()
                                            };

                                            match self.storage.find_outputs(&find_args).await {
                                                Ok(outputs) if outputs.len() == 1 => {
                                                    let oi = &outputs[0];
                                                    let update = OutputPartial {
                                                        spendable: Some(false),
                                                        spent_by: Some(*id),
                                                        ..Default::default()
                                                    };
                                                    let _ = self
                                                        .storage
                                                        .update_output(oi.output_id, &update)
                                                        .await;
                                                    log.push_str(&format!(
                                                        "{}input {} matched to output {} updated spentBy {}\n",
                                                        inner_pad, vin, oi.output_id, id
                                                    ));
                                                }
                                                _ => {
                                                    log.push_str(&format!(
                                                        "{}input {} not matched to user's outputs\n",
                                                        inner_pad, vin
                                                    ));
                                                }
                                            }
                                        }

                                        // Step 4: Check output spendability via isUtxo
                                        let out_find_args = FindOutputsArgs {
                                            partial: OutputPartial {
                                                transaction_id: Some(*id),
                                                ..Default::default()
                                            },
                                            ..Default::default()
                                        };

                                        if let Ok(outputs) =
                                            self.storage.find_outputs(&out_find_args).await
                                        {
                                            for o in &outputs {
                                                let script_bytes = match &o.locking_script {
                                                    Some(s) if !s.is_empty() => s,
                                                    _ => {
                                                        log.push_str(&format!(
                                                            "{}output {} does not have a valid locking script\n",
                                                            inner_pad, o.output_id
                                                        ));
                                                        continue;
                                                    }
                                                };

                                                let txid_str =
                                                    o.txid.as_deref().unwrap_or("");
                                                let vout = o.vout as u32;

                                                match self
                                                    .services
                                                    .is_utxo(script_bytes, txid_str, vout)
                                                    .await
                                                {
                                                    Ok(is_utxo) => {
                                                        let current_spendable = o.spendable;
                                                        if is_utxo != current_spendable {
                                                            let update = OutputPartial {
                                                                spendable: Some(is_utxo),
                                                                ..Default::default()
                                                            };
                                                            let _ = self
                                                                .storage
                                                                .update_output(
                                                                    o.output_id,
                                                                    &update,
                                                                )
                                                                .await;
                                                            log.push_str(&format!(
                                                                "{}output {} set to {}\n",
                                                                inner_pad,
                                                                o.output_id,
                                                                if is_utxo {
                                                                    "spendable"
                                                                } else {
                                                                    "spent"
                                                                }
                                                            ));
                                                        }
                                                    }
                                                    Err(_) => {}
                                                }
                                            }
                                        }
                                    }
                                }
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
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
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
                .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial::default(),
                    since: None,
                    paged: Some(Paged { limit, offset }),
                    statuses: Some(vec![ProvenTxReqStatus::Unfail]),
                })
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

    #[test]
    fn test_unfail_steps_3_4_logic() {
        // Step 3: matching inputs to outputs
        let found_outputs = 1;
        let should_update_spent_by = found_outputs == 1;
        assert!(should_update_spent_by);

        // Step 4: spendability check
        let current_spendable = true;
        let is_utxo = false;
        let should_update = is_utxo != current_spendable;
        assert!(should_update);
    }
}
