//! Signer-level abortAction.
//!
//! Cancels an in-progress unsigned transaction and releases locked UTXOs.
//! Checks both in-memory pending sign actions and storage records.

use bsv::wallet::interfaces::AbortActionResult;

use crate::error::{WalletError, WalletResult};
use crate::signer::types::ValidAbortActionArgs;
use crate::status::TransactionStatus;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
};
use crate::storage::manager::WalletStorageManager;

/// Execute the signer-level abortAction flow.
///
/// 1. Find the transaction by reference
/// 2. Verify it is in unsigned or unprocessed status
/// 3. Update transaction status to failed
/// 4. Release all UTXOs still claimed by this transaction
pub async fn signer_abort_action(
    storage: &WalletStorageManager,
    auth: &str,
    args: &ValidAbortActionArgs,
) -> WalletResult<AbortActionResult> {
    // Find the user
    let (user, _) = storage.find_or_insert_user(auth).await?;
    let user_id = user.user_id;

    // Find the transaction by reference
    let find_tx_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            reference: Some(args.reference.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let txs = storage.find_transactions(&find_tx_args).await?;
    let transaction = txs
        .into_iter()
        .next()
        .ok_or_else(|| WalletError::InvalidParameter {
            parameter: "reference".to_string(),
            must_be: format!(
                "a valid transaction reference. '{}' not found",
                args.reference
            ),
        })?;

    // Only allow aborting unsigned or unprocessed transactions
    if transaction.status != TransactionStatus::Unsigned
        && transaction.status != TransactionStatus::Unprocessed
    {
        return Err(WalletError::InvalidOperation(format!(
            "Cannot abort transaction with status {}. Only unsigned or unprocessed transactions can be aborted.",
            transaction.status
        )));
    }

    let transaction_id = transaction.transaction_id;

    // Update transaction status to failed
    let tx_update = TransactionPartial {
        status: Some(TransactionStatus::Failed),
        ..Default::default()
    };
    storage
        .update_transaction(transaction_id, &tx_update)
        .await?;

    // Release locked UTXOs: find all outputs where spentBy = this transaction
    // and set them back to spendable
    let find_spent_args = FindOutputsArgs {
        partial: OutputPartial {
            user_id: Some(user_id),
            spent_by: Some(transaction_id),
            ..Default::default()
        },
        ..Default::default()
    };
    let spent_outputs = storage.find_outputs(&find_spent_args).await?;

    let spent_output_ids: Vec<i64> = spent_outputs
        .iter()
        .filter(|output| output.transaction_id != transaction_id)
        .map(|output| output.output_id)
        .collect();
    storage
        .release_inputs_spent_by_trx(&spent_output_ids, transaction_id, None)
        .await?;

    Ok(AbortActionResult { aborted: true })
}
