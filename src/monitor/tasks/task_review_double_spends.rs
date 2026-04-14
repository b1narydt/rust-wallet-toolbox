//! TaskReviewDoubleSpends -- reviews double-spend flagged transactions and unfails false positives.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewDoubleSpends.ts.
//!
//! Queries ProvenTxReqs with status "doubleSpend", re-verifies each via
//! `get_status_for_txids`, and marks non-"unknown" results as "unfail" so the
//! normal unfail pipeline can retry proof acquisition.
//!
//! Persists review progress to the monitor_events table so that interrupted
//! runs resume from where they left off, matching the TS checkpoint pattern.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::helpers::now_msecs;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::ONE_MINUTE;
use crate::services::traits::WalletServices;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{
    FindMonitorEventsArgs, FindProvenTxReqsArgs, MonitorEventPartial, Paged, ProvenTxReqPartial,
};
use crate::storage::manager::WalletStorageManager;
use crate::tables::MonitorEvent;

/// Reviews ProvenTxReqs flagged as double-spends and unfails false positives.
///
/// For each req with status `DoubleSpend`, calls `get_status_for_txids` to
/// re-verify. If the txid is found on-chain (status != "unknown"), sets the
/// req status to `Unfail` so the normal unfail pipeline retries proof acquisition.
///
/// Progress is checkpointed to the monitor_events table so that interrupted
/// runs resume from the last position.
pub struct TaskReviewDoubleSpends {
    storage: Arc<WalletStorageManager>,
    services: Arc<dyn WalletServices>,
    trigger_msecs: u64,
    trigger_quick_msecs: u64,
    last_run_msecs: u64,
    review_limit: i64,
    /// Minimum age in minutes before a doubleSpend req is eligible for review.
    /// Matches TS `minAgeMinutes` (default 60).
    min_age_minutes: u64,
    /// When true, use the quick trigger interval (last batch was full).
    use_quick_trigger: bool,
}

/// Checkpoint persisted to monitor_events for resume-on-restart.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewDoubleSpendCheckpoint {
    resume_offset: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_proven_tx_req_id: Option<i64>,
    review_limit: i64,
    min_age_minutes: u64,
    reviewed: u64,
    unfails: u64,
}

impl TaskReviewDoubleSpends {
    /// Create a new double-spend review task.
    pub fn new(storage: Arc<WalletStorageManager>, services: Arc<dyn WalletServices>) -> Self {
        Self {
            storage,
            services,
            trigger_msecs: 12 * ONE_MINUTE,
            trigger_quick_msecs: ONE_MINUTE,
            last_run_msecs: 0,
            review_limit: 100,
            min_age_minutes: 60,
            use_quick_trigger: false,
        }
    }

    /// Current effective trigger interval based on whether the last batch was full.
    fn effective_trigger(&self) -> u64 {
        if self.use_quick_trigger {
            self.trigger_quick_msecs
        } else {
            self.trigger_msecs
        }
    }

    /// Retrieve the last checkpoint from monitor_events, if any.
    async fn get_last_checkpoint(&self) -> Result<Option<(i64, Option<i64>)>, WalletError> {
        let events = self
            .storage
            .find_monitor_events(&FindMonitorEventsArgs {
                partial: MonitorEventPartial {
                    id: None,
                    event: Some("ReviewDoubleSpends".to_string()),
                },
                since: None,
                paged: Some(Paged {
                    limit: 5,
                    offset: 0,
                }),
            })
            .await?;

        for event in &events {
            if let Some(ref details) = event.details {
                if let Ok(cp) = serde_json::from_str::<ReviewDoubleSpendCheckpoint>(details) {
                    return Ok(Some((cp.resume_offset, cp.expected_proven_tx_req_id)));
                }
            }
        }
        Ok(None)
    }

    /// Delete old checkpoint events, keeping only the most recent.
    ///
    /// Called after a complete review cycle (no remaining reqs) to prevent
    /// unbounded growth of checkpoint rows in the monitor_events table.
    async fn purge_old_checkpoints(&self) -> Result<(), WalletError> {
        let events = self
            .storage
            .find_monitor_events(&FindMonitorEventsArgs {
                partial: MonitorEventPartial {
                    id: None,
                    event: Some("ReviewDoubleSpends".to_string()),
                },
                since: None,
                paged: None,
            })
            .await?;

        if events.len() <= 1 {
            return Ok(());
        }

        // The most recent checkpoint has the highest id.
        let max_id = events.iter().map(|e| e.id).max().unwrap_or(0);
        self.storage
            .delete_monitor_events_before_id("ReviewDoubleSpends", max_id)
            .await?;
        Ok(())
    }

    /// Save a checkpoint to monitor_events.
    async fn save_checkpoint(&self, cp: &ReviewDoubleSpendCheckpoint) -> Result<(), WalletError> {
        let now = chrono::Utc::now().naive_utc();
        let details = serde_json::to_string(cp).ok();
        let event = MonitorEvent {
            created_at: now,
            updated_at: now,
            id: 0, // auto-assigned by DB
            event: "ReviewDoubleSpends".to_string(),
            details,
        };
        self.storage.insert_monitor_event(&event).await?;
        Ok(())
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewDoubleSpends {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewDoubleSpends"
    }

    fn trigger(&mut self, now_msecs: u64) -> bool {
        let interval = self.effective_trigger();
        interval > 0 && now_msecs > self.last_run_msecs + interval
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        self.last_run_msecs = now_msecs();

        // Age cutoff: only review reqs updated more than min_age_minutes ago.
        let age_cutoff =
            chrono::Utc::now().naive_utc() - chrono::Duration::minutes(self.min_age_minutes as i64);

        // Resume from last checkpoint if available.
        let checkpoint = self.get_last_checkpoint().await?;
        let mut offset: i64 = 0;

        if let Some((resume_offset, expected_id)) = checkpoint {
            // Verify the checkpoint is still valid by checking the expected req at that offset.
            if let Some(expected) = expected_id {
                let verify = self
                    .storage
                    .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                        partial: ProvenTxReqPartial::default(),
                        since: None,
                        paged: Some(Paged {
                            limit: 1,
                            offset: resume_offset,
                        }),
                        statuses: Some(vec![ProvenTxReqStatus::DoubleSpend]),
                    })
                    .await?;

                if verify
                    .first()
                    .map_or(true, |r| r.proven_tx_req_id != expected)
                {
                    // Dataset changed — restart from beginning.
                    offset = 0;
                } else {
                    offset = resume_offset + 1;
                }
            } else {
                offset = resume_offset;
            }
        }

        let all_reqs = self
            .storage
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial::default(),
                since: None,
                paged: Some(Paged {
                    limit: self.review_limit,
                    offset,
                }),
                statuses: Some(vec![ProvenTxReqStatus::DoubleSpend]),
            })
            .await?;

        // Filter to only reqs old enough (updated_at <= age_cutoff), matching TS behavior.
        let reqs: Vec<_> = all_reqs
            .into_iter()
            .filter(|r| r.updated_at <= age_cutoff)
            .collect();

        if reqs.is_empty() {
            self.use_quick_trigger = false;
            // Clean up stale checkpoint events, keeping only the most recent.
            self.purge_old_checkpoints().await?;
            return Ok(String::new());
        }

        // Switch to quick trigger if we hit the limit (more work to do).
        self.use_quick_trigger = reqs.len() as i64 >= self.review_limit;

        let mut unfailed = 0u64;
        let mut still_unknown = 0u64;
        let mut last_double_spend_id: Option<i64> = None;
        let mut retained_count: i64 = 0;

        for req in &reqs {
            let result = self
                .services
                .get_status_for_txids(&[req.txid.clone()], false)
                .await;

            // If the service found the txid on-chain, mark for unfail.
            let is_unknown = match result.results.first() {
                Some(r) => r.status == "unknown",
                None => true,
            };

            if is_unknown {
                still_unknown += 1;
                last_double_spend_id = Some(req.proven_tx_req_id);
                retained_count += 1;
            } else {
                let update = ProvenTxReqPartial {
                    status: Some(ProvenTxReqStatus::Unfail),
                    ..Default::default()
                };
                self.storage
                    .update_proven_tx_req(req.proven_tx_req_id, &update)
                    .await?;
                unfailed += 1;
            }
        }

        // Persist checkpoint for resume on restart.
        let cp = ReviewDoubleSpendCheckpoint {
            resume_offset: if last_double_spend_id.is_some() {
                offset + retained_count - 1
            } else {
                0
            },
            expected_proven_tx_req_id: last_double_spend_id,
            review_limit: self.review_limit,
            min_age_minutes: self.min_age_minutes,
            reviewed: reqs.len() as u64,
            unfails: unfailed,
        };
        self.save_checkpoint(&cp).await?;

        Ok(format!(
            "reviewed {} double-spend reqs (offset {}): {} unfailed, {} still unknown",
            reqs.len(),
            offset,
            unfailed,
            still_unknown
        ))
    }
}

#[cfg(test)]
mod tests {
    //! Behavioral tests for `TaskReviewDoubleSpends`.
    //!
    //! Each test builds a fresh in-memory SQLite storage, attaches a scripted
    //! `MockServices` with a configurable `get_status_for_txids` response, and
    //! exercises the real `run_task` / helper methods.

    use super::*;
    use crate::error::WalletResult;
    use crate::services::traits::WalletServices;
    use crate::services::types::{
        BlockHeader, BsvExchangeRate, FiatExchangeRates, GetMerklePathResult, GetRawTxResult,
        GetScriptHashHistoryResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat,
        GetUtxoStatusResult, NLockTimeInput, PostBeefResult, ServicesCallHistory,
        StatusForTxidResult,
    };
    use crate::status::ProvenTxReqStatus;
    use crate::storage::find_args::{FindMonitorEventsArgs, MonitorEventPartial};
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::wallet_provider::WalletStorageProvider;
    use crate::storage::StorageConfig;
    use crate::tables::ProvenTxReq;
    use crate::types::Chain;
    use async_trait::async_trait;
    use bsv::transaction::Beef;
    use chrono::{Duration, Utc};
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // Mock WalletServices whose `get_status_for_txids` returns a scripted
    // response keyed by txid. Captures call count for verification.
    // -----------------------------------------------------------------------

    struct ScriptedStatusServices {
        /// `(status_code, per-txid-result)` pairs for each incoming txid.
        /// Status "success" → include a `StatusForTxidResult` with given status.
        /// Status "error" → empty results (simulates outage).
        outage: bool,
        /// txid -> status string ("mined", "known", "unknown")
        responses: std::collections::HashMap<String, String>,
        /// Collected txids that were looked up.
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptedStatusServices {
        fn success(
            responses: std::collections::HashMap<String, String>,
        ) -> Arc<dyn WalletServices> {
            Arc::new(Self {
                outage: false,
                responses,
                calls: Mutex::new(Vec::new()),
            })
        }

        fn outage() -> Arc<dyn WalletServices> {
            Arc::new(Self {
                outage: true,
                responses: Default::default(),
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl WalletServices for ScriptedStatusServices {
        fn chain(&self) -> Chain {
            Chain::Test
        }
        async fn get_chain_tracker(
            &self,
        ) -> WalletResult<Box<dyn bsv::transaction::chain_tracker::ChainTracker>> {
            Err(WalletError::NotImplemented("mock".into()))
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
            txids: &[String],
            _use_next: bool,
        ) -> GetStatusForTxidsResult {
            self.calls.lock().unwrap().push(txids.to_vec());
            if self.outage {
                return GetStatusForTxidsResult {
                    name: "mock".into(),
                    status: "error".into(),
                    error: Some("service outage".into()),
                    results: vec![],
                };
            }
            let results = txids
                .iter()
                .map(|t| {
                    let status = self
                        .responses
                        .get(t)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    StatusForTxidResult {
                        txid: t.clone(),
                        depth: None,
                        status,
                    }
                })
                .collect();
            GetStatusForTxidsResult {
                name: "mock".into(),
                status: "success".into(),
                error: None,
                results,
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
            Ok(0)
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
    // Storage helpers
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

    /// Insert a `ProvenTxReq` with given txid + updated_at.
    async fn insert_req(
        storage: &SqliteStorage,
        txid: &str,
        updated_at: chrono::NaiveDateTime,
        status: ProvenTxReqStatus,
    ) -> i64 {
        use crate::storage::traits::reader_writer::StorageReaderWriter;
        let req = ProvenTxReq {
            created_at: updated_at,
            updated_at,
            proven_tx_req_id: 0,
            proven_tx_id: None,
            status,
            attempts: 0,
            notified: false,
            txid: txid.to_string(),
            batch: None,
            history: "{}".to_string(),
            notify: "{}".to_string(),
            raw_tx: vec![],
            input_beef: None,
        };
        storage.insert_proven_tx_req(&req, None).await.unwrap()
    }

    // =======================================================================
    // Tests
    // =======================================================================

    /// `purge_old_checkpoints` should delete all checkpoint events whose id is
    /// less than the most-recent checkpoint id. Guards the stale-checkpoint
    /// cleanup invariant introduced in commit 9044177.
    #[tokio::test]
    async fn test_purge_old_checkpoints_keeps_only_most_recent() {
        let (mgr, _storage) = make_manager_and_storage().await;
        // Insert three checkpoint events manually.
        let now = Utc::now().naive_utc();
        for i in 0..3 {
            let ev = MonitorEvent {
                created_at: now,
                updated_at: now,
                id: 0,
                event: "ReviewDoubleSpends".to_string(),
                details: Some(format!("{{\"resumeOffset\":{i}}}")),
            };
            mgr.insert_monitor_event(&ev).await.unwrap();
        }
        // Insert an unrelated event — must NOT be deleted.
        let other = MonitorEvent {
            created_at: now,
            updated_at: now,
            id: 0,
            event: "OtherEvent".to_string(),
            details: None,
        };
        mgr.insert_monitor_event(&other).await.unwrap();

        let task = TaskReviewDoubleSpends::new(
            mgr.clone(),
            ScriptedStatusServices::success(Default::default()),
        );
        task.purge_old_checkpoints().await.unwrap();

        let remaining = mgr
            .find_monitor_events(&FindMonitorEventsArgs {
                partial: MonitorEventPartial {
                    id: None,
                    event: Some("ReviewDoubleSpends".to_string()),
                },
                since: None,
                paged: None,
            })
            .await
            .unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "only the most recent ReviewDoubleSpends event should remain"
        );

        let other_remaining = mgr
            .find_monitor_events(&FindMonitorEventsArgs {
                partial: MonitorEventPartial {
                    id: None,
                    event: Some("OtherEvent".to_string()),
                },
                since: None,
                paged: None,
            })
            .await
            .unwrap();
        assert_eq!(
            other_remaining.len(),
            1,
            "unrelated events must be preserved"
        );
    }

    /// Age filter: a req whose `updated_at` is NEWER than `min_age_minutes` is
    /// NOT reviewed (commit 77c55cb). An older req IS reviewed.
    #[tokio::test]
    async fn test_age_filter_only_processes_old_enough_reqs() {
        let (mgr, storage) = make_manager_and_storage().await;
        let now = Utc::now().naive_utc();
        // too-new: updated 10 minutes ago (< 60-min default) -- must NOT be touched
        let young_id = insert_req(
            &storage,
            "young_txid",
            now - Duration::minutes(10),
            ProvenTxReqStatus::DoubleSpend,
        )
        .await;
        // old enough: updated 120 minutes ago -- IS eligible
        let old_id = insert_req(
            &storage,
            "old_txid",
            now - Duration::minutes(120),
            ProvenTxReqStatus::DoubleSpend,
        )
        .await;

        // Scripted services: say both txids are "mined" so anything touched → Unfail.
        let mut resp = std::collections::HashMap::new();
        resp.insert("young_txid".to_string(), "mined".to_string());
        resp.insert("old_txid".to_string(), "mined".to_string());

        let svc = ScriptedStatusServices::success(resp);
        let mut task = TaskReviewDoubleSpends::new(mgr.clone(), svc);
        task.run_task().await.unwrap();

        // Re-read both reqs.
        let young = mgr
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    proven_tx_req_id: Some(young_id),
                    ..Default::default()
                },
                since: None,
                paged: None,
                statuses: None,
            })
            .await
            .unwrap();
        let old = mgr
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    proven_tx_req_id: Some(old_id),
                    ..Default::default()
                },
                since: None,
                paged: None,
                statuses: None,
            })
            .await
            .unwrap();

        assert_eq!(
            young[0].status,
            ProvenTxReqStatus::DoubleSpend,
            "young (< min_age) req must NOT be touched"
        );
        assert_eq!(
            old[0].status,
            ProvenTxReqStatus::Unfail,
            "old req with on-chain status must be flipped to Unfail"
        );
    }

    /// Service outage (status="error", empty results): the task must NOT
    /// terminalize (flip) any req. PR review IMPORTANT #6.
    ///
    /// If this test fails, the production code is treating an outage as
    /// equivalent to "unknown" for every txid and may therefore spuriously
    /// advance reqs through the pipeline. See PR review note.
    #[tokio::test]
    async fn test_service_outage_does_not_terminalize() {
        let (mgr, storage) = make_manager_and_storage().await;
        let old = Utc::now().naive_utc() - Duration::minutes(120);
        let id = insert_req(&storage, "txid_outage", old, ProvenTxReqStatus::DoubleSpend).await;

        let mut task = TaskReviewDoubleSpends::new(mgr.clone(), ScriptedStatusServices::outage());
        task.run_task().await.unwrap();

        let reqs = mgr
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    proven_tx_req_id: Some(id),
                    ..Default::default()
                },
                since: None,
                paged: None,
                statuses: None,
            })
            .await
            .unwrap();
        assert_eq!(
            reqs[0].status,
            ProvenTxReqStatus::DoubleSpend,
            "outage must not cause a status transition — current code is unsafe if this fails"
        );
    }

    /// Checkpoint persistence: after a run that found eligible reqs, a
    /// ReviewDoubleSpends monitor_event row is written with the correct
    /// `reviewed` count serialized in JSON details.
    #[tokio::test]
    async fn test_checkpoint_persisted_after_run() {
        let (mgr, storage) = make_manager_and_storage().await;
        let old = Utc::now().naive_utc() - Duration::minutes(120);
        insert_req(&storage, "a_tx", old, ProvenTxReqStatus::DoubleSpend).await;
        insert_req(&storage, "b_tx", old, ProvenTxReqStatus::DoubleSpend).await;

        // Both txids return "unknown" → no unfails, both retained.
        let mut task = TaskReviewDoubleSpends::new(
            mgr.clone(),
            ScriptedStatusServices::success(Default::default()),
        );
        task.run_task().await.unwrap();

        let events = mgr
            .find_monitor_events(&FindMonitorEventsArgs {
                partial: MonitorEventPartial {
                    id: None,
                    event: Some("ReviewDoubleSpends".to_string()),
                },
                since: None,
                paged: None,
            })
            .await
            .unwrap();
        assert_eq!(
            events.len(),
            1,
            "exactly one checkpoint event should be persisted"
        );
        let details = events[0].details.as_ref().expect("checkpoint has details");
        let cp: ReviewDoubleSpendCheckpoint = serde_json::from_str(details).unwrap();
        assert_eq!(cp.reviewed, 2, "checkpoint reviewed count must equal reqs");
        assert_eq!(cp.unfails, 0, "no unfails when all txids return unknown");
    }

    /// Checkpoint serialization roundtrip. Kept from the original suite —
    /// the only substantive test of the old version.
    #[test]
    fn test_checkpoint_serialization() {
        let cp = ReviewDoubleSpendCheckpoint {
            resume_offset: 42,
            expected_proven_tx_req_id: Some(123),
            review_limit: 100,
            min_age_minutes: 60,
            reviewed: 10,
            unfails: 3,
        };
        let json = serde_json::to_string(&cp).unwrap();
        let parsed: ReviewDoubleSpendCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.resume_offset, 42);
        assert_eq!(parsed.expected_proven_tx_req_id, Some(123));
        assert_eq!(parsed.reviewed, 10);
        assert_eq!(parsed.unfails, 3);
    }
}
