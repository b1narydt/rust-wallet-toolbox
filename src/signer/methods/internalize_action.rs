//! Signer-level internalizeAction.
//!
//! Ported from wallet-toolbox/src/signer/methods/internalizeAction.ts.
//! Delegates to storage.internalize_action with BEEF validation.
//!
//! **Security**: For wallet payment outputs, validates that each output's
//! locking script matches the expected BRC-29 derivation before accepting.
//! This prevents a malicious sender from sending outputs with bogus
//! derivation parameters that the wallet could never spend.

use std::io::Cursor;

use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::transaction::beef::Beef;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::InternalizeOutput;
use bsv::wallet::types::{Counterparty, CounterpartyType};

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::types::{SignerInternalizeActionResult, ValidInternalizeActionArgs};
use crate::storage::action_types::StorageInternalizeActionArgs;
use crate::storage::manager::WalletStorageManager;
use crate::utility::script_template_brc29::brc29_protocol;
use crate::wallet::types::AuthId;

/// Execute the signer-level internalizeAction flow.
///
/// Converts signer-level args to storage-level args and delegates to
/// storage.internalize_action, which handles BEEF parsing, merkle proof
/// validation, and output tracking.
///
/// Before delegating to storage, this function:
/// 1. Parses the AtomicBEEF to extract the subject transaction.
/// 2. Validates that each wallet payment output's locking script matches
///    the expected BRC-29 derivation (using the sender's identity key and
///    the derivation prefix/suffix from the payment remittance).
///
/// The SDK `InternalizeOutput` enum is passed through directly so the
/// storage layer (and wire format) uses the tagged-enum shape that matches
/// TypeScript.
pub async fn signer_internalize_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    key_deriver: &CachedKeyDeriver,
    auth: &str,
    args: &ValidInternalizeActionArgs,
) -> WalletResult<SignerInternalizeActionResult> {
    // -----------------------------------------------------------------------
    // 1. Parse AtomicBEEF to get the subject transaction
    // -----------------------------------------------------------------------
    let beef = Beef::from_binary(&mut Cursor::new(&args.tx)).map_err(|e| {
        WalletError::Internal(format!("Failed to parse AtomicBEEF: {}", e))
    })?;

    let tx = beef.into_transaction().map_err(|e| {
        WalletError::Internal(format!(
            "Failed to extract subject transaction from BEEF: {}",
            e
        ))
    })?;

    // -----------------------------------------------------------------------
    // 2. Validate wallet payment outputs against BRC-29 derivation
    // -----------------------------------------------------------------------
    let protocol = brc29_protocol();

    for output in &args.outputs {
        if let InternalizeOutput::WalletPayment {
            output_index,
            payment,
        } = output
        {
            let oi = *output_index as usize;

            // Validate output index is in range
            if oi >= tx.outputs.len() {
                return Err(WalletError::InvalidOperation(format!(
                    "Wallet payment output index {} is out of range (transaction has {} outputs).",
                    output_index,
                    tx.outputs.len()
                )));
            }

            // Get the actual locking script from the parsed transaction
            let actual_script = tx.outputs[oi].locking_script.to_binary();

            // Build the key ID: "{derivation_prefix} {derivation_suffix}"
            let derivation_prefix =
                String::from_utf8_lossy(&payment.derivation_prefix).to_string();
            let derivation_suffix =
                String::from_utf8_lossy(&payment.derivation_suffix).to_string();
            let key_id = format!("{} {}", derivation_prefix, derivation_suffix);

            // The sender's identity key is the counterparty for derivation
            let counterparty = Counterparty {
                counterparty_type: CounterpartyType::Other,
                public_key: Some(payment.sender_identity_key.clone()),
            };

            // Derive the expected public key using BRC-29 protocol.
            // for_self=true because we are the receiver deriving our own key.
            let derived_pub = key_deriver
                .derive_public_key(&protocol, &key_id, &counterparty, true)
                .map_err(|e| {
                    WalletError::Internal(format!(
                        "BRC-29 key derivation failed for output {}: {}",
                        output_index, e
                    ))
                })?;

            // Build expected P2PKH locking script from derived public key
            let hash_vec = derived_pub.to_hash();
            let mut hash = [0u8; 20];
            hash.copy_from_slice(&hash_vec);
            let p2pkh = P2PKH::from_public_key_hash(hash);
            let expected_script = p2pkh.lock().map_err(|e| {
                WalletError::Internal(format!(
                    "Failed to build expected P2PKH script for output {}: {}",
                    output_index, e
                ))
            })?;
            let expected_bytes = expected_script.to_binary();

            // Compare expected vs actual
            if actual_script != expected_bytes {
                return Err(WalletError::InvalidOperation(format!(
                    "Wallet payment output {} has locking script that doesn't match BRC-29 derivation. The wallet cannot spend this output.",
                    output_index
                )));
            }
        }
    }

    // -----------------------------------------------------------------------
    // 3. Delegate to storage layer
    // -----------------------------------------------------------------------
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
        not_delayed_results: result.not_delayed_results,
    })
}
