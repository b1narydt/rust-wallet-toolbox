//! Signer-level internalizeAction.
//!
//! Ported from wallet-toolbox/src/signer/methods/internalizeAction.ts.
//! Delegates to storage.internalize_action with BEEF validation.

use crate::error::WalletResult;
use crate::services::traits::WalletServices;
use crate::signer::types::{SignerInternalizeActionResult, ValidInternalizeActionArgs};
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;
use crate::storage::action_types::{StorageInternalizeActionArgs, StorageInternalizeOutput};
use bsv::wallet::interfaces::InternalizeOutput;

/// Execute the signer-level internalizeAction flow.
///
/// Converts signer-level args to storage-level args and delegates to
/// storage.internalize_action, which handles BEEF parsing, merkle proof
/// validation, and output tracking.
pub async fn signer_internalize_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    auth: &str,
    args: &ValidInternalizeActionArgs,
) -> WalletResult<SignerInternalizeActionResult> {
    // Convert SDK InternalizeOutput enum to storage-level args
    let storage_args = StorageInternalizeActionArgs {
        tx: args.tx.clone(),
        description: args.description.clone(),
        labels: args.labels.iter().map(|l| l.to_string()).collect(),
        outputs: args
            .outputs
            .iter()
            .map(|o| match o {
                InternalizeOutput::WalletPayment {
                    output_index,
                    payment,
                } => StorageInternalizeOutput {
                    output_index: *output_index,
                    protocol: "wallet payment".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: vec![],
                    derivation_prefix: Some(
                        String::from_utf8_lossy(&payment.derivation_prefix).to_string(),
                    ),
                    derivation_suffix: Some(
                        String::from_utf8_lossy(&payment.derivation_suffix).to_string(),
                    ),
                    sender_identity_key: Some(payment.sender_identity_key.to_der_hex()),
                },
                InternalizeOutput::BasketInsertion {
                    output_index,
                    insertion,
                } => StorageInternalizeOutput {
                    output_index: *output_index,
                    protocol: "basket insertion".to_string(),
                    basket: Some(insertion.basket.to_string()),
                    custom_instructions: insertion.custom_instructions.clone(),
                    tags: insertion.tags.iter().map(|t| t.to_string()).collect(),
                    derivation_prefix: None,
                    derivation_suffix: None,
                    sender_identity_key: None,
                },
            })
            .collect(),
    };

    let auth_id = AuthId { identity_key: auth.to_string(), user_id: None, is_active: None };
    let result = storage
        .internalize_action(&auth_id, &storage_args, services)
        .await?;

    Ok(SignerInternalizeActionResult {
        accepted: result.accepted,
        is_merge: result.is_merge,
        txid: result.txid,
        satoshis: result.satoshis,
        send_with_results: result.send_with_results,
    })
}
