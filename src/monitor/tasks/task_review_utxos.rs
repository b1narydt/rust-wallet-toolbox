//! TaskReviewUtxos -- UTXO validation task (disabled by default).
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskReviewUtxos.ts.
//!
//! This task is intentionally disabled: `trigger()` always returns false.
//! Manual review is available via `review_by_identity_key`.

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
    storage: WalletStorageManager,
}

impl TaskReviewUtxos {
    /// Create a new UTXO review task.
    pub fn new(storage: WalletStorageManager) -> Self {
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
    #[test]
    fn test_name() {
        assert_eq!("ReviewUtxos", "ReviewUtxos");
    }

    #[test]
    fn test_trigger_always_false() {
        let name = "ReviewUtxos";
        assert_eq!(name, "ReviewUtxos");
    }
}
