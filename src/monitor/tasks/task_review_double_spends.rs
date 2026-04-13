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
    use crate::monitor::ONE_MINUTE;

    #[test]
    fn test_review_double_spends_defaults() {
        assert_eq!(12 * ONE_MINUTE, 720_000);
        assert_eq!(ONE_MINUTE, 60_000);
    }

    #[test]
    fn test_name() {
        assert_eq!("ReviewDoubleSpends", "ReviewDoubleSpends");
    }

    #[test]
    fn test_checkpoint_serialization() {
        let cp = super::ReviewDoubleSpendCheckpoint {
            resume_offset: 42,
            expected_proven_tx_req_id: Some(123),
            review_limit: 100,
            min_age_minutes: 60,
            reviewed: 10,
            unfails: 3,
        };
        let json = serde_json::to_string(&cp).unwrap();
        let parsed: super::ReviewDoubleSpendCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.resume_offset, 42);
        assert_eq!(parsed.expected_proven_tx_req_id, Some(123));
    }
}
