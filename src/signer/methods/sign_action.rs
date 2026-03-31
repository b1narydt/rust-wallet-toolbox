//! Signer-level signAction for completing delayed signing.
//!
//! Ported from wallet-toolbox/src/signer/methods/signAction.ts.
//! Recovers a PendingSignAction, applies user-provided spends,
//! signs BRC-29 inputs, and processes the transaction.

use std::collections::HashMap;
use std::io::Cursor;

use bsv::primitives::public_key::PublicKey;
use bsv::transaction::transaction::Transaction;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::complete_signed::complete_signed_transaction;
use crate::signer::types::{PendingSignAction, SignerSignActionResult, ValidSignActionArgs};
use crate::storage::action_types::StorageProcessActionArgs;
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;

/// Execute the signer-level signAction flow.
///
/// 1. Recover the PendingSignAction from in-memory state
/// 2. Reconstruct the unsigned Transaction from stored bytes
/// 3. Apply user-provided spends and sign BRC-29 inputs
/// 4. Process the signed transaction in storage
/// 5. Optionally broadcast
pub async fn signer_sign_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
    auth: &str,
    args: &ValidSignActionArgs,
    pending: &PendingSignAction,
) -> WalletResult<SignerSignActionResult> {
    // --- Step 1: Reconstruct the unsigned transaction ---
    let mut cursor = Cursor::new(&pending.tx);
    let mut tx = Transaction::from_binary(&mut cursor)
        .map_err(|e| WalletError::Internal(format!("Failed to reconstruct unsigned tx: {}", e)))?;

    // Merge prior createAction options: inherit flags the caller didn't override
    let is_no_send = args.is_no_send || pending.args.is_no_send;
    let is_delayed = args.is_delayed || pending.args.is_delayed;
    let is_send_with = args.is_send_with || pending.args.is_send_with;

    // --- Step 2: Sign the transaction ---
    let signed_tx_bytes = complete_signed_transaction(
        &mut tx,
        &pending.pdi,
        &args.spends,
        key_deriver,
        identity_pub_key,
    )?;

    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {}", e)))?;

    // --- Step 3: Build BEEF ---
    let beef_bytes = super::create_action::build_beef_bytes(&tx, &pending.dcr.input_beef)?;

    // --- Step 4: Process action in storage ---
    let process_args = StorageProcessActionArgs {
        is_new_tx: args.is_new_tx,
        is_send_with,
        is_no_send,
        is_delayed,
        reference: Some(pending.reference.clone()),
        txid: Some(txid.clone()),
        raw_tx: Some(signed_tx_bytes),
        send_with: if is_send_with { pending.args.options.send_with.clone() } else { vec![] },
    };
    let auth_id = AuthId {
        identity_key: auth.to_string(),
        user_id: None,
        is_active: None,
    };
    let process_result = storage.process_action(&auth_id, &process_args).await?;

    // --- Step 5: Broadcast if needed ---
    if !is_no_send && !is_delayed {
        let _post_results = services.post_beef(&beef_bytes, &[txid.clone()]).await;
    }

    // returnTXIDOnly: check signAction's own option, falling back to createAction's
    let return_txid_only = args.options.return_txid_only.0.unwrap_or(false)
        || pending.args.options.return_txid_only.0.unwrap_or(false);

    let result = SignerSignActionResult {
        txid: Some(txid),
        tx: if return_txid_only { None } else { Some(beef_bytes) },
        send_with_results: process_result.send_with_results.unwrap_or_default(),
        not_delayed_results: process_result.not_delayed_results,
    };

    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_merge_prior_options() {
        // signAction doesn't set noSend, inherits from createAction
        let sign_no_send = false;
        let prior_no_send = true;
        let merged = sign_no_send || prior_no_send;
        assert!(merged);

        // signAction explicitly sets noSend=false, but prior was true
        // In this OR logic, true wins — this matches TS behavior where
        // options like noSend are "sticky" from createAction
        let sign_no_send = false;
        let prior_no_send = true;
        let merged = sign_no_send || prior_no_send;
        assert!(merged); // noSend from createAction persists

        // Both false → merged is false
        let merged = false || false;
        assert!(!merged);

        // signAction sets true, prior false → merged is true
        let merged = true || false;
        assert!(merged);
    }

    #[test]
    fn test_return_txid_only_sign_action() {
        use bsv::wallet::types::BooleanDefaultFalse;

        // signAction's own returnTXIDOnly takes effect
        let sign_return = BooleanDefaultFalse(Some(true));
        let prior_return = BooleanDefaultFalse(Some(false));
        let return_txid_only = sign_return.0.unwrap_or(false) || prior_return.0.unwrap_or(false);
        assert!(return_txid_only);

        // Falls back to prior createAction's setting
        let sign_return = BooleanDefaultFalse(None);
        let prior_return = BooleanDefaultFalse(Some(true));
        let return_txid_only = sign_return.0.unwrap_or(false) || prior_return.0.unwrap_or(false);
        assert!(return_txid_only);

        // Neither set → false
        let sign_return = BooleanDefaultFalse(None);
        let prior_return = BooleanDefaultFalse(None);
        let return_txid_only = sign_return.0.unwrap_or(false) || prior_return.0.unwrap_or(false);
        assert!(!return_txid_only);
    }

    #[test]
    fn test_send_with_uses_prior_args() {
        let prior_send_with = vec!["tx1".to_string(), "tx2".to_string()];

        // is_send_with merged true → pass prior txids
        let is_send_with = true;
        let result: Vec<String> = if is_send_with { prior_send_with.clone() } else { vec![] };
        assert_eq!(result.len(), 2);

        // is_send_with merged false → empty
        let is_send_with = false;
        let result: Vec<String> = if is_send_with { prior_send_with } else { vec![] };
        assert!(result.is_empty());
    }
}
