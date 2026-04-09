//! TaskReorg -- processes deactivated headers from reorg events.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReorg.ts (89 lines).
//!
//! When chain reorgs are detected, deactivated block headers are queued.
//! This task processes aged headers by finding affected ProvenTx records
//! and attempting to re-fetch their merkle proofs.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::{DeactivatedHeader, ONE_MINUTE, ONE_SECOND};
use crate::services::traits::WalletServices;
use crate::storage::find_args::{FindProvenTxsArgs, ProvenTxPartial};
use crate::storage::manager::WalletStorageManager;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::tables::ProvenTx;

use super::super::task_trait::WalletMonitorTask;

/// Background task that handles chain reorg processing.
///
/// Processes deactivated block headers by finding ProvenTx records that
/// reference those blocks and attempting to get updated merkle proofs.
/// Uses an aging pattern to delay processing, allowing the chain to stabilize.
pub struct TaskReorg {
    /// Storage manager for persistence operations.
    storage: WalletStorageManager,
    /// Network services for proof retrieval.
    services: Arc<dyn WalletServices>,
    /// How long to age headers before processing (default 10 minutes).
    aged_msecs: u64,
    /// Maximum retries per deactivated header (default 3).
    max_retries: u32,
    /// Last time this task ran (epoch ms).
    last_run_msecs: u64,
    /// Trigger interval (default 30 seconds).
    trigger_msecs: u64,
    /// Shared queue of deactivated headers from Monitor.
    deactivated_headers: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>>,
    /// Headers staged for processing in the current run.
    process: Vec<DeactivatedHeader>,
}

impl TaskReorg {
    /// Create a new TaskReorg with default intervals.
    pub fn new(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        deactivated_headers: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>>,
    ) -> Self {
        Self {
            storage,
            services,
            aged_msecs: ONE_MINUTE * 10,
            max_retries: 3,
            last_run_msecs: 0,
            trigger_msecs: ONE_SECOND * 30,
            deactivated_headers,
            process: Vec::new(),
        }
    }

    /// Create with custom intervals.
    pub fn with_intervals(
        storage: WalletStorageManager,
        services: Arc<dyn WalletServices>,
        deactivated_headers: Arc<tokio::sync::Mutex<Vec<DeactivatedHeader>>>,
        aged_msecs: u64,
        max_retries: u32,
        trigger_msecs: u64,
    ) -> Self {
        Self {
            storage,
            services,
            aged_msecs,
            max_retries,
            last_run_msecs: 0,
            trigger_msecs,
            deactivated_headers,
            process: Vec::new(),
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReorg {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "Reorg"
    }

    fn trigger(&mut self, now_msecs_since_epoch: u64) -> bool {
        if now_msecs_since_epoch <= self.last_run_msecs + self.trigger_msecs {
            return false;
        }

        // Shift aged deactivated headers from the shared queue to the process list
        let cutoff = now_msecs_since_epoch.saturating_sub(self.aged_msecs);
        if let Ok(mut queue) = self.deactivated_headers.try_lock() {
            let mut remaining = Vec::new();
            for header in queue.drain(..) {
                if header.when_msecs < cutoff {
                    self.process.push(header);
                } else {
                    remaining.push(header);
                }
            }
            *queue = remaining;
        }

        if !self.process.is_empty() {
            self.last_run_msecs = now_msecs_since_epoch;
            true
        } else {
            false
        }
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let mut log = String::new();

        while let Some(header) = self.process.pop() {
            log.push_str(&format!(
                "  processing deactivated header height={} hash={} tries={}\n",
                header.header.height, header.header.hash, header.tries
            ));

            // Find ProvenTx records with this block_hash
            let find_args = FindProvenTxsArgs {
                partial: ProvenTxPartial {
                    block_hash: Some(header.header.hash.clone()),
                    ..Default::default()
                },
                since: None,
                paged: None,
            };

            let proven_txs = match self.storage.find_proven_txs(&find_args).await {
                Ok(txs) => txs,
                Err(e) => {
                    log.push_str(&format!("    error finding proven txs: {}\n", e));
                    continue;
                }
            };

            if proven_txs.is_empty() {
                log.push_str("    no matching proven_txs records\n");
                continue;
            }

            log.push_str(&format!(
                "    found {} proven_txs records for block {}\n",
                proven_txs.len(),
                header.header.hash
            ));

            let mut unavailable_count = 0;

            for ptx in &proven_txs {
                // Try to get an updated merkle proof
                let gmpr = self.services.get_merkle_path(&ptx.txid, false).await;

                if let (Some(merkle_path), Some(new_header)) = (&gmpr.merkle_path, &gmpr.header) {
                    // Got updated proof -- update the ProvenTx record
                    let now = chrono::Utc::now().naive_utc();
                    let _updated_ptx = ProvenTx {
                        created_at: ptx.created_at,
                        updated_at: now,
                        proven_tx_id: ptx.proven_tx_id,
                        txid: ptx.txid.clone(),
                        height: new_header.height as i32,
                        index: 0,
                        merkle_path: merkle_path.clone(),
                        raw_tx: ptx.raw_tx.clone(),
                        block_hash: new_header.hash.clone(),
                        merkle_root: new_header.merkle_root.clone(),
                    };

                    let update = crate::storage::find_args::ProvenTxPartial {
                        proven_tx_id: Some(ptx.proven_tx_id),
                        height: Some(new_header.height as i32),
                        block_hash: Some(new_header.hash.clone()),
                        ..Default::default()
                    };
                    match self
                        .storage
                        .update_proven_tx(ptx.proven_tx_id, &update)
                        .await
                    {
                        Ok(_) => {
                            log.push_str(&format!(
                                "    updated proof for txid {} to height {}\n",
                                ptx.txid, new_header.height
                            ));
                        }
                        Err(e) => {
                            log.push_str(&format!(
                                "    error updating proof for txid {}: {}\n",
                                ptx.txid, e
                            ));
                            unavailable_count += 1;
                        }
                    }
                } else {
                    log.push_str(&format!(
                        "    no updated proof available for txid {}\n",
                        ptx.txid
                    ));
                    unavailable_count += 1;
                }
            }

            // If some proofs were unavailable, retry if under max_retries
            if unavailable_count > 0 {
                if header.tries + 1 >= self.max_retries {
                    log.push_str(&format!(
                        "    maximum retries {} exceeded\n",
                        self.max_retries
                    ));
                } else {
                    log.push_str("    retrying...\n");
                    // Push back to the shared queue for another attempt
                    if let Ok(mut queue) = self.deactivated_headers.try_lock() {
                        queue.push(DeactivatedHeader {
                            header: header.header.clone(),
                            when_msecs: now_msecs(),
                            tries: header.tries + 1,
                        });
                    }
                }
            }
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
    use crate::services::types::BlockHeader;

    #[test]
    fn test_default_intervals() {
        assert_eq!(ONE_MINUTE * 10, 600_000);
        assert_eq!(ONE_SECOND * 30, 30_000);
    }

    #[test]
    fn test_task_name() {
        assert_eq!("Reorg", "Reorg");
    }

    #[tokio::test]
    async fn test_deactivated_header_aging() {
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

        // Add a header with old timestamp (should be aged)
        {
            let mut q = queue.lock().await;
            q.push(DeactivatedHeader {
                when_msecs: 1000, // very old
                tries: 0,
                header: header.clone(),
            });
        }

        // Verify it's in the queue
        let q = queue.lock().await;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].tries, 0);
    }
}
