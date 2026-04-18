//! Provider-based transaction building and signing.
//!
//! Async counterparts to [`build_signable_transaction`] and
//! [`complete_signed_transaction`] that accept a [`SigningProvider`]
//! instead of a [`CachedKeyDeriver`]. This enables the same transaction
//! construction pipeline to work with any signing backend.
//!
//! [`build_signable_transaction`]: crate::signer::build_signable::build_signable_transaction
//! [`complete_signed_transaction`]: crate::signer::complete_signed::complete_signed_transaction
//! [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver

use std::io::Cursor;

use bsv::primitives::public_key::PublicKey;
use bsv::script::locking_script::LockingScript;
use bsv::script::unlocking_script::UnlockingScript;
use bsv::transaction::transaction::Transaction;
use bsv::transaction::transaction_input::TransactionInput;
use bsv::transaction::transaction_output::TransactionOutput;

use crate::error::{WalletError, WalletResult};
use crate::signer::signing_provider::SigningProvider;
use crate::signer::types::{PendingStorageInput, ValidCreateActionArgs};
use crate::storage::action_types::{StorageCreateActionResult, StorageCreateTransactionSdkInput};
use crate::types::StorageProvidedBy;

/// Build an unsigned transaction using a [`SigningProvider`] for change output derivation.
///
/// This is the async counterpart to [`build_signable_transaction`]. The only
/// behavioral difference is that change output locking scripts are derived via
/// [`SigningProvider::derive_change_locking_script`] instead of calling
/// `ScriptTemplateBRC29::lock` directly, allowing non-local key derivation.
///
/// All other logic — input/output ordering, fee calculation, pending input
/// collection — follows the same algorithms as the sync version.
///
/// [`build_signable_transaction`]: crate::signer::build_signable::build_signable_transaction
pub async fn build_signable_transaction_with_provider(
    dcr: &StorageCreateActionResult,
    args: &ValidCreateActionArgs,
    provider: &dyn SigningProvider,
) -> WalletResult<(Transaction, u64, Vec<PendingStorageInput>)> {
    let storage_inputs = &dcr.inputs;
    let storage_outputs = &dcr.outputs;
    let mut tx = Transaction::new();
    tx.version = dcr.version;
    tx.lock_time = dcr.lock_time;

    // Build vout-to-index mapping
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

    // Add outputs in vout order
    for vout in 0..storage_outputs.len() {
        let i = vout_to_index[vout];
        let out = &storage_outputs[i];

        let is_change = out.provided_by == StorageProvidedBy::Storage
            && out.purpose.as_deref() == Some("change");

        let locking_script = if is_change {
            let derivation_suffix = out.derivation_suffix.as_ref().ok_or_else(|| {
                WalletError::Internal("change output missing derivation_suffix".to_string())
            })?;

            let script_bytes = provider
                .derive_change_locking_script(
                    &dcr.derivation_prefix,
                    derivation_suffix,
                )
                .await?;
            LockingScript::from_binary(&script_bytes)
        } else {
            let script_bytes = hex_to_bytes(&out.locking_script)?;
            LockingScript::from_binary(&script_bytes)
        };

        let output = TransactionOutput {
            satoshis: Some(out.satoshis),
            locking_script,
            change: is_change,
        };
        tx.add_output(output);
    }

    // Add dummy OP_RETURN if no outputs
    if storage_outputs.is_empty() {
        let output = TransactionOutput {
            satoshis: Some(0),
            locking_script: LockingScript::from_binary(&[0x00, 0x6a, 0x01, 0x2a]),
            change: false,
        };
        tx.add_output(output);
    }

    // Merge and sort inputs
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

    for (args_input, storage_input) in &merged_inputs {
        if let Some(ai) = args_input {
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
            if storage_input.output_type != "P2PKH" {
                return Err(WalletError::InvalidParameter {
                    parameter: "type".to_string(),
                    must_be: format!(
                        "vin {}, \"{}\" is not a supported unlocking script type.",
                        storage_input.vin, storage_input.output_type
                    ),
                });
            }

            let vin = tx.inputs.len() as u32;

            pending_storage_inputs.push(PendingStorageInput {
                vin,
                derivation_prefix: storage_input.derivation_prefix.clone().unwrap_or_else(|| {
                    tracing::warn!(vin = vin, "missing derivation_prefix, defaulting to empty");
                    String::new()
                }),
                derivation_suffix: storage_input.derivation_suffix.clone().unwrap_or_else(|| {
                    tracing::warn!(vin = vin, "missing derivation_suffix, defaulting to empty");
                    String::new()
                }),
                unlocker_pub_key: storage_input.sender_identity_key.clone(),
                source_satoshis: storage_input.source_satoshis,
                locking_script: storage_input.source_locking_script.clone(),
            });
            let source_tx = storage_input.source_transaction.as_ref().and_then(|raw| {
                let mut cursor = Cursor::new(raw);
                match Transaction::from_binary(&mut cursor) {
                    Ok(tx) => Some(Box::new(tx)),
                    Err(e) => {
                        tracing::warn!(
                            vin = vin,
                            error = %e,
                            "source transaction deserialization failed"
                        );
                        None
                    }
                }
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

    let total_change_outputs: u64 = storage_outputs
        .iter()
        .filter(|o| o.purpose.as_deref() == Some("change"))
        .map(|o| o.satoshis)
        .sum();
    let amount = total_change_inputs.saturating_sub(total_change_outputs);

    Ok((tx, amount, pending_storage_inputs))
}

/// Complete a transaction by signing inputs via a [`SigningProvider`].
///
/// This is the async counterpart to [`complete_signed_transaction`]. For each
/// pending BRC-29 input, computes the sighash and delegates to
/// [`SigningProvider::sign_input`], which returns the P2PKH unlocking script.
/// User-provided unlocking scripts (from `spends`) are inserted directly.
///
/// Returns the fully signed transaction as serialized bytes.
///
/// [`complete_signed_transaction`]: crate::signer::complete_signed::complete_signed_transaction
pub async fn complete_signed_transaction_with_provider(
    tx: &mut Transaction,
    pending_inputs: &[PendingStorageInput],
    spends: &std::collections::HashMap<u32, bsv::wallet::interfaces::SignActionSpend>,
    provider: &dyn SigningProvider,
) -> WalletResult<Vec<u8>> {
    let sighash_type = bsv::primitives::transaction_signature::SIGHASH_ALL
        | bsv::primitives::transaction_signature::SIGHASH_FORKID;

    // Step 1: Insert user-provided unlocking scripts
    for (vin_key, spend) in spends {
        let vin = *vin_key as usize;
        if vin >= tx.inputs.len() {
            return Err(WalletError::InvalidParameter {
                parameter: "spends".to_string(),
                must_be: format!("valid input index. vin {} out of range", vin),
            });
        }
        tx.inputs[vin].unlocking_script =
            Some(UnlockingScript::from_binary(&spend.unlocking_script));
        if let Some(seq) = spend.sequence_number {
            tx.inputs[vin].sequence = seq;
        }
    }

    // Step 2: Sign BRC-29 inputs via provider
    for pdi in pending_inputs {
        let vin = pdi.vin as usize;
        if vin >= tx.inputs.len() {
            return Err(WalletError::InvalidParameter {
                parameter: "pendingInputs".to_string(),
                must_be: format!("valid input index. vin {} out of range", vin),
            });
        }

        let source_locking_script = LockingScript::from_binary(&hex_to_bytes(&pdi.locking_script)?);

        // Build source transaction stub for sighash computation
        let mut source_tx = Transaction::new();
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

        // Compute sighash
        let preimage = tx
            .sighash_preimage(
                vin,
                sighash_type,
                pdi.source_satoshis,
                &source_locking_script,
            )
            .map_err(|e| WalletError::Internal(format!("sighash preimage: {e}")))?;

        let hash = bsv::primitives::hash::sha256d(&preimage);
        let mut sighash = [0u8; 32];
        sighash.copy_from_slice(&hash);

        // Resolve unlocker public key
        let identity_pub = provider.identity_public_key();
        let unlocker_pub_key = if let Some(ref pub_key_hex) = pdi.unlocker_pub_key {
            PublicKey::from_string(pub_key_hex)
                .map_err(|e| WalletError::Internal(format!("Invalid unlocker pub key: {e}")))?
        } else {
            identity_pub.clone()
        };

        // Sign via provider (async — supports any signing backend)
        let unlock_script_bytes = provider
            .sign_input(
                &sighash,
                sighash_type,
                &pdi.derivation_prefix,
                &pdi.derivation_suffix,
                &unlocker_pub_key,
            )
            .await?;

        tx.inputs[vin].unlocking_script = Some(UnlockingScript::from_binary(&unlock_script_bytes));
    }

    // Step 3: Serialize the fully signed transaction
    let mut buf = Vec::new();
    tx.to_binary(&mut buf)
        .map_err(|e| WalletError::Internal(format!("Serialize signed tx: {e}")))?;

    Ok(buf)
}

/// Simple hex decoding (no external dependency).
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, WalletError> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            if i + 2 <= hex.len() {
                u8::from_str_radix(&hex[i..i + 2], 16)
                    .map_err(|_| WalletError::Internal(format!("invalid hex at position {i}")))
            } else {
                Err(WalletError::Internal("odd-length hex string".into()))
            }
        })
        .collect()
}
