//! Signer-level internalizeAction.
//!
//! Ported from wallet-toolbox/src/signer/methods/internalizeAction.ts.
//! Delegates to storage.internalize_action with BEEF validation.

use crate::error::WalletResult;
use crate::services::traits::WalletServices;
use crate::signer::types::{SignerInternalizeActionResult, ValidInternalizeActionArgs};
use crate::storage::action_types::StorageInternalizeActionArgs;
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;

/// Execute the signer-level internalizeAction flow.
///
/// Converts signer-level args to storage-level args and delegates to
/// storage.internalize_action, which handles BEEF parsing, merkle proof
/// validation, and output tracking.
///
/// The SDK `InternalizeOutput` enum is passed through directly so the
/// storage layer (and wire format) uses the tagged-enum shape that matches
/// TypeScript.
pub async fn signer_internalize_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    auth: &str,
    args: &ValidInternalizeActionArgs,
) -> WalletResult<SignerInternalizeActionResult> {
    // Pass SDK InternalizeOutput enum directly — no flat conversion needed.
    let storage_args = StorageInternalizeActionArgs {
        tx: args.tx.clone(),
        description: args.description.clone(),
        labels: args.labels.iter().map(|l| l.to_string()).collect(),
        seek_permission: true,
        outputs: args.outputs.clone(),
    };

    let auth_id = AuthId {
        identity_key: auth.to_string(),
        user_id: None,
        is_active: None,
    };
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
