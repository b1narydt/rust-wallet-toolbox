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
use crate::storage::action_traits::StorageActionProvider;
use crate::storage::action_types::StorageProcessActionArgs;

/// Execute the signer-level signAction flow.
///
/// 1. Recover the PendingSignAction from in-memory state
/// 2. Reconstruct the unsigned Transaction from stored bytes
/// 3. Apply user-provided spends and sign BRC-29 inputs
/// 4. Process the signed transaction in storage
/// 5. Optionally broadcast
pub async fn signer_sign_action(
    storage: &(dyn StorageActionProvider + Send + Sync),
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
        is_send_with: args.is_send_with,
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        reference: Some(pending.reference.clone()),
        txid: Some(txid.clone()),
        raw_tx: Some(signed_tx_bytes),
        send_with: vec![],
    };
    let process_result = storage.process_action(auth, &process_args, None).await?;

    // --- Step 5: Broadcast if needed ---
    if !args.is_no_send && !args.is_delayed {
        let _post_results = services.post_beef(&beef_bytes, &[txid.clone()]).await;
    }

    let result = SignerSignActionResult {
        txid: Some(txid),
        tx: Some(beef_bytes),
        send_with_results: process_result.send_with_results.unwrap_or_default(),
    };

    Ok(result)
}
