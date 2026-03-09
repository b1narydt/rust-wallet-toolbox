//! Complete a signed transaction by applying unlocking scripts.
//!
//! Ported from wallet-toolbox/src/signer/methods/completeSignedTransaction.ts.
//! Signs all BRC-29 wallet-controlled inputs and inserts user-provided
//! unlocking scripts for signAction inputs.

use std::collections::HashMap;

use bsv::primitives::public_key::PublicKey;
use bsv::primitives::transaction_signature::{SIGHASH_ALL, SIGHASH_FORKID};
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::script::locking_script::LockingScript;
use bsv::script::templates::ScriptTemplateUnlock;
use bsv::script::unlocking_script::UnlockingScript;
use bsv::transaction::transaction::Transaction;
use bsv::transaction::transaction_output::TransactionOutput;
use bsv::wallet::interfaces::SignActionSpend;

use crate::error::{WalletError, WalletResult};
use crate::signer::types::PendingStorageInput;
use crate::utility::script_template_brc29::ScriptTemplateBRC29;

/// Simple hex decoding.
fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| {
            if i + 2 <= hex.len() {
                u8::from_str_radix(&hex[i..i + 2], 16).ok()
            } else {
                None
            }
        })
        .collect()
}

/// Complete a transaction by signing all inputs.
///
/// Two kinds of inputs are handled:
/// 1. User-provided spends (from signAction): unlocking scripts are inserted
///    from the `spends` HashMap, keyed by input index.
/// 2. BRC-29 wallet-controlled inputs (from PendingStorageInput): signed using
///    ScriptTemplateBRC29.unlock() and Transaction.sign().
///
/// After all inputs are signed, the transaction is serialized to bytes.
///
/// # Arguments
/// * `tx` - The transaction to sign (modified in place)
/// * `pending_inputs` - BRC-29 inputs needing wallet signing
/// * `spends` - User-provided unlocking scripts keyed by input index
/// * `key_deriver` - The cached key deriver (root key accessed via key_deriver.root_key())
/// * `identity_pub_key` - The wallet's identity public key
///
/// # Returns
/// The fully signed transaction serialized to bytes.
pub fn complete_signed_transaction(
    tx: &mut Transaction,
    pending_inputs: &[PendingStorageInput],
    spends: &HashMap<u32, SignActionSpend>,
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
) -> WalletResult<Vec<u8>> {
    let sighash = SIGHASH_ALL | SIGHASH_FORKID;

    // ---------------------------------------------------------------
    // Step 1: Insert user-provided unlocking scripts from spends
    // ---------------------------------------------------------------
    for (vin_key, spend) in spends {
        let vin = *vin_key as usize;
        if vin >= tx.inputs.len() {
            return Err(WalletError::InvalidParameter {
                parameter: "spends".to_string(),
                must_be: format!("valid input index. vin {} out of range", vin),
            });
        }

        tx.inputs[vin].unlocking_script = Some(UnlockingScript::from_binary(&spend.unlocking_script));

        if let Some(seq) = spend.sequence_number {
            tx.inputs[vin].sequence = seq;
        }
    }

    // ---------------------------------------------------------------
    // Step 2: Sign BRC-29 wallet-controlled inputs
    // ---------------------------------------------------------------
    for pdi in pending_inputs {
        let vin = pdi.vin as usize;
        if vin >= tx.inputs.len() {
            return Err(WalletError::InvalidParameter {
                parameter: "pendingInputs".to_string(),
                must_be: format!("valid input index. vin {} out of range", vin),
            });
        }

        let sabppp = ScriptTemplateBRC29::new(
            pdi.derivation_prefix.clone(),
            pdi.derivation_suffix.clone(),
        );

        // For self-payments (change inputs), unlocker_pub_key is the identity key.
        // For received payments, it's the sender's identity key.
        let unlocker_pub_key = if let Some(ref pub_key_hex) = pdi.unlocker_pub_key {
            PublicKey::from_string(pub_key_hex).map_err(|e| {
                WalletError::Internal(format!("Invalid unlocker pub key: {}", e))
            })?
        } else {
            identity_pub_key.clone()
        };

        // unlock() returns a P2PKH template configured with the derived private key
        let p2pkh = sabppp.unlock(key_deriver.root_key(), &unlocker_pub_key)?;

        let source_locking_script = LockingScript::from_binary(&hex_to_bytes(&pdi.locking_script));

        // Build a synthetic source transaction with the correct output at source_output_index.
        // This sets source_transaction on the input for BEEF construction correctness
        // and future compatibility with Transaction::sign_all_inputs().
        let mut source_tx = Transaction::new();
        // Pad outputs up to source_output_index
        for _ in 0..tx.inputs[vin].source_output_index {
            source_tx.add_output(TransactionOutput {
                satoshis: Some(0),
                locking_script: LockingScript::from_binary(&[]),
                change: false,
            });
        }
        source_tx.add_output(TransactionOutput {
            satoshis: Some(pdi.source_satoshis),
            locking_script: source_locking_script.clone(),
            change: false,
        });
        tx.inputs[vin].source_transaction = Some(Box::new(source_tx));

        // Sign this input using the P2PKH template.
        // Explicit satoshis/locking_script params are still needed since tx.sign()
        // does not read from source_transaction (only sign_all_inputs() does).
        tx.sign(
            vin,
            &p2pkh,
            sighash,
            pdi.source_satoshis,
            &source_locking_script,
        )
        .map_err(|e| WalletError::Internal(format!("Failed to sign input {}: {}", vin, e)))?;
    }

    // ---------------------------------------------------------------
    // Step 3: Serialize the signed transaction
    // ---------------------------------------------------------------
    let mut buf = Vec::new();
    tx.to_binary(&mut buf)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize signed tx: {}", e)))?;

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::primitives::private_key::PrivateKey;

    fn test_keys() -> (CachedKeyDeriver, PublicKey) {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let pub_key = priv_key.to_public_key();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        (key_deriver, pub_key)
    }

    #[test]
    fn test_complete_signed_with_no_inputs() {
        let (key_deriver, pub_key) = test_keys();
        let mut tx = Transaction::new();
        use bsv::script::locking_script::LockingScript;
        use bsv::transaction::transaction_output::TransactionOutput;
        tx.add_output(TransactionOutput {
            satoshis: Some(1000),
            locking_script: LockingScript::from_binary(&[0x76, 0xa9, 0x14,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x88, 0xac]),
            change: false,
        });

        let result = complete_signed_transaction(
            &mut tx,
            &[],
            &HashMap::new(),
            &key_deriver,
            &pub_key,
        );

        // Should succeed with no inputs to sign
        assert!(result.is_ok());
        let bytes = result.unwrap();
        // Should be a valid serialized transaction (non-empty)
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_complete_signed_with_brc29_input() {
        let (key_deriver, pub_key) = test_keys();

        // Create a BRC-29 locking script for the source output
        let sabppp = ScriptTemplateBRC29::new("prefix1".to_string(), "suffix1".to_string());
        let lock_script_bytes = sabppp.lock(key_deriver.root_key(), &pub_key).unwrap();
        let lock_script_hex: String = lock_script_bytes.iter().map(|b| format!("{:02x}", b)).collect();

        // Build a transaction with one input that needs BRC-29 signing
        let mut tx = Transaction::new();

        // Add an input
        use bsv::transaction::transaction_input::TransactionInput;
        tx.add_input(TransactionInput {
            source_transaction: None,
            source_txid: Some("aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444".to_string()),
            source_output_index: 0,
            unlocking_script: Some(UnlockingScript::from_binary(&[])),
            sequence: 0xFFFFFFFF,
        });

        // Add an output
        use bsv::transaction::transaction_output::TransactionOutput;
        tx.add_output(TransactionOutput {
            satoshis: Some(5000),
            locking_script: LockingScript::from_binary(&[0x76, 0xa9, 0x14,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x88, 0xac]),
            change: false,
        });

        let pdi = vec![PendingStorageInput {
            vin: 0,
            derivation_prefix: "prefix1".to_string(),
            derivation_suffix: "suffix1".to_string(),
            unlocker_pub_key: None,
            source_satoshis: 10_000,
            locking_script: lock_script_hex,
        }];

        let result = complete_signed_transaction(
            &mut tx,
            &pdi,
            &HashMap::new(),
            &key_deriver,
            &pub_key,
        );

        assert!(result.is_ok(), "signing should succeed: {:?}", result.err());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "signed tx should not be empty");

        // Verify the input now has an unlocking script
        let input = &tx.inputs[0];
        assert!(
            input.unlocking_script.is_some(),
            "input should have unlocking script after signing"
        );
        let unlock_bytes = input.unlocking_script.as_ref().unwrap().to_binary();
        assert!(
            !unlock_bytes.is_empty(),
            "unlocking script should not be empty after signing"
        );
    }

    #[test]
    fn test_complete_signed_invalid_vin_errors() {
        let (key_deriver, pub_key) = test_keys();
        let mut tx = Transaction::new();

        let pdi = vec![PendingStorageInput {
            vin: 5, // Out of range
            derivation_prefix: "prefix".to_string(),
            derivation_suffix: "suffix".to_string(),
            unlocker_pub_key: None,
            source_satoshis: 1000,
            locking_script: "76a914000000000000000000000000000000000000000088ac".to_string(),
        }];

        let result = complete_signed_transaction(
            &mut tx,
            &pdi,
            &HashMap::new(),
            &key_deriver,
            &pub_key,
        );

        assert!(result.is_err());
    }
}
