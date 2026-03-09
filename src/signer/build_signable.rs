//! Build a signable (unsigned) transaction from storage create action result.
//!
//! Ported from wallet-toolbox/src/signer/methods/buildSignableTransaction.ts.
//! Constructs a Transaction with inputs and outputs in the correct order,
//! preparing PendingStorageInput records for later signing.

use std::io::Cursor;

use bsv::primitives::public_key::PublicKey;
use bsv::script::locking_script::LockingScript;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::script::unlocking_script::UnlockingScript;
use bsv::transaction::transaction::Transaction;
use bsv::transaction::transaction_input::TransactionInput;
use bsv::transaction::transaction_output::TransactionOutput;

use crate::error::{WalletError, WalletResult};
use crate::signer::types::{PendingStorageInput, ValidCreateActionArgs};
use crate::storage::action_types::{
    StorageCreateActionResult, StorageCreateTransactionSdkInput,
};
use crate::types::StorageProvidedBy;
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

/// Build an unsigned transaction from the storage create action result.
///
/// Returns:
/// - The unsigned Transaction with all inputs and outputs
/// - The total change input satoshis minus change output satoshis (the "amount")
/// - A list of PendingStorageInput records for BRC-29 inputs that need signing
///
/// Outputs are added in vout order (respecting storage's vout assignments).
/// Inputs are sorted by vin. User-supplied inputs use their unlocking scripts
/// (or empty if signAction will provide them later). Storage/change inputs
/// get empty scripts and are tracked in PendingStorageInput for later signing.
pub fn build_signable_transaction(
    dcr: &StorageCreateActionResult,
    args: &ValidCreateActionArgs,
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
) -> WalletResult<(Transaction, u64, Vec<PendingStorageInput>)> {
    let storage_inputs = &dcr.inputs;
    let storage_outputs = &dcr.outputs;

    let mut tx = Transaction::new();
    tx.version = dcr.version;
    tx.lock_time = dcr.lock_time;

    // ---------------------------------------------------------------
    // Build a vout-to-index mapping so we add outputs in vout order
    // ---------------------------------------------------------------
    let mut vout_to_index: Vec<usize> = vec![0; storage_outputs.len()];
    for vout in 0..storage_outputs.len() {
        let idx = storage_outputs
            .iter()
            .position(|o| o.vout == vout as u32)
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "output.vout".to_string(),
                must_be: format!("sequential. {} is missing", vout),
            })?;
        vout_to_index[vout] = idx;
    }

    // ---------------------------------------------------------------
    // Add OUTPUTS in vout order
    // ---------------------------------------------------------------
    for vout in 0..storage_outputs.len() {
        let i = vout_to_index[vout];
        let out = &storage_outputs[i];

        let is_change =
            out.provided_by == StorageProvidedBy::Storage && out.purpose.as_deref() == Some("change");

        let locking_script = if is_change {
            // Derive change output locking script using BRC-29
            make_change_lock(out, dcr, key_deriver, identity_pub_key)?
        } else {
            // Use the locking script from storage directly
            let script_bytes = hex_to_bytes(&out.locking_script);
            LockingScript::from_binary(&script_bytes)
        };

        let output = TransactionOutput {
            satoshis: Some(out.satoshis),
            locking_script,
            change: is_change,
        };
        tx.add_output(output);
    }

    // If no outputs, add a dummy OP_RETURN output
    if storage_outputs.is_empty() {
        let output = TransactionOutput {
            satoshis: Some(0),
            locking_script: LockingScript::from_binary(&[0x00, 0x6a, 0x01, 0x2a]), // OP_FALSE OP_RETURN PUSH1 0x2a
            change: false,
        };
        tx.add_output(output);
    }

    // ---------------------------------------------------------------
    // Merge and sort INPUTS by vin order
    // ---------------------------------------------------------------
    let mut merged_inputs: Vec<(
        Option<&crate::signer::types::ValidCreateActionInput>,
        &StorageCreateTransactionSdkInput,
    )> = Vec::new();

    for si in storage_inputs {
        let args_input = if (si.vin as usize) < args.inputs.len() {
            Some(&args.inputs[si.vin as usize])
        } else {
            None
        };
        merged_inputs.push((args_input, si));
    }
    merged_inputs.sort_by_key(|(_, si)| si.vin);

    let mut pending_storage_inputs: Vec<PendingStorageInput> = Vec::new();
    let mut total_change_inputs: u64 = 0;

    // ---------------------------------------------------------------
    // Add INPUTS
    // ---------------------------------------------------------------
    for (args_input, storage_input) in &merged_inputs {
        if let Some(ai) = args_input {
            // Type 1: User-supplied input
            let unlock = if let Some(ref script_bytes) = ai.unlocking_script {
                UnlockingScript::from_binary(script_bytes)
            } else {
                UnlockingScript::from_binary(&[])
            };

            let input = TransactionInput {
                source_transaction: None,
                source_txid: Some(ai.outpoint.txid.clone()),
                source_output_index: ai.outpoint.vout,
                unlocking_script: Some(unlock),
                sequence: ai.sequence_number,
            };
            tx.add_input(input);
        } else {
            // Type 2: SABPPP/BRC-29 protocol input (change input)
            if storage_input.output_type != "P2PKH" {
                return Err(WalletError::InvalidParameter {
                    parameter: "type".to_string(),
                    must_be: format!(
                        "vin {}, \"{}\" is not a supported unlocking script type.",
                        storage_input.vin, storage_input.output_type
                    ),
                });
            }

            pending_storage_inputs.push(PendingStorageInput {
                vin: tx.inputs.len() as u32,
                derivation_prefix: storage_input
                    .derivation_prefix
                    .clone()
                    .unwrap_or_default(),
                derivation_suffix: storage_input
                    .derivation_suffix
                    .clone()
                    .unwrap_or_default(),
                unlocker_pub_key: storage_input.sender_identity_key.clone(),
                source_satoshis: storage_input.source_satoshis,
                locking_script: storage_input.source_locking_script.clone(),
            });

            // Reconstruct source transaction if raw bytes available
            let source_tx = storage_input.source_transaction.as_ref().and_then(|raw| {
                let mut cursor = Cursor::new(raw);
                Transaction::from_binary(&mut cursor).ok().map(Box::new)
            });

            let input = TransactionInput {
                source_transaction: source_tx,
                source_txid: Some(storage_input.source_txid.clone()),
                source_output_index: storage_input.source_vout,
                unlocking_script: Some(UnlockingScript::from_binary(&[])),
                sequence: 0xFFFFFFFF,
            };
            tx.add_input(input);

            total_change_inputs += storage_input.source_satoshis;
        }
    }

    // Calculate the amount: total change inputs - total change outputs
    let total_change_outputs: u64 = storage_outputs
        .iter()
        .filter(|o| o.purpose.as_deref() == Some("change"))
        .map(|o| o.satoshis)
        .sum();
    let amount = total_change_inputs.saturating_sub(total_change_outputs);

    Ok((tx, amount, pending_storage_inputs))
}

/// Derive a change output locking script using BRC-29 key derivation.
fn make_change_lock(
    out: &crate::storage::action_types::StorageCreateTransactionSdkOutput,
    dcr: &StorageCreateActionResult,
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
) -> WalletResult<LockingScript> {
    let derivation_prefix = dcr.derivation_prefix.clone();
    let derivation_suffix = out
        .derivation_suffix
        .as_ref()
        .ok_or_else(|| WalletError::Internal("change output missing derivation_suffix".to_string()))?
        .clone();

    let sabppp = ScriptTemplateBRC29::new(derivation_prefix, derivation_suffix);

    // Lock: locker=root_key from key_deriver, unlocker=identity_pub_key (self-payment)
    let script_bytes = sabppp.lock(key_deriver.root_key(), identity_pub_key)?;
    Ok(LockingScript::from_binary(&script_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::action_types::{
        StorageCreateActionResult, StorageCreateTransactionSdkInput,
        StorageCreateTransactionSdkOutput,
    };
    use bsv::primitives::private_key::PrivateKey;
    use crate::types::StorageProvidedBy;

    fn test_keys() -> (CachedKeyDeriver, PublicKey) {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let pub_key = priv_key.to_public_key();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        (key_deriver, pub_key)
    }

    fn make_test_dcr() -> StorageCreateActionResult {
        StorageCreateActionResult {
            reference: "test_ref".to_string(),
            version: 1,
            lock_time: 0,
            inputs: vec![StorageCreateTransactionSdkInput {
                vin: 0,
                source_txid: "aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444"
                    .to_string(),
                source_vout: 0,
                source_satoshis: 10_000,
                source_locking_script:
                    "76a91400000000000000000000000000000000000000008ac".to_string(),
                source_transaction: None,
                unlocking_script_length: 107,
                provided_by: StorageProvidedBy::Storage,
                output_type: "P2PKH".to_string(),
                spending_description: None,
                derivation_prefix: Some("testprefix".to_string()),
                derivation_suffix: Some("testsuffix".to_string()),
                sender_identity_key: None,
            }],
            outputs: vec![
                StorageCreateTransactionSdkOutput {
                    vout: 0,
                    satoshis: 5_000,
                    locking_script:
                        "76a914bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb88ac".to_string(),
                    provided_by: StorageProvidedBy::You,
                    purpose: None,
                    basket: None,
                    tags: vec![],
                    output_description: Some("payment".to_string()),
                    derivation_suffix: None,
                    custom_instructions: None,
                },
                StorageCreateTransactionSdkOutput {
                    vout: 1,
                    satoshis: 4_800,
                    locking_script: String::new(),
                    provided_by: StorageProvidedBy::Storage,
                    purpose: Some("change".to_string()),
                    basket: Some("default".to_string()),
                    tags: vec![],
                    output_description: None,
                    derivation_suffix: Some("changesuffix".to_string()),
                    custom_instructions: None,
                },
            ],
            derivation_prefix: "txprefix".to_string(),
            input_beef: None,
            no_send_change_output_vouts: None,
        }
    }

    fn make_test_args() -> ValidCreateActionArgs {
        use bsv::wallet::interfaces::CreateActionOptions;

        ValidCreateActionArgs {
            description: "test payment".to_string(),
            inputs: vec![], // No user inputs; all from storage
            outputs: vec![],
            lock_time: 0,
            version: 1,
            labels: vec![],
            options: CreateActionOptions::default(),
            input_beef: None,
            is_new_tx: true,
            is_sign_action: false,
            is_no_send: false,
            is_delayed: false,
            is_send_with: false,
        }
    }

    #[test]
    fn test_build_signable_basic() {
        let (key_deriver, pub_key) = test_keys();
        let dcr = make_test_dcr();
        let args = make_test_args();

        let (tx, amount, pdi) =
            build_signable_transaction(&dcr, &args, &key_deriver, &pub_key).unwrap();

        // Should have 2 outputs (user + change)
        assert_eq!(tx.outputs.len(), 2);
        // First output should be user's (5000 sats)
        assert_eq!(tx.outputs[0].satoshis, Some(5_000));
        // Second output should be change (4800 sats)
        assert_eq!(tx.outputs[1].satoshis, Some(4_800));
        assert!(tx.outputs[1].change);

        // Should have 1 input (storage/change)
        assert_eq!(tx.inputs.len(), 1);

        // Should have 1 pending storage input for BRC-29 signing
        assert_eq!(pdi.len(), 1);
        assert_eq!(pdi[0].derivation_prefix, "testprefix");
        assert_eq!(pdi[0].derivation_suffix, "testsuffix");
        assert_eq!(pdi[0].source_satoshis, 10_000);

        // Amount = change inputs (10000) - change outputs (4800) = 5200
        assert_eq!(amount, 5_200);
    }

    #[test]
    fn test_build_signable_version_and_locktime() {
        let (key_deriver, pub_key) = test_keys();
        let mut dcr = make_test_dcr();
        dcr.version = 2;
        dcr.lock_time = 500000;
        let args = make_test_args();

        let (tx, _, _) = build_signable_transaction(&dcr, &args, &key_deriver, &pub_key).unwrap();

        assert_eq!(tx.version, 2);
        assert_eq!(tx.lock_time, 500000);
    }

    #[test]
    fn test_build_signable_empty_outputs_adds_dummy() {
        let (key_deriver, pub_key) = test_keys();
        let mut dcr = make_test_dcr();
        dcr.inputs.clear();
        dcr.outputs.clear();
        let args = make_test_args();

        let (tx, _, _) = build_signable_transaction(&dcr, &args, &key_deriver, &pub_key).unwrap();

        // Should have 1 dummy output
        assert_eq!(tx.outputs.len(), 1);
        assert_eq!(tx.outputs[0].satoshis, Some(0));
    }

    #[test]
    fn test_build_signable_change_output_has_valid_p2pkh_script() {
        let (key_deriver, pub_key) = test_keys();
        let dcr = make_test_dcr();
        let args = make_test_args();

        let (tx, _, _) = build_signable_transaction(&dcr, &args, &key_deriver, &pub_key).unwrap();

        // Change output (vout=1) should have a valid P2PKH script (25 bytes)
        let change_script = tx.outputs[1].locking_script.to_binary();
        assert_eq!(change_script.len(), 25, "P2PKH locking script should be 25 bytes");
        assert_eq!(change_script[0], 0x76, "should start with OP_DUP");
        assert_eq!(change_script[24], 0xac, "should end with OP_CHECKSIG");
    }
}
