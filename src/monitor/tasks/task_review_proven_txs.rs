//! TaskReviewProvenTxs -- audits proven_txs merkle roots against the canonical chain.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewProvenTxs.ts.
//!
//! Walks block heights from `last_reviewed_height` up to `tip - min_block_age`,
//! finds proven txs at each height, and reproves any whose merkle root doesn't
//! match the canonical header via the chain tracker.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::storage::find_args::{FindProvenTxsArgs, ProvenTxPartial};
use crate::storage::manager::WalletStorageManager;

/// Audits proven transaction merkle roots against the canonical chain.
///
/// Walks block heights in batches, comparing stored merkle roots to canonical
/// block headers via `ChainTracker::is_valid_root_for_height`. Mismatched
/// transactions are reproved via `reprove_proven`.
pub struct TaskReviewProvenTxs {
    storage: Arc<WalletStorageManager>,
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    trigger_quick_msecs: u64,
    last_run_msecs: u64,
    /// The last block height that was fully reviewed.
    last_reviewed_height: u32,
    /// Maximum number of heights to process per run.
    max_heights_per_run: u32,
    /// Minimum number of blocks behind the tip before a height is eligible.
    min_block_age: u32,
    /// When true, use the quick trigger interval.
    use_quick_trigger: bool,
}

impl TaskReviewProvenTxs {
    /// Create a new proven-tx review task.
    pub fn new(storage: Arc<WalletStorageManager>, services: Arc<dyn WalletServices>) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 10 * ONE_MINUTE,
            trigger_quick_msecs: ONE_MINUTE,
            last_run_msecs: 0,
            last_reviewed_height: 0,
            max_heights_per_run: 100,
            min_block_age: 100,
            use_quick_trigger: false,
        }
    }

    /// Current effective trigger interval.
    fn effective_trigger(&self) -> u64 {
        if self.use_quick_trigger {
            self.trigger_quick_msecs
        } else {
            self.trigger_msecs
        }
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewProvenTxs {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewProvenTxs"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        let interval = self.effective_trigger();
        interval > 0 && now_msecs > self.last_run_msecs + interval
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();

        let tip = self.services.get_height().await?;
        let max_eligible = tip.saturating_sub(self.min_block_age);

        if max_eligible == 0 || self.last_reviewed_height >= max_eligible {
            self.use_quick_trigger = false;
            return Ok(format!(
                "tip={tip}, max_eligible={max_eligible}, nothing to review"
            ));
        }

        let start = self.last_reviewed_height + 1;
        let end = max_eligible.min(start + self.max_heights_per_run - 1);

        // Obtain chain tracker for merkle root validation.
        let chain_tracker = self.services.get_chain_tracker().await?;

        let mut reviewed_heights = 0u32;
        let mut mismatched_heights = 0u32;
        let mut affected_txs = 0u32;
        let mut reproved_txs = 0u32;
        let mut log = String::new();

        for height in start..=end {
            reviewed_heights += 1;

            // Find all proven txs stored at this height.
            let ptxs = self
                .storage
                .find_proven_txs(&FindProvenTxsArgs {
                    partial: ProvenTxPartial {
                        height: Some(height as i32),
                        ..Default::default()
                    },
                    since: None,
                    paged: None,
                })
                .await?;

            if ptxs.is_empty() {
                continue;
            }

            // Check each proven tx's merkle root against the canonical chain.
            let mut height_has_mismatch = false;

            for ptx in &ptxs {
                // Distinguish "chain tracker says this root is wrong" (Ok(false))
                // from "chain tracker is unavailable" (Err). Collapsing Err into
                // `false` (as a plain `.unwrap_or(false)` would) turns a full
                // chain-tracker outage into a flood of false-positive mismatches,
                // which then get force-reproved. That's PR review IMPORTANT #7:
                // an outage must NOT count as a mismatch. Per-tx skip on Err is
                // sufficient for correctness.
                let is_valid = match chain_tracker
                    .is_valid_root_for_height(&ptx.merkle_root, height)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            txid = %ptx.txid,
                            height,
                            error = %e,
                            "TaskReviewProvenTxs: chain tracker error - skipping (treating as unknown, not mismatch)"
                        );
                        continue;
                    }
                };

                if is_valid {
                    continue;
                }

                // Merkle root doesn't match canonical — reprove this tx.
                affected_txs += 1;
                if !height_has_mismatch {
                    mismatched_heights += 1;
                    height_has_mismatch = true;
                }

                let result = self.storage.reprove_proven(ptx, false).await?;
                if result.updated.is_some() {
                    reproved_txs += 1;
                    log += &format!("  reproved txid={} at height {height}\n", ptx.txid);
                } else if result.unavailable {
                    log += &format!("  unavailable txid={} at height {height}\n", ptx.txid);
                }
            }
        }

        self.last_reviewed_height = end;
        self.use_quick_trigger = end < max_eligible;

        Ok(format!(
            "tip={tip}, reviewed heights {start}..={end}: \
             {reviewed_heights} checked, {mismatched_heights} mismatched, \
             {affected_txs} affected, {reproved_txs} reproved\n{log}"
        ))
    }
}

#[cfg(test)]
mod tests {
    //! Behavioral tests for `TaskReviewProvenTxs`.
    //!
    //! Exercises merkle-root auditing against a scripted mock chain tracker.
    //! Covers the `mismatched_heights` counter fix from commit f0b7c12 and
    //! the PR review IMPORTANT #7 concern about chain-tracker outages being
    //! mistakenly treated as mismatches.

    use super::*;
    use crate::error::WalletResult;
    use crate::services::traits::WalletServices;
    use crate::services::types::{
        BlockHeader, BsvExchangeRate, FiatExchangeRates, GetMerklePathResult, GetRawTxResult,
        GetScriptHashHistoryResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat,
        GetUtxoStatusResult, NLockTimeInput, PostBeefResult, ServicesCallHistory,
    };
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::wallet_provider::WalletStorageProvider;
    use crate::storage::StorageConfig;
    use crate::tables::ProvenTx;
    use crate::types::Chain;
    use async_trait::async_trait;
    use bsv::transaction::chain_tracker::ChainTracker;
    use bsv::transaction::error::TransactionError;
    use bsv::transaction::Beef;
    use chrono::Utc;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // Scripted chain tracker.
    //
    // Behaviour flags:
    //   * `all_valid = true`  -> every root is valid
    //   * `all_error = true`  -> every lookup returns Err (outage)
    //   * otherwise: exact `valid_roots` hash set determines the answer.
    // -----------------------------------------------------------------------

    struct ScriptedTracker {
        all_valid: bool,
        all_error: bool,
        valid_roots: std::collections::HashSet<String>,
        call_count: Mutex<u32>,
    }

    #[async_trait]
    impl ChainTracker for ScriptedTracker {
        async fn is_valid_root_for_height(
            &self,
            root: &str,
            _height: u32,
        ) -> Result<bool, TransactionError> {
            *self.call_count.lock().unwrap() += 1;
            if self.all_error {
                return Err(TransactionError::InvalidFormat("outage".to_string()));
            }
            if self.all_valid {
                return Ok(true);
            }
            Ok(self.valid_roots.contains(root))
        }
    }

    // -----------------------------------------------------------------------
    // Minimal WalletServices that yields a configured tip height and tracker.
    // -----------------------------------------------------------------------

    struct MockServices {
        height: u32,
        /// A boxed tracker factory, invoked on every `get_chain_tracker` call.
        tracker: Mutex<Option<Box<dyn ChainTracker>>>,
    }

    #[async_trait]
    impl WalletServices for MockServices {
        fn chain(&self) -> Chain {
            Chain::Test
        }
        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            self.tracker.lock().unwrap().take().ok_or_else(|| {
                WalletError::NotImplemented(
                    "tracker already taken — test expected only one call".into(),
                )
            })
        }
        async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> GetMerklePathResult {
            GetMerklePathResult::default()
        }
        async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> GetRawTxResult {
            GetRawTxResult::default()
        }
        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> Vec<PostBeefResult> {
            vec![]
        }
        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> GetUtxoStatusResult {
            GetUtxoStatusResult {
                name: "mock".into(),
                status: "error".into(),
                error: Some("mock".into()),
                is_utxo: None,
                details: vec![],
            }
        }
        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> GetStatusForTxidsResult {
            GetStatusForTxidsResult {
                name: "mock".into(),
                status: "error".into(),
                error: Some("mock".into()),
                results: vec![],
            }
        }
        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> GetScriptHashHistoryResult {
            GetScriptHashHistoryResult {
                name: "mock".into(),
                status: "error".into(),
                error: Some("mock".into()),
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
            Ok(self.height)
        }
        async fn n_lock_time_is_final(&self, _input: NLockTimeInput) -> WalletResult<bool> {
            Ok(true)
        }
        async fn get_bsv_exchange_rate(&self) -> WalletResult<BsvExchangeRate> {
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
            _target_currencies: &[String],
        ) -> WalletResult<FiatExchangeRates> {
            Err(WalletError::NotImplemented("mock".into()))
        }
        fn get_services_call_history(&self, _reset: bool) -> ServicesCallHistory {
            ServicesCallHistory { services: vec![] }
        }
        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
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

    // -----------------------------------------------------------------------
    // Storage fixtures
    // -----------------------------------------------------------------------

    async fn make_manager_and_storage() -> (Arc<WalletStorageManager>, Arc<SqliteStorage>) {
        let cfg = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(cfg, Chain::Test).await.unwrap();
        StorageProvider::migrate_database(&storage).await.unwrap();
        let storage = Arc::new(storage);
        let provider: Arc<dyn WalletStorageProvider> = storage.clone();
        let mgr = Arc::new(WalletStorageManager::new(
            "02aabbcc".to_string(),
            Some(provider),
            vec![],
        ));
        mgr.make_available().await.unwrap();
        (mgr, storage)
    }

    /// Insert a ProvenTx at `height` with the given `merkle_root`. Returns id.
    async fn insert_proven_tx(
        storage: &SqliteStorage,
        txid: &str,
        height: i32,
        merkle_root: &str,
    ) -> i64 {
        use crate::storage::traits::reader_writer::StorageReaderWriter;
        let now = Utc::now().naive_utc();
        let ptx = ProvenTx {
            created_at: now,
            updated_at: now,
            proven_tx_id: 0,
            txid: txid.to_string(),
            height,
            index: 0,
            merkle_path: vec![1, 2, 3],
            raw_tx: vec![4, 5, 6],
            block_hash: format!("blockhash_{txid}"),
            merkle_root: merkle_root.to_string(),
        };
        storage.insert_proven_tx(&ptx, None).await.unwrap()
    }

    // =======================================================================
    // Tests
    // =======================================================================

    /// Regression test for commit f0b7c12 — `mismatched_heights` must equal
    /// the number of heights with at least one mismatched tx, not the number
    /// of mismatched txs. Seeds four txs at two heights, half valid.
    ///
    /// The run log is the only observable surface of the counter, so we
    /// parse it. If this test fails because the log format changed, update
    /// the parser — do NOT loosen the numeric assertions.
    #[tokio::test]
    async fn test_mismatched_heights_counted_per_height_not_per_tx() {
        let (mgr, storage) = make_manager_and_storage().await;

        // Height 101: one valid + one mismatched -> counts as 1 mismatched height.
        insert_proven_tx(&storage, "tx_101_a", 101, "root_valid_a").await;
        insert_proven_tx(&storage, "tx_101_b", 101, "root_bad_b").await;
        // Height 102: both mismatched -> counts as 1 mismatched height.
        insert_proven_tx(&storage, "tx_102_a", 102, "root_bad_c").await;
        insert_proven_tx(&storage, "tx_102_b", 102, "root_bad_d").await;

        let mut valid = std::collections::HashSet::new();
        valid.insert("root_valid_a".to_string());
        let tracker = Box::new(ScriptedTracker {
            all_valid: false,
            all_error: false,
            valid_roots: valid,
            call_count: Mutex::new(0),
        });

        // Tip = 300 so max_eligible = 200 covers heights 101-102.
        let services: Arc<dyn WalletServices> = Arc::new(MockServices {
            height: 300,
            tracker: Mutex::new(Some(tracker)),
        });
        mgr.set_services(services.clone()).await;

        let mut task = TaskReviewProvenTxs::new(mgr.clone(), services);
        // Shrink scan range so we don't iterate 100 empty heights.
        task.last_reviewed_height = 100;
        task.max_heights_per_run = 2;

        let log = task.run_task().await.unwrap();

        // Expected: 2 heights reviewed, 2 mismatched heights, 3 affected txs.
        assert!(
            log.contains("2 mismatched"),
            "mismatched_heights must be 2 (one per height with any mismatch); log = {log}"
        );
        assert!(
            log.contains("3 affected"),
            "affected_txs must be 3 (individual mismatched txs); log = {log}"
        );
        assert!(
            !log.contains("4 mismatched") && !log.contains("0 mismatched"),
            "mismatched_heights must not equal total-tx-count or zero; log = {log}"
        );
    }

    /// PR review IMPORTANT #7: a chain-tracker outage (Err) must NOT be
    /// treated as a mismatch. Currently `is_valid_root_for_height(...).unwrap_or(false)`
    /// collapses Err → false, which WOULD count every tx as mismatched.
    /// We don't call `reprove_proven` here because the mock services can't
    /// provide a merkle path — but we can observe that the task proceeds to
    /// try reprove (`affected_txs` > 0), which is the bug.
    ///
    /// This test is expected to FAIL against current code, surfacing the
    /// PR #7 regression. Per instructions: do NOT loosen the assertion;
    /// document the failure.
    #[tokio::test]
    async fn test_chain_tracker_outage_does_not_count_as_mismatch() {
        let (mgr, storage) = make_manager_and_storage().await;
        insert_proven_tx(&storage, "tx_a", 101, "some_root").await;
        insert_proven_tx(&storage, "tx_b", 101, "other_root").await;

        let tracker = Box::new(ScriptedTracker {
            all_valid: false,
            all_error: true,
            valid_roots: Default::default(),
            call_count: Mutex::new(0),
        });
        let services: Arc<dyn WalletServices> = Arc::new(MockServices {
            height: 300,
            tracker: Mutex::new(Some(tracker)),
        });
        mgr.set_services(services.clone()).await;

        let mut task = TaskReviewProvenTxs::new(mgr.clone(), services);
        task.last_reviewed_height = 100;
        task.max_heights_per_run = 1;

        // If the outage is treated correctly, the task will either short-circuit
        // or skip affected counting. If it is treated as mismatch, `reprove_proven`
        // will be attempted and fail due to unavailable merkle path → log "unavailable".
        let result = task.run_task().await;

        // If the code treated outage properly, run_task would succeed with
        // `0 affected`. If it treats outage as mismatch, it would try reprove
        // and log "unavailable" (because mock services have no merkle path).
        match result {
            Ok(log) => {
                assert!(
                    log.contains("0 affected"),
                    "chain-tracker outage must NOT mark any tx as affected; log = {log}"
                );
            }
            Err(e) => {
                panic!(
                    "chain-tracker outage should not propagate an error from run_task; got {e:?}"
                );
            }
        }
    }

    /// All roots valid on a populated height — no mismatches, no reprove.
    #[tokio::test]
    async fn test_all_roots_valid_produces_no_mismatches() {
        let (mgr, storage) = make_manager_and_storage().await;
        insert_proven_tx(&storage, "tx_a", 101, "root_a").await;
        insert_proven_tx(&storage, "tx_b", 101, "root_b").await;

        let tracker = Box::new(ScriptedTracker {
            all_valid: true,
            all_error: false,
            valid_roots: Default::default(),
            call_count: Mutex::new(0),
        });
        let services: Arc<dyn WalletServices> = Arc::new(MockServices {
            height: 300,
            tracker: Mutex::new(Some(tracker)),
        });
        mgr.set_services(services.clone()).await;

        let mut task = TaskReviewProvenTxs::new(mgr.clone(), services);
        task.last_reviewed_height = 100;
        task.max_heights_per_run = 1;

        let log = task.run_task().await.unwrap();
        assert!(log.contains("0 mismatched"), "log = {log}");
        assert!(log.contains("0 affected"), "log = {log}");
        // last_reviewed_height advances to 101 after processing.
        assert_eq!(task.last_reviewed_height, 101);
    }

    /// Tip too low for eligibility: task short-circuits without querying
    /// the chain tracker. `last_reviewed_height` must NOT advance.
    #[tokio::test]
    async fn test_tip_below_min_block_age_is_noop() {
        let (mgr, _storage) = make_manager_and_storage().await;

        let tracker = Box::new(ScriptedTracker {
            all_valid: true,
            all_error: false,
            valid_roots: Default::default(),
            call_count: Mutex::new(0),
        });
        let services: Arc<dyn WalletServices> = Arc::new(MockServices {
            height: 50, // below default min_block_age (100)
            tracker: Mutex::new(Some(tracker)),
        });
        mgr.set_services(services.clone()).await;

        let mut task = TaskReviewProvenTxs::new(mgr.clone(), services);
        let log = task.run_task().await.unwrap();
        assert!(
            log.contains("nothing to review"),
            "expected 'nothing to review' short-circuit; got: {log}"
        );
        assert_eq!(
            task.last_reviewed_height, 0,
            "must not advance when no heights are eligible"
        );
    }
}
