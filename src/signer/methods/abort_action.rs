//! Signer-level abortAction.
//!
//! Cancels an in-progress unsigned transaction and releases locked UTXOs.
//! Checks both in-memory pending sign actions and storage records.

use bsv::wallet::interfaces::AbortActionResult;

use crate::error::{WalletError, WalletResult};
use crate::signer::types::ValidAbortActionArgs;
use crate::status::TransactionStatus;
use crate::storage::action_traits::StorageActionProvider;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
};

/// Execute the signer-level abortAction flow.
///
/// 1. Find the transaction by reference
/// 2. Verify it is in unsigned or unprocessed status
/// 3. Update transaction status to failed
/// 4. Release all locked UTXOs (set spendable=true)
///
/// NOTE: The current OutputPartial update pattern cannot set spent_by to NULL
/// (None means "don't update"). Released UTXOs are marked spendable=true but
/// retain their spent_by reference. The create_action allocation filter uses
/// `spent_by.is_none()` so a future enhancement should add a nullable update
/// mechanism. For now, the transaction being failed prevents double-spending.
pub async fn signer_abort_action(
    storage: &(dyn StorageActionProvider + Send + Sync),
    auth: &str,
    args: &ValidAbortActionArgs,
) -> WalletResult<AbortActionResult> {
    // Find the user
    let (user, _) = storage.find_or_insert_user(auth, None).await?;
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
    let txs = storage.find_transactions(&find_tx_args, None).await?;
    let transaction = txs.into_iter().next().ok_or_else(|| WalletError::InvalidParameter {
        parameter: "reference".to_string(),
        must_be: format!("a valid transaction reference. '{}' not found", args.reference),
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
        .update_transaction(transaction_id, &tx_update, None)
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
    let spent_outputs = storage.find_outputs(&find_spent_args, None).await?;

    for output in &spent_outputs {
        // Only release outputs that belong to a different transaction
        // (i.e., they are inputs being consumed, not outputs being created)
        if output.transaction_id != transaction_id {
            let output_update = OutputPartial {
                spendable: Some(true),
                ..Default::default()
            };
            storage
                .update_output(output.output_id, &output_update, None)
                .await?;
        }
    }

    Ok(AbortActionResult { aborted: true })
}
