//! Signer-level createAction orchestration.
//!
//! Ported from wallet-toolbox/src/signer/methods/createAction.ts.
//! Coordinates storage.create_action, build_signable_transaction,
//! complete_signed_transaction, storage.process_action, and BEEF construction.

use std::collections::HashMap;

use bsv::primitives::public_key::PublicKey;
use bsv::transaction::Beef;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::build_signable::build_signable_transaction;
use crate::signer::complete_signed::complete_signed_transaction;
use crate::signer::types::{
    PendingSignAction, SignableTransactionRef, SignerCreateActionResult, ValidCreateActionArgs,
};
use crate::storage::action_traits::StorageActionProvider;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionInput, StorageCreateActionOutput,
    StorageProcessActionArgs,
};

/// Simple bytes-to-hex encoding.
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Convert ValidCreateActionArgs to StorageCreateActionArgs.
///
/// Strips unlocking scripts (storage only needs the length).
fn to_storage_args(args: &ValidCreateActionArgs) -> StorageCreateActionArgs {
    StorageCreateActionArgs {
        description: args.description.clone(),
        inputs: args
            .inputs
            .iter()
            .map(|i| StorageCreateActionInput {
                outpoint_txid: i.outpoint.txid.clone(),
                outpoint_vout: i.outpoint.vout,
                input_description: i.input_description.clone(),
                unlocking_script_length: if i.unlocking_script.is_some() {
                    i.unlocking_script.as_ref().unwrap().len()
                } else {
                    i.unlocking_script_length
                },
                sequence_number: i.sequence_number,
            })
            .collect(),
        outputs: args
            .outputs
            .iter()
            .map(|o| {
                let script_hex = o
                    .locking_script
                    .as_ref()
                    .map(|s| bytes_to_hex(s))
                    .unwrap_or_default();
                StorageCreateActionOutput {
                    locking_script: script_hex,
                    satoshis: o.satoshis,
                    output_description: o.output_description.clone(),
                    basket: o.basket.clone(),
                    custom_instructions: o.custom_instructions.clone(),
                    tags: o.tags.clone(),
                }
            })
            .collect(),
        lock_time: args.lock_time,
        version: args.version,
        labels: args.labels.iter().map(|l| l.to_string()).collect(),
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        is_sign_action: args.is_sign_action,
        input_beef: args.input_beef.clone(),
    }
}

/// Execute the signer-level createAction flow.
///
/// 1. Call storage.create_action to allocate UTXOs and create records
/// 2. Build the unsigned transaction via build_signable_transaction
/// 3a. If is_sign_action: store PendingSignAction, return SignableTransaction
/// 3b. Otherwise: sign via complete_signed_transaction, process, optionally broadcast
pub async fn signer_create_action(
    storage: &(dyn StorageActionProvider + Send + Sync),
    services: &(dyn WalletServices + Send + Sync),
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
    auth: &str,
    args: &ValidCreateActionArgs,
) -> WalletResult<(SignerCreateActionResult, Option<PendingSignAction>)> {
    // --- Step 1: Storage create action ---
    let storage_args = to_storage_args(args);
    let dcr = storage.create_action(auth, &storage_args, None).await?;
    let reference = dcr.reference.clone();

    // --- Step 2: Build unsigned transaction ---
    let (mut tx, amount, pdi) =
        build_signable_transaction(&dcr, args, key_deriver, identity_pub_key)?;

    // --- Step 3a: Delayed signing (is_sign_action) ---
    if args.is_sign_action {
        // Serialize unsigned tx for later recovery
        let mut tx_bytes = Vec::new();
        tx.to_binary(&mut tx_bytes)
            .map_err(|e| WalletError::Internal(format!("Failed to serialize unsigned tx: {}", e)))?;

        let pending = PendingSignAction {
            reference: reference.clone(),
            dcr: dcr.clone(),
            args: args.clone(),
            tx: tx_bytes.clone(),
            amount,
            pdi: pdi.clone(),
        };

        // Build signable transaction BEEF for the caller
        let signable_beef = build_beef_bytes(&tx, &dcr.input_beef)?;

        let no_send_change = if args.is_no_send {
            let txid = tx
                .id()
                .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {}", e)))?;
            dcr.no_send_change_output_vouts
                .as_ref()
                .map(|vouts| vouts.iter().map(|v| format!("{}.{}", txid, v)).collect())
                .unwrap_or_default()
        } else {
            vec![]
        };

        let result = SignerCreateActionResult {
            txid: None,
            tx: None,
            no_send_change,
            send_with_results: vec![],
            signable_transaction: Some(SignableTransactionRef {
                reference: reference.clone(),
                tx: signable_beef,
            }),
        };

        return Ok((result, Some(pending)));
    }

    // --- Step 3b: Immediate signing ---
    let signed_tx_bytes = complete_signed_transaction(
        &mut tx,
        &pdi,
        &HashMap::new(),
        key_deriver,
        identity_pub_key,
    )?;

    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {}", e)))?;

    // Build BEEF with signed transaction
    let beef_bytes = build_beef_bytes(&tx, &dcr.input_beef)?;

    let no_send_change = if args.is_no_send {
        dcr.no_send_change_output_vouts
            .as_ref()
            .map(|vouts| vouts.iter().map(|v| format!("{}.{}", txid, v)).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };

    // --- Step 4: Process action in storage ---
    let process_args = StorageProcessActionArgs {
        is_new_tx: args.is_new_tx,
        is_send_with: args.is_send_with,
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        reference: Some(reference),
        txid: Some(txid.clone()),
        raw_tx: Some(signed_tx_bytes),
        send_with: vec![],
    };
    let process_result = storage.process_action(auth, &process_args, None).await?;

    // --- Step 5: Broadcast if needed ---
    if !args.is_no_send && !args.is_delayed {
        let _post_results = services.post_beef(&beef_bytes, &[txid.clone()]).await;
    }

    let result = SignerCreateActionResult {
        txid: Some(txid),
        tx: Some(beef_bytes),
        no_send_change,
        send_with_results: process_result.send_with_results.unwrap_or_default(),
        signable_transaction: None,
    };

    Ok((result, None))
}

/// Build Atomic BEEF bytes containing the transaction and its input proofs.
///
/// Constructs a Beef by:
/// 1. Merging input_beef if available (contains source txs with proofs)
/// 2. Merging the signed/unsigned transaction via merge_raw_tx
/// 3. Serializing as Atomic BEEF via to_binary_atomic
pub(crate) fn build_beef_bytes(
    tx: &bsv::transaction::transaction::Transaction,
    input_beef: &Option<Vec<u8>>,
) -> WalletResult<Vec<u8>> {
    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {}", e)))?;

    // Start with a fresh V1 BEEF and merge input_beef if available
    let mut beef = Beef::new(bsv::transaction::beef::BEEF_V1);
    if let Some(ref input_beef_bytes) = input_beef {
        if !input_beef_bytes.is_empty() {
            beef.merge_beef_from_binary(input_beef_bytes)
                .map_err(|e| WalletError::Internal(format!("Failed to merge input BEEF: {}", e)))?;
        }
    }

    // Serialize the transaction to raw bytes and merge via merge_raw_tx
    let mut raw_tx = Vec::new();
    tx.to_binary(&mut raw_tx)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize tx: {}", e)))?;
    beef.merge_raw_tx(&raw_tx, None)
        .map_err(|e| WalletError::Internal(format!("Failed to merge raw tx: {}", e)))?;

    // Serialize as Atomic BEEF targeting our transaction
    beef.to_binary_atomic(&txid)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize Atomic BEEF: {}", e)))
}
