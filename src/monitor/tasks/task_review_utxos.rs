//! TaskReviewUtxos -- UTXO validation task (disabled by default).
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewUtxos.ts.
//!
//! This task is intentionally disabled: `trigger()` always returns false.
//! Manual review is available via `review_by_identity_key`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::WalletError;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::{AuthId, SPEC_OP_INVALID_CHANGE};

/// UTXO validation task.
///
/// Disabled by default -- `trigger()` always returns `false`.
/// Use `review_by_identity_key` for on-demand UTXO review.
pub struct TaskReviewUtxos {
    storage: Arc<WalletStorageManager>,
}

impl TaskReviewUtxos {
    /// Create a new UTXO review task.
    pub fn new(storage: Arc<WalletStorageManager>) -> Self {
        Self { storage }
    }

    /// Review UTXOs for a specific user by their identity key.
    ///
    /// Queries outputs in the `specOpInvalidChange` basket with release tags
    /// to find invalid or unspendable UTXOs. Returns a formatted log string.
    ///
    /// # Arguments
    ///
    /// * `identity_key` - The user's public identity key (hex DER).
    /// * `mode` - `"all"` reviews all spendable outputs; `"change"` reviews
    ///   only change outputs.
    pub async fn review_by_identity_key(
        &self,
        identity_key: &str,
        mode: &str,
    ) -> Result<String, WalletError> {
        let user = self.storage.find_user_by_identity_key(identity_key).await?;

        let user = match user {
            Some(u) => u,
            None => {
                return Ok(format!("identityKey {identity_key} was not found\n"));
            }
        };

        let tags = if mode == "all" {
            vec!["release".to_string(), "all".to_string()]
        } else {
            vec!["release".to_string()]
        };

        let auth = AuthId {
            identity_key: user.identity_key.clone(),
            user_id: Some(user.user_id),
            is_active: None,
        };

        let args = bsv::wallet::interfaces::ListOutputsArgs {
            basket: SPEC_OP_INVALID_CHANGE.to_string(),
            tags: tags.clone(),
            tag_query_mode: Some(bsv::wallet::interfaces::QueryMode::All),
            include: None,
            include_custom_instructions: bsv::wallet::types::BooleanDefaultFalse(Some(false)),
            include_tags: bsv::wallet::types::BooleanDefaultFalse(Some(false)),
            include_labels: bsv::wallet::types::BooleanDefaultFalse(Some(false)),
            limit: None,
            offset: None,
            seek_permission: bsv::wallet::types::BooleanDefaultTrue(Some(false)),
        };

        let result = self.storage.list_outputs(&auth, &args).await?;

        if result.total_outputs == 0 {
            return Ok(format!(
                "userId {}: no invalid utxos found, {}\n",
                user.user_id, user.identity_key
            ));
        }

        let total_sats: u64 = result.outputs.iter().map(|o| o.satoshis).sum();
        let target = if mode == "all" {
            "spendable utxos"
        } else {
            "spendable change utxos"
        };
        let has_release = tags.iter().any(|t| t == "release");
        let action = if has_release {
            "updated to unspendable"
        } else {
            "found"
        };

        let mut log = format!(
            "userId {}: {} {target} {action}, total {total_sats}, {}\n",
            user.user_id, result.total_outputs, user.identity_key
        );
        for output in &result.outputs {
            let spendable = if output.spendable {
                "spendable"
            } else {
                "spent"
            };
            log += &format!(
                "  {} {} now {spendable}\n",
                output.outpoint, output.satoshis
            );
        }
        Ok(log)
    }
}

#[async_trait]
impl WalletMonitorTask for TaskReviewUtxos {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ReviewUtxos"
    }

    fn trigger(&mut self, _now_msecs: u64) -> bool {
        false
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        Ok("TaskReviewUtxos is disabled; use review_by_identity_key instead.".to_string())
    }
}

#[cfg(test)]
mod tests {
    //! Behavioral tests for `TaskReviewUtxos`.
    //!
    //! Exercises the real `trigger()` contract (always false) and
    //! `review_by_identity_key` on-demand handling for both present and
    //! absent users.

    use super::*;
    use crate::monitor::task_trait::WalletMonitorTask;
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::traits::wallet_provider::WalletStorageProvider;
    use crate::storage::StorageConfig;
    use crate::tables::User;
    use crate::types::Chain;
    use chrono::Utc;

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
            "02identity".to_string(),
            Some(provider),
            vec![],
        ));
        mgr.make_available().await.unwrap();
        (mgr, storage)
    }

    // =======================================================================
    // Tests
    // =======================================================================

    /// `trigger()` must always return false regardless of the clock — this
    /// task is disabled by design (manual-only via `review_by_identity_key`).
    /// Exercises several `now_msecs` values to guard against accidental
    /// re-enable by boundary-condition regressions.
    #[tokio::test]
    async fn test_trigger_always_returns_false() {
        let (mgr, _storage) = make_manager_and_storage().await;
        let mut task = TaskReviewUtxos::new(mgr);

        assert!(!task.trigger(0));
        assert!(!task.trigger(1_000));
        assert!(!task.trigger(1_000_000_000));
        assert!(!task.trigger(u64::MAX));
    }

    /// `name()` returns the stable identifier used by the monitor registry.
    /// (Kept non-tautological: obtain via the real trait method on a
    /// constructed instance, not a bare string literal.)
    #[tokio::test]
    async fn test_name_matches_registry_identifier() {
        let (mgr, _storage) = make_manager_and_storage().await;
        let task = TaskReviewUtxos::new(mgr);
        assert_eq!(task.name(), "ReviewUtxos");
    }

    /// `run_task()` must return a message indicating the task is disabled
    /// (guards against accidental re-enable via polling loop).
    #[tokio::test]
    async fn test_run_task_reports_disabled() {
        let (mgr, _storage) = make_manager_and_storage().await;
        let mut task = TaskReviewUtxos::new(mgr);
        let out = task.run_task().await.unwrap();
        assert!(
            out.contains("disabled")
                && out.contains("review_by_identity_key"),
            "run_task output must mention disabled state and the manual API; got: {out}"
        );
    }

    /// `review_by_identity_key` for an unknown identity key returns the
    /// "was not found" formatted message.
    #[tokio::test]
    async fn test_review_by_identity_key_unknown_user() {
        let (mgr, _storage) = make_manager_and_storage().await;
        let task = TaskReviewUtxos::new(mgr);

        let out = task
            .review_by_identity_key("03unknown_key", "all")
            .await
            .unwrap();
        assert!(
            out.contains("was not found"),
            "unknown-user path must report 'was not found'; got: {out}"
        );
        assert!(
            out.contains("03unknown_key"),
            "output must echo the identity key; got: {out}"
        );
    }

    /// `review_by_identity_key` for a known user with no invalid-change
    /// outputs returns the "no invalid utxos found" formatted message.
    /// Also verifies the `user_id` and `identity_key` are surfaced.
    #[tokio::test]
    async fn test_review_by_identity_key_known_user_no_outputs() {
        let (mgr, storage) = make_manager_and_storage().await;
        let now = Utc::now().naive_utc();
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02userkey".to_string(),
            active_storage: "default".to_string(),
        };
        let user_id = storage.insert_user(&user, None).await.unwrap();
        assert!(user_id > 0);

        let task = TaskReviewUtxos::new(mgr);
        let out = task.review_by_identity_key("02userkey", "all").await.unwrap();
        assert!(
            out.contains("no invalid utxos found"),
            "known-user-no-outputs path must report 'no invalid utxos found'; got: {out}"
        );
        assert!(
            out.contains("02userkey"),
            "output must include the identity key; got: {out}"
        );
        assert!(
            out.contains(&format!("userId {user_id}")),
            "output must include the user id; got: {out}"
        );
    }

    /// `review_by_identity_key` must accept both "all" and "change" modes
    /// without panicking. Mode selection affects tag filtering; this test
    /// guards the dispatch, not the deeper output-selection behavior
    /// (which requires full basket/output fixtures to exercise).
    #[tokio::test]
    async fn test_review_by_identity_key_both_modes() {
        let (mgr, storage) = make_manager_and_storage().await;
        let now = Utc::now().naive_utc();
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02modeuser".to_string(),
            active_storage: "default".to_string(),
        };
        storage.insert_user(&user, None).await.unwrap();

        let task = TaskReviewUtxos::new(mgr);
        let out_all = task
            .review_by_identity_key("02modeuser", "all")
            .await
            .unwrap();
        let out_change = task
            .review_by_identity_key("02modeuser", "change")
            .await
            .unwrap();
        // Both paths return the "no invalid utxos" message when there are
        // no outputs seeded — but they must both reach that point, not error.
        assert!(out_all.contains("no invalid utxos"));
        assert!(out_change.contains("no invalid utxos"));
    }
}
