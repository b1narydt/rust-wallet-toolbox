//! Storage internalizeAction implementation.
//!
//! Takes ownership of outputs from an external transaction by parsing
//! AtomicBEEF, validating merkle proofs, and mapping outputs to baskets.
//! Supports both wallet-payment and basket-insertion protocols.
//! Ported from wallet-toolbox/src/storage/methods/internalizeAction.ts.

use std::io::Cursor;

use chrono::Utc;

use bsv::transaction::Beef;
use bsv::wallet::interfaces::InternalizeOutput;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::status::TransactionStatus;
use crate::storage::action_types::{StorageInternalizeActionArgs, StorageInternalizeActionResult};
use crate::storage::find_args::{
    FindOutputsArgs, FindProvenTxsArgs, FindTransactionsArgs, OutputPartial, ProvenTxPartial,
    TransactionPartial,
};
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::{verify_one_or_none, TrxToken};
use crate::tables::{Output, ProvenTx, Transaction};
use crate::types::StorageProvidedBy;

/// Internalize outputs from an external transaction.
///
/// Parses AtomicBEEF, validates merkle proofs via chain tracker, creates
/// Transaction and Output records, and handles both wallet-payment and
/// basket-insertion protocols.
pub async fn storage_internalize_action<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    services: &dyn WalletServices,
    user_id: i64,
    args: &StorageInternalizeActionArgs,
    _trx: Option<&TrxToken>,
) -> WalletResult<StorageInternalizeActionResult> {
    // Parse AtomicBEEF
    let mut cursor = Cursor::new(&args.tx);
    let ab = Beef::from_binary(&mut cursor).map_err(|e| WalletError::InvalidParameter {
        parameter: "tx".to_string(),
        must_be: format!("valid AtomicBEEF: {}", e),
    })?;

    // Validate merkle proofs via chain tracker
    let chain_tracker = services.get_chain_tracker().await?;
    for btx in &ab.txs {
        if let Some(bump_idx) = btx.bump_index {
            if let Some(bump) = ab.bumps.get(bump_idx) {
                let merkle_root = bump.compute_root(Some(&btx.txid)).map_err(|e| {
                    WalletError::Internal(format!("Failed to compute merkle root: {}", e))
                })?;
                let valid = chain_tracker
                    .is_valid_root_for_height(&merkle_root, bump.block_height)
                    .await
                    .map_err(|e| WalletError::Internal(format!("Chain tracker error: {}", e)))?;
                if !valid {
                    return Err(WalletError::InvalidParameter {
                        parameter: "tx".to_string(),
                        must_be: format!(
                            "valid AtomicBEEF with valid merkle proof for tx {}",
                            btx.txid
                        ),
                    });
                }
            }
        }
    }

    // Get the atomic txid (the newest/main transaction in the BEEF)
    let txid = ab.atomic_txid.as_ref().cloned().unwrap_or_else(|| {
        // Fallback: use the last transaction's txid
        ab.txs.last().map(|t| t.txid.clone()).unwrap_or_default()
    });

    if txid.is_empty() {
        return Err(WalletError::InvalidParameter {
            parameter: "tx".to_string(),
            must_be: "an AtomicBEEF with an identifiable transaction".to_string(),
        });
    }

    // Find the main transaction in the BEEF
    let beef_tx =
        ab.txs
            .iter()
            .find(|t| t.txid == txid)
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "tx".to_string(),
                must_be: format!("valid AtomicBEEF containing transaction {}", txid),
            })?;

    let tx = beef_tx.tx.as_ref().ok_or_else(|| {
        WalletError::Internal(format!("BEEF transaction {} has no raw tx data", txid))
    })?;

    let num_outputs = tx.outputs.len();

    // Validate output indices
    for out_spec in &args.outputs {
        let oi = output_index_of(out_spec);
        if oi >= num_outputs as u32 {
            return Err(WalletError::InvalidParameter {
                parameter: "outputIndex".to_string(),
                must_be: format!("a valid output index in range 0 to {}", num_outputs - 1),
            });
        }
    }

    // Check if this transaction already exists in storage (merge case)
    let find_tx_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            txid: Some(txid.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let existing_tx = verify_one_or_none(storage.find_transactions(&find_tx_args, _trx).await?)?;
    let is_merge = existing_tx.is_some();

    if let Some(ref etx) = existing_tx {
        if etx.status != TransactionStatus::Completed
            && etx.status != TransactionStatus::Unproven
            && etx.status != TransactionStatus::Nosend
        {
            return Err(WalletError::InvalidParameter {
                parameter: "tx".to_string(),
                must_be: format!(
                    "target transaction of internalizeAction has valid status, got {}",
                    etx.status
                ),
            });
        }
    }

    // Get or create the default basket
    let default_basket = storage
        .find_or_insert_output_basket(user_id, "default", _trx)
        .await?;

    // Calculate net satoshis change
    let mut satoshis: i64 = 0;

    // Pre-calculate satoshis for each output spec
    for out_spec in &args.outputs {
        let oi = output_index_of(out_spec);
        let txo = &tx.outputs[oi as usize];
        let output_satoshis = txo.satoshis.unwrap_or(0) as i64;

        match out_spec {
            InternalizeOutput::WalletPayment { .. } => {
                if is_merge {
                    let find_out = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            txid: Some(txid.clone()),
                            vout: Some(oi as i32),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let eo = verify_one_or_none(storage.find_outputs(&find_out, _trx).await?)?;
                    if let Some(ref existing_output) = eo {
                        if existing_output.basket_id == Some(default_basket.basket_id) {
                            // Ignore existing change output
                        } else {
                            satoshis += output_satoshis;
                        }
                    } else {
                        satoshis += output_satoshis;
                    }
                } else {
                    satoshis += output_satoshis;
                }
            }
            InternalizeOutput::BasketInsertion { .. } => {
                if is_merge {
                    let find_out = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            txid: Some(txid.clone()),
                            vout: Some(oi as i32),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let eo = verify_one_or_none(storage.find_outputs(&find_out, _trx).await?)?;
                    if let Some(ref existing_output) = eo {
                        if existing_output.basket_id == Some(default_basket.basket_id) {
                            satoshis -= output_satoshis;
                        }
                    }
                }
            }
        }
    }

    // Check if the BEEF includes a merkle proof for the main transaction
    let has_proof = beef_tx.has_proof();

    let tx_status = if has_proof {
        TransactionStatus::Completed
    } else {
        TransactionStatus::Unproven
    };

    // Begin database transaction
    let db_trx = storage.begin_transaction().await?;

    // Store ALL proven ancestor transactions from the BEEF into proven_txs table.
    // This ensures get_valid_beef_for_txid can later reconstruct BEEF for any
    // transaction that spends outputs from these ancestors.
    let now_for_proven = Utc::now().naive_utc();
    for btx in &ab.txs {
        if let Some(bump_idx) = btx.bump_index {
            if let Some(bump) = ab.bumps.get(bump_idx) {
                // This tx has a merkle proof — store it as proven
                if let Some(ref bsv_tx) = btx.tx {
                    let ancestor_txid = &btx.txid;
                    let merkle_root = bump.compute_root(Some(ancestor_txid)).unwrap_or_default();

                    let raw_tx_bytes = bsv_tx.to_bytes().unwrap_or_default();
                    let mut bump_bytes = Vec::new();
                    let _ = bump.to_binary(&mut bump_bytes);

                    // Check if ProvenTx already exists
                    let find_proven = FindProvenTxsArgs {
                        partial: ProvenTxPartial {
                            txid: Some(ancestor_txid.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let existing = verify_one_or_none(
                        storage.find_proven_txs(&find_proven, Some(&db_trx)).await?,
                    )?;

                    if existing.is_none() && !raw_tx_bytes.is_empty() && !bump_bytes.is_empty() {
                        let new_proven = ProvenTx {
                            created_at: now_for_proven,
                            updated_at: now_for_proven,
                            proven_tx_id: 0,
                            txid: ancestor_txid.clone(),
                            height: bump.block_height as i32,
                            index: 0,
                            merkle_path: bump_bytes,
                            raw_tx: raw_tx_bytes,
                            block_hash: String::new(),
                            merkle_root,
                        };
                        let _ = storage.insert_proven_tx(&new_proven, Some(&db_trx)).await;
                    }
                }
            }
        }
    }

    // Create or find ProvenTx for the MAIN transaction if it has proof
    let mut proven_tx_id: Option<i64> = None;
    if has_proof {
        if let Some(bump_idx) = beef_tx.bump_index {
            if let Some(bump) = ab.bumps.get(bump_idx) {
                let merkle_root = bump.compute_root(Some(&txid)).map_err(|e| {
                    WalletError::Internal(format!("Failed to compute merkle root: {}", e))
                })?;

                let now = Utc::now().naive_utc();
                let raw_tx_bytes = tx.to_bytes().unwrap_or_default();

                let mut bump_bytes = Vec::new();
                bump.to_binary(&mut bump_bytes).map_err(|e| {
                    WalletError::Internal(format!("Failed to serialize merkle path: {}", e))
                })?;

                // Check if ProvenTx already exists (may have been inserted above)
                let find_proven = FindProvenTxsArgs {
                    partial: ProvenTxPartial {
                        txid: Some(txid.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let existing_proven = verify_one_or_none(
                    storage.find_proven_txs(&find_proven, Some(&db_trx)).await?,
                )?;

                if let Some(ep) = existing_proven {
                    proven_tx_id = Some(ep.proven_tx_id);
                } else {
                    let new_proven = ProvenTx {
                        created_at: now,
                        updated_at: now,
                        proven_tx_id: 0,
                        txid: txid.clone(),
                        height: bump.block_height as i32,
                        index: 0,
                        merkle_path: bump_bytes,
                        raw_tx: raw_tx_bytes,
                        block_hash: String::new(),
                        merkle_root,
                    };
                    let ptx_id = storage.insert_proven_tx(&new_proven, Some(&db_trx)).await?;
                    proven_tx_id = Some(ptx_id);
                }
            }
        }
    }

    // Create or update the Transaction record
    let transaction_id = if let Some(ref etx) = existing_tx {
        let tid = etx.transaction_id;
        if satoshis != 0 {
            let update = TransactionPartial {
                status: Some(tx_status.clone()),
                ..Default::default()
            };
            storage
                .update_transaction(tid, &update, Some(&db_trx))
                .await?;
        }
        tid
    } else {
        let now = Utc::now().naive_utc();
        // Store raw_tx so get_valid_beef_for_txid can build BEEF later
        // (needed by createAction's merge_input_beef for remote clients)
        let raw_tx_bytes = tx.to_bytes().unwrap_or_default();
        // Store the full BEEF as input_beef to preserve ancestor proofs
        let beef_bytes = {
            let mut buf = Vec::new();
            ab.to_binary(&mut buf).ok();
            if buf.is_empty() {
                None
            } else {
                Some(buf)
            }
        };
        let new_tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id,
            status: tx_status,
            reference: format!("int_{}", &txid[..std::cmp::min(16, txid.len())]),
            is_outgoing: false,
            satoshis,
            description: args.description.clone(),
            version: Some(tx.version as i32),
            lock_time: Some(tx.lock_time as i32),
            txid: Some(txid.clone()),
            input_beef: beef_bytes,
            raw_tx: Some(raw_tx_bytes),
        };
        storage.insert_transaction(&new_tx, Some(&db_trx)).await?
    };

    // Add labels
    for label in &args.labels {
        let tx_label = storage
            .find_or_insert_tx_label(user_id, label, Some(&db_trx))
            .await?;
        let label_map = crate::tables::TxLabelMap {
            created_at: Utc::now().naive_utc(),
            updated_at: Utc::now().naive_utc(),
            transaction_id,
            tx_label_id: tx_label.tx_label_id,
            is_deleted: false,
        };
        let _ = storage.insert_tx_label_map(&label_map, Some(&db_trx)).await;
    }

    // Process each output specification
    for out_spec in &args.outputs {
        let oi = output_index_of(out_spec);
        let txo = &tx.outputs[oi as usize];
        let vout = oi as i32;
        let output_satoshis = txo.satoshis.unwrap_or(0) as i64;
        let locking_script = txo.locking_script.to_binary();

        match out_spec {
            InternalizeOutput::WalletPayment {
                output_index: _,
                payment,
            } => {
                let sender_key = Some(payment.sender_identity_key.to_der_hex());
                let prefix = Some(
                    String::from_utf8_lossy(&payment.derivation_prefix).to_string(),
                );
                let suffix = Some(
                    String::from_utf8_lossy(&payment.derivation_suffix).to_string(),
                );

                if is_merge {
                    let find_out = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            txid: Some(txid.clone()),
                            vout: Some(vout),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let eo =
                        verify_one_or_none(storage.find_outputs(&find_out, Some(&db_trx)).await?)?;

                    if let Some(existing_output) = eo {
                        if existing_output.basket_id == Some(default_basket.basket_id) {
                            continue; // No-op for existing change output
                        }
                        // Convert to change output
                        let update = OutputPartial {
                            basket_id: Some(default_basket.basket_id),
                            output_type: Some("P2PKH".to_string()),
                            change: Some(true),
                            provided_by: Some(StorageProvidedBy::Storage),
                            purpose: Some("change".to_string()),
                            sender_identity_key: sender_key.clone(),
                            ..Default::default()
                        };
                        storage
                            .update_output(existing_output.output_id, &update, Some(&db_trx))
                            .await?;
                    } else {
                        store_new_wallet_payment(
                            storage,
                            transaction_id,
                            user_id,
                            &txid,
                            vout,
                            output_satoshis,
                            &locking_script,
                            default_basket.basket_id,
                            sender_key.as_deref(),
                            prefix.as_deref(),
                            suffix.as_deref(),
                            Some(&db_trx),
                        )
                        .await?;
                    }
                } else {
                    store_new_wallet_payment(
                        storage,
                        transaction_id,
                        user_id,
                        &txid,
                        vout,
                        output_satoshis,
                        &locking_script,
                        default_basket.basket_id,
                        sender_key.as_deref(),
                        prefix.as_deref(),
                        suffix.as_deref(),
                        Some(&db_trx),
                    )
                    .await?;
                }
            }
            InternalizeOutput::BasketInsertion {
                output_index: _,
                insertion,
            } => {
                let basket_name = if insertion.basket.is_empty() {
                    "default"
                } else {
                    &insertion.basket
                };
                let basket = storage
                    .find_or_insert_output_basket(user_id, basket_name, Some(&db_trx))
                    .await?;

                if is_merge {
                    let find_out = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            txid: Some(txid.clone()),
                            vout: Some(vout),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let eo =
                        verify_one_or_none(storage.find_outputs(&find_out, Some(&db_trx)).await?)?;

                    if let Some(existing_output) = eo {
                        let update = OutputPartial {
                            basket_id: Some(basket.basket_id),
                            output_type: Some("custom".to_string()),
                            change: Some(false),
                            provided_by: Some(StorageProvidedBy::You),
                            purpose: Some(String::new()),
                            ..Default::default()
                        };
                        storage
                            .update_output(existing_output.output_id, &update, Some(&db_trx))
                            .await?;
                    } else {
                        store_new_basket_insertion(
                            storage,
                            transaction_id,
                            user_id,
                            &txid,
                            vout,
                            output_satoshis,
                            &locking_script,
                            basket.basket_id,
                            insertion.custom_instructions.as_deref(),
                            Some(&db_trx),
                        )
                        .await?;
                    }
                } else {
                    store_new_basket_insertion(
                        storage,
                        transaction_id,
                        user_id,
                        &txid,
                        vout,
                        output_satoshis,
                        &locking_script,
                        basket.basket_id,
                        insertion.custom_instructions.as_deref(),
                        Some(&db_trx),
                    )
                    .await?;
                }

                // Add tags for basket insertions
                for tag in &insertion.tags {
                    let output_tag = storage
                        .find_or_insert_output_tag(user_id, tag, Some(&db_trx))
                        .await?;
                    let find_out = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            transaction_id: Some(transaction_id),
                            vout: Some(vout),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    if let Some(out) =
                        verify_one_or_none(storage.find_outputs(&find_out, Some(&db_trx)).await?)?
                    {
                        let tag_map = crate::tables::OutputTagMap {
                            created_at: Utc::now().naive_utc(),
                            updated_at: Utc::now().naive_utc(),
                            output_id: out.output_id,
                            output_tag_id: output_tag.output_tag_id,
                            is_deleted: false,
                        };
                        let _ = storage.insert_output_tag_map(&tag_map, Some(&db_trx)).await;
                    }
                }
            }
        }
    }

    // Create ProvenTxReq if no proof exists (for monitor to collect proof later)
    if !has_proof && !is_merge {
        let now = Utc::now().naive_utc();
        let raw_tx_bytes = tx.to_bytes().unwrap_or_default();
        let notify = serde_json::json!({
            "transactionIds": [transaction_id]
        });
        let new_req = crate::tables::ProvenTxReq {
            created_at: now,
            updated_at: now,
            proven_tx_req_id: 0,
            proven_tx_id: None,
            status: crate::status::ProvenTxReqStatus::Unsent,
            attempts: 0,
            notified: false,
            txid: txid.clone(),
            batch: None,
            history: serde_json::json!([{
                "what": "internalizeAction",
                "userId": user_id
            }])
            .to_string(),
            notify: serde_json::to_string(&notify).unwrap_or_default(),
            raw_tx: raw_tx_bytes,
            input_beef: Some(args.tx.clone()),
        };
        let _ = storage
            .insert_proven_tx_req(&new_req, Some(&db_trx))
            .await?;
    }

    // Commit database transaction
    storage.commit_transaction(db_trx).await?;

    Ok(StorageInternalizeActionResult {
        accepted: true,
        is_merge,
        txid,
        satoshis,
        send_with_results: None,
        not_delayed_results: None,
    })
}

/// Extract the output_index from either variant of InternalizeOutput.
fn output_index_of(out: &InternalizeOutput) -> u32 {
    match out {
        InternalizeOutput::WalletPayment { output_index, .. } => *output_index,
        InternalizeOutput::BasketInsertion { output_index, .. } => *output_index,
    }
}

/// Create a new wallet payment output record.
async fn store_new_wallet_payment<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    transaction_id: i64,
    user_id: i64,
    txid: &str,
    vout: i32,
    satoshis: i64,
    locking_script: &[u8],
    basket_id: i64,
    sender_identity_key: Option<&str>,
    derivation_prefix: Option<&str>,
    derivation_suffix: Option<&str>,
    trx: Option<&TrxToken>,
) -> WalletResult<i64> {
    let now = Utc::now().naive_utc();
    let output = Output {
        created_at: now,
        updated_at: now,
        output_id: 0,
        transaction_id,
        user_id,
        spendable: true,
        locking_script: Some(locking_script.to_vec()),
        vout,
        basket_id: Some(basket_id),
        satoshis,
        txid: Some(txid.to_string()),
        sender_identity_key: sender_identity_key.map(|s| s.to_string()),
        output_type: "P2PKH".to_string(),
        provided_by: StorageProvidedBy::Storage,
        purpose: "change".to_string(),
        derivation_prefix: derivation_prefix.map(|s| s.to_string()),
        derivation_suffix: derivation_suffix.map(|s| s.to_string()),
        change: true,
        spent_by: None,
        custom_instructions: None,
        output_description: Some(String::new()),
        spending_description: None,
        script_length: None,
        script_offset: None,
        sequence_number: None,
    };
    storage.insert_output(&output, trx).await
}

/// Create a new basket insertion output record.
async fn store_new_basket_insertion<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    transaction_id: i64,
    user_id: i64,
    txid: &str,
    vout: i32,
    satoshis: i64,
    locking_script: &[u8],
    basket_id: i64,
    custom_instructions: Option<&str>,
    trx: Option<&TrxToken>,
) -> WalletResult<i64> {
    let now = Utc::now().naive_utc();
    let output = Output {
        created_at: now,
        updated_at: now,
        output_id: 0,
        transaction_id,
        user_id,
        spendable: true,
        locking_script: Some(locking_script.to_vec()),
        vout,
        basket_id: Some(basket_id),
        satoshis,
        txid: Some(txid.to_string()),
        output_type: "custom".to_string(),
        custom_instructions: custom_instructions.map(|s| s.to_string()),
        change: false,
        spent_by: None,
        output_description: Some(String::new()),
        spending_description: None,
        provided_by: StorageProvidedBy::You,
        purpose: String::new(),
        sender_identity_key: None,
        derivation_prefix: None,
        derivation_suffix: None,
        script_length: None,
        script_offset: None,
        sequence_number: None,
    };
    storage.insert_output(&output, trx).await
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests {
    use super::*;
    use crate::services::types;
    use crate::storage::find_args::{
        FindOutputsArgs, FindProvenTxReqsArgs, FindTransactionsArgs, OutputPartial,
        ProvenTxReqPartial, TransactionPartial,
    };
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader::StorageReader;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::StorageConfig;
    use crate::types::Chain;

    use async_trait::async_trait;
    use bsv::primitives::public_key::PublicKey;
    use bsv::script::LockingScript;
    use bsv::transaction::chain_tracker::ChainTracker;
    use bsv::transaction::error::TransactionError;
    use bsv::wallet::interfaces::{BasketInsertion, Payment};
    use bsv::transaction::{Transaction as BsvTransaction, TransactionInput, TransactionOutput};

    // Mock chain tracker that accepts all proofs
    struct MockChainTracker;

    #[async_trait]
    impl ChainTracker for MockChainTracker {
        async fn is_valid_root_for_height(
            &self,
            _root: &str,
            _height: u32,
        ) -> Result<bool, TransactionError> {
            Ok(true)
        }
    }

    // Mock WalletServices for testing
    struct MockWalletServices;

    #[async_trait]
    impl WalletServices for MockWalletServices {
        fn chain(&self) -> Chain {
            Chain::Test
        }

        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            Ok(Box::new(MockChainTracker))
        }

        async fn get_merkle_path(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> types::GetMerklePathResult {
            types::GetMerklePathResult {
                name: Some("mock".to_string()),
                merkle_path: None,
                header: None,
                error: None,
            }
        }

        async fn get_raw_tx(&self, txid: &str, _use_next: bool) -> types::GetRawTxResult {
            types::GetRawTxResult {
                txid: txid.to_string(),
                name: Some("mock".to_string()),
                raw_tx: None,
                error: None,
            }
        }

        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> Vec<types::PostBeefResult> {
            vec![]
        }

        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<types::GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> types::GetUtxoStatusResult {
            types::GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                is_utxo: Some(false),
                details: vec![],
            }
        }

        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> types::GetStatusForTxidsResult {
            types::GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                results: vec![],
            }
        }

        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> types::GetScriptHashHistoryResult {
            types::GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                history: vec![],
            }
        }

        async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }

        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Ok(vec![0u8; 80])
        }

        async fn get_height(&self) -> WalletResult<u32> {
            Ok(100_000)
        }

        async fn n_lock_time_is_final(&self, _input: types::NLockTimeInput) -> WalletResult<bool> {
            Ok(true)
        }

        async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
            Ok(types::BsvExchangeRate::default())
        }

        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }

        async fn get_fiat_exchange_rates(
            &self,
            _target_currencies: &[String],
        ) -> WalletResult<types::FiatExchangeRates> {
            Ok(types::FiatExchangeRates::default())
        }

        fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
            types::ServicesCallHistory { services: vec![] }
        }

        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }

        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }

        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            _txid: &str,
            _vout: u32,
        ) -> WalletResult<bool> {
            Ok(false)
        }
    }

    /// Helper to set up storage for internalize tests.
    async fn setup_test_storage() -> (SqliteStorage, i64) {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test)
            .await
            .expect("create storage");
        storage.migrate_database().await.expect("migrate");
        storage.make_available().await.expect("make available");

        let (user, _) = storage
            .find_or_insert_user("test_identity_key", None)
            .await
            .expect("create user");

        let _ = storage
            .find_or_insert_output_basket(user.user_id, "default", None)
            .await
            .expect("create basket");

        (storage, user.user_id)
    }

    /// Build a simple transaction and wrap it in an AtomicBEEF.
    fn create_test_atomic_beef() -> (Vec<u8>, String) {
        use bsv::script::UnlockingScript;

        // Build a minimal transaction
        let mut tx = BsvTransaction::new();
        tx.version = 1;
        tx.lock_time = 0;

        // Add a dummy input
        let input = TransactionInput {
            source_transaction: None,
            source_txid: Some("a".repeat(64)),
            source_output_index: 0,
            unlocking_script: Some(UnlockingScript::from_binary(&[0x00])),
            sequence: 0xFFFFFFFF,
        };
        tx.add_input(input);

        // Add two outputs with P2PKH-like scripts
        let script1 =
            LockingScript::from_hex("76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac").unwrap();
        let out1 = TransactionOutput {
            satoshis: Some(1000),
            locking_script: script1,
            change: false,
        };
        tx.add_output(out1);

        let script2 =
            LockingScript::from_hex("76a91400112233445566778899aabbccddeeff0011223388ac").unwrap();
        let out2 = TransactionOutput {
            satoshis: Some(2000),
            locking_script: script2,
            change: false,
        };
        tx.add_output(out2);

        let txid = tx.id().expect("compute txid");

        // Create a BEEF containing this transaction
        use bsv::transaction::beef_tx::BeefTx;
        let beef_tx = BeefTx::from_tx(tx, None).expect("create beef tx");
        let mut beef = Beef::new(bsv::transaction::beef::BEEF_V1);
        beef.txs.push(beef_tx);
        beef.atomic_txid = Some(txid.clone());

        let mut beef_bytes = Vec::new();
        beef.to_binary(&mut beef_bytes).expect("serialize beef");

        (beef_bytes, txid)
    }

    #[tokio::test]
    async fn test_internalize_wallet_payment() {
        let (storage, user_id) = setup_test_storage().await;
        let services = MockWalletServices;
        let (beef_bytes, txid) = create_test_atomic_beef();

        let sender_key = PublicKey::from_string(
            &("02".to_owned() + &"ab".repeat(32)),
        )
        .unwrap();

        let args = StorageInternalizeActionArgs {
            tx: beef_bytes,
            description: "test payment".to_string(),
            labels: vec!["test-label".to_string()],
            seek_permission: true,
            outputs: vec![InternalizeOutput::WalletPayment {
                output_index: 0,
                payment: Payment {
                    derivation_prefix: b"prefix1".to_vec(),
                    derivation_suffix: b"suffix1".to_vec(),
                    sender_identity_key: sender_key,
                },
            }],
        };

        let result = storage_internalize_action(&storage, &services, user_id, &args, None)
            .await
            .expect("internalize_action should succeed");

        assert!(result.accepted);
        assert!(!result.is_merge);
        assert_eq!(result.txid, txid);
        assert_eq!(result.satoshis, 1000);

        // Verify transaction was created
        let tx_args = FindTransactionsArgs {
            partial: TransactionPartial {
                user_id: Some(user_id),
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find txs");
        assert_eq!(txs.len(), 1);
        assert!(!txs[0].is_outgoing);
        assert_eq!(txs[0].status, TransactionStatus::Unproven);

        // Verify output was created in default basket
        let out_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage
            .find_outputs(&out_args, None)
            .await
            .expect("find outputs");
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].change);
        assert!(outputs[0].spendable);
        assert_eq!(outputs[0].output_type, "P2PKH");
        assert_eq!(outputs[0].purpose, "change");

        // Verify ProvenTxReq was created
        let req_args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let reqs = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find reqs");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].status, crate::status::ProvenTxReqStatus::Unsent);
    }

    #[tokio::test]
    async fn test_internalize_basket_insertion() {
        let (storage, user_id) = setup_test_storage().await;
        let services = MockWalletServices;
        let (beef_bytes, txid) = create_test_atomic_beef();

        let args = StorageInternalizeActionArgs {
            tx: beef_bytes,
            description: "test basket insert".to_string(),
            labels: vec![],
            seek_permission: true,
            outputs: vec![InternalizeOutput::BasketInsertion {
                output_index: 1,
                insertion: BasketInsertion {
                    basket: "custom-basket".to_string(),
                    custom_instructions: Some("special instructions".to_string()),
                    tags: vec!["tag1".to_string()],
                },
            }],
        };

        let result = storage_internalize_action(&storage, &services, user_id, &args, None)
            .await
            .expect("internalize_action should succeed");

        assert!(result.accepted);
        assert!(!result.is_merge);
        assert_eq!(result.txid, txid);
        assert_eq!(result.satoshis, 0);

        // Verify output was created with custom type
        let out_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage
            .find_outputs(&out_args, None)
            .await
            .expect("find outputs");
        assert_eq!(outputs.len(), 1);
        assert!(!outputs[0].change);
        assert_eq!(outputs[0].output_type, "custom");
        assert_eq!(
            outputs[0].custom_instructions,
            Some("special instructions".to_string())
        );
    }

    #[tokio::test]
    async fn test_internalize_invalid_beef_rejection() {
        let (storage, user_id) = setup_test_storage().await;
        let services = MockWalletServices;

        let args = StorageInternalizeActionArgs {
            tx: vec![0x00, 0x01, 0x02, 0x03],
            description: "bad beef".to_string(),
            labels: vec![],
            seek_permission: true,
            outputs: vec![],
        };

        let result = storage_internalize_action(&storage, &services, user_id, &args, None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), "WERR_INVALID_PARAMETER");
    }

    #[tokio::test]
    async fn test_internalize_both_protocols() {
        let (storage, user_id) = setup_test_storage().await;
        let services = MockWalletServices;
        let (beef_bytes, txid) = create_test_atomic_beef();

        let sender_key = PublicKey::from_string(
            &("02".to_owned() + &"ab".repeat(32)),
        )
        .unwrap();

        let args = StorageInternalizeActionArgs {
            tx: beef_bytes,
            description: "both protocols".to_string(),
            labels: vec![],
            seek_permission: true,
            outputs: vec![
                InternalizeOutput::WalletPayment {
                    output_index: 0,
                    payment: Payment {
                        derivation_prefix: b"p1".to_vec(),
                        derivation_suffix: b"s1".to_vec(),
                        sender_identity_key: sender_key,
                    },
                },
                InternalizeOutput::BasketInsertion {
                    output_index: 1,
                    insertion: BasketInsertion {
                        basket: "my-basket".to_string(),
                        custom_instructions: None,
                        tags: vec![],
                    },
                },
            ],
        };

        let result = storage_internalize_action(&storage, &services, user_id, &args, None)
            .await
            .expect("internalize_action should succeed");

        assert!(result.accepted);
        assert_eq!(result.satoshis, 1000);

        // Verify two outputs created
        let out_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage
            .find_outputs(&out_args, None)
            .await
            .expect("find outputs");
        assert_eq!(outputs.len(), 2);

        let change_outputs: Vec<_> = outputs.iter().filter(|o| o.change).collect();
        let custom_outputs: Vec<_> = outputs.iter().filter(|o| !o.change).collect();
        assert_eq!(change_outputs.len(), 1);
        assert_eq!(custom_outputs.len(), 1);
        assert_eq!(change_outputs[0].output_type, "P2PKH");
        assert_eq!(custom_outputs[0].output_type, "custom");
    }
}
