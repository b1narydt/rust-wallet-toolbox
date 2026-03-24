//! Standalone abort_action for WalletStorageProvider blanket impl.
//!
//! Extracted from `WalletStorageManager::abort_action` so it can be called
//! from the blanket impl over `StorageProvider` without holding a manager lock.
//! The logic is identical to the manager version, operating directly on the
//! single provider rather than routing through active/backup.

use bsv::wallet::interfaces::{AbortActionArgs, AbortActionResult};

use crate::error::{WalletError, WalletResult};
use crate::status::TransactionStatus;
use crate::storage::find_args::{FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

/// Abort (cancel) a transaction by reference, releasing locked UTXOs.
///
/// Finds the transaction by reference, validates it is in an abortable status,
/// sets status to Failed, and releases inputs (sets spendable=true, clears spentBy).
pub async fn abort_action(
    storage: &(dyn StorageProvider + Send + Sync),
    auth: &str,
    args: &AbortActionArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<AbortActionResult> {
    let user = storage
        .find_user_by_identity_key(auth, trx)
        .await?
        .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;

    let reference = String::from_utf8_lossy(&args.reference).to_string();

    let txs = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user.user_id),
                    reference: Some(reference.clone()),
                    ..Default::default()
                },
                no_raw_tx: true,
                ..Default::default()
            },
            trx,
        )
        .await?;

    let tx = if txs.is_empty() {
        return Err(WalletError::InvalidParameter {
            parameter: "reference".to_string(),
            must_be: "an existing action reference".to_string(),
        });
    } else {
        txs.into_iter().next().unwrap()
    };

    // Validate abortability: must be outgoing and not in an un-abortable status
    let un_abortable = [
        TransactionStatus::Completed,
        TransactionStatus::Failed,
        TransactionStatus::Sending,
        TransactionStatus::Unproven,
    ];

    if !tx.is_outgoing || un_abortable.contains(&tx.status) {
        return Err(WalletError::InvalidParameter {
            parameter: "reference".to_string(),
            must_be: "an inprocess, outgoing action that has not been signed and shared to the network".to_string(),
        });
    }

    // Release inputs: find outputs spent by this transaction and make them spendable again
    let inputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    spent_by: Some(tx.transaction_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    for input in &inputs {
        storage
            .update_output(
                input.output_id,
                &OutputPartial {
                    spendable: Some(true),
                    spent_by: Some(0), // 0 = unset
                    ..Default::default()
                },
                trx,
            )
            .await?;
    }

    // Update transaction status to failed
    storage
        .update_transaction(
            tx.transaction_id,
            &TransactionPartial {
                status: Some(TransactionStatus::Failed),
                ..Default::default()
            },
            trx,
        )
        .await?;

    Ok(AbortActionResult { aborted: true })
}
