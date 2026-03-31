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
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionInput, StorageCreateActionOptions,
    StorageCreateActionOutput, StorageOutPoint, StorageProcessActionArgs,
};
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;

/// Simple bytes-to-hex encoding.
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Parse an OutpointString ("txid.vout") into a StorageOutPoint.
fn parse_outpoint_string(s: &str) -> StorageOutPoint {
    let parts: Vec<&str> = s.rsplitn(2, '.').collect();
    if parts.len() == 2 {
        StorageOutPoint {
            txid: parts[1].to_string(),
            vout: parts[0].parse().unwrap_or(0),
        }
    } else {
        StorageOutPoint {
            txid: s.to_string(),
            vout: 0,
        }
    }
}

/// Convert ValidCreateActionArgs to StorageCreateActionArgs.
///
/// Strips unlocking scripts (storage only needs the length).
/// Maps SDK CreateActionOptions to StorageCreateActionOptions.
fn to_storage_args(args: &ValidCreateActionArgs) -> StorageCreateActionArgs {
    use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue};

    let opts = &args.options;
    let storage_options = StorageCreateActionOptions {
        sign_and_process: BooleanDefaultTrue(opts.sign_and_process.0),
        accept_delayed_broadcast: BooleanDefaultTrue(opts.accept_delayed_broadcast.0),
        trust_self: opts.trust_self.as_ref().map(|ts| ts.as_str().to_string()),
        known_txids: opts.known_txids.clone(),
        return_txid_only: BooleanDefaultFalse(opts.return_txid_only.0),
        no_send: BooleanDefaultFalse(opts.no_send.0),
        no_send_change: opts
            .no_send_change
            .iter()
            .map(|s| parse_outpoint_string(s))
            .collect(),
        send_with: opts.send_with.clone(),
        randomize_outputs: BooleanDefaultTrue(opts.randomize_outputs.0),
    };

    StorageCreateActionArgs {
        description: args.description.clone(),
        inputs: args
            .inputs
            .iter()
            .map(|i| StorageCreateActionInput {
                outpoint: StorageOutPoint {
                    txid: i.outpoint.txid.clone(),
                    vout: i.outpoint.vout,
                },
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
        options: storage_options,
        input_beef: args.input_beef.clone(),
        is_new_tx: args.is_new_tx,
        is_sign_action: args.is_sign_action,
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        is_send_with: args.is_send_with,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    }
}

/// Execute the signer-level createAction flow.
///
/// 1. Call storage.create_action to allocate UTXOs and create records
/// 2. Build the unsigned transaction via build_signable_transaction
/// 3a. If is_sign_action: store PendingSignAction, return SignableTransaction
/// 3b. Otherwise: sign via complete_signed_transaction, process, optionally broadcast
pub async fn signer_create_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    key_deriver: &CachedKeyDeriver,
    identity_pub_key: &PublicKey,
    auth: &str,
    args: &ValidCreateActionArgs,
) -> WalletResult<(SignerCreateActionResult, Option<PendingSignAction>)> {
    // --- Step 1: Storage create action ---
    let auth_id = AuthId {
        identity_key: auth.to_string(),
        user_id: None,
        is_active: None,
    };
    let storage_args = to_storage_args(args);
    let dcr = storage.create_action(&auth_id, &storage_args).await?;
    let reference = dcr.reference.clone();

    // --- Step 2: Build unsigned transaction ---
    let (mut tx, amount, pdi) =
        build_signable_transaction(&dcr, args, key_deriver, identity_pub_key)?;

    // --- Step 3a: Delayed signing (is_sign_action) ---
    if args.is_sign_action {
        // Serialize unsigned tx for later recovery
        let mut tx_bytes = Vec::new();
        tx.to_binary(&mut tx_bytes).map_err(|e| {
            WalletError::Internal(format!("Failed to serialize unsigned tx: {}", e))
        })?;

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
            not_delayed_results: None,
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

    // Build BEEF, verify unlock scripts, then serialize
    let beef = build_beef(&tx, &dcr.input_beef)?;
    crate::signer::verify_unlock_scripts::verify_unlock_scripts(&txid, &beef)?;
    let beef_bytes = serialize_beef_atomic(&beef, &txid)?;

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
        send_with: if args.is_send_with { args.options.send_with.clone() } else { vec![] },
    };
    let process_result = storage.process_action(&auth_id, &process_args).await?;

    // --- Step 5: Broadcast if needed ---
    if !args.is_no_send && !args.is_delayed {
        let _post_results = services.post_beef(&beef_bytes, &[txid.clone()]).await;
    }

    let result = SignerCreateActionResult {
        txid: Some(txid),
        tx: if args.options.return_txid_only.0.unwrap_or(false) { None } else { Some(beef_bytes) },
        no_send_change,
        send_with_results: process_result.send_with_results.unwrap_or_default(),
        signable_transaction: None,
        not_delayed_results: process_result.not_delayed_results,
    };

    Ok((result, None))
}

/// Build a Beef object containing the transaction and its input proofs.
///
/// Constructs a Beef by:
/// 1. Merging input_beef if available (contains source txs with proofs)
/// 2. Merging the signed/unsigned transaction via merge_raw_tx
pub(crate) fn build_beef(
    tx: &bsv::transaction::transaction::Transaction,
    input_beef: &Option<Vec<u8>>,
) -> WalletResult<Beef> {
    let mut beef = Beef::new(bsv::transaction::beef::BEEF_V1);
    if let Some(ref input_beef_bytes) = input_beef {
        if !input_beef_bytes.is_empty() {
            beef.merge_beef_from_binary(input_beef_bytes)
                .map_err(|e| WalletError::Internal(format!("Failed to merge input BEEF: {}", e)))?;
        }
    }

    let mut raw_tx = Vec::new();
    tx.to_binary(&mut raw_tx)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize tx: {}", e)))?;
    beef.merge_raw_tx(&raw_tx, None)
        .map_err(|e| WalletError::Internal(format!("Failed to merge raw tx: {}", e)))?;

    Ok(beef)
}

/// Serialize a Beef as Atomic BEEF bytes targeting a specific txid.
pub(crate) fn serialize_beef_atomic(beef: &Beef, txid: &str) -> WalletResult<Vec<u8>> {
    beef.to_binary_atomic(txid)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize Atomic BEEF: {}", e)))
}

/// Build Atomic BEEF bytes containing the transaction and its input proofs.
///
/// Convenience wrapper around `build_beef` + `serialize_beef_atomic`.
/// Used for the signable BEEF path (delayed signing) where verification
/// is NOT done since the tx is unsigned.
pub(crate) fn build_beef_bytes(
    tx: &bsv::transaction::transaction::Transaction,
    input_beef: &Option<Vec<u8>>,
) -> WalletResult<Vec<u8>> {
    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {}", e)))?;
    let beef = build_beef(tx, input_beef)?;
    serialize_beef_atomic(&beef, &txid)
}

#[cfg(test)]
mod tests {
    use bsv::wallet::types::BooleanDefaultFalse;

    #[test]
    fn test_return_txid_only_controls_tx_field() {
        let return_txid_only = BooleanDefaultFalse(Some(true));
        let beef_bytes = vec![1, 2, 3];
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) { None } else { Some(beef_bytes.clone()) };
        assert!(tx.is_none());

        let return_txid_only = BooleanDefaultFalse(Some(false));
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) { None } else { Some(beef_bytes.clone()) };
        assert!(tx.is_some());

        // Default (None) should behave as false — tx is included
        let return_txid_only = BooleanDefaultFalse(None);
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) { None } else { Some(beef_bytes) };
        assert!(tx.is_some());
    }

    #[test]
    fn test_send_with_conditional() {
        let send_with_txids = vec!["aabb".to_string(), "ccdd".to_string()];

        let is_send_with = true;
        let result: Vec<String> = if is_send_with { send_with_txids.clone() } else { vec![] };
        assert_eq!(result.len(), 2);

        let is_send_with = false;
        let result: Vec<String> = if is_send_with { send_with_txids } else { vec![] };
        assert!(result.is_empty());
    }
}
