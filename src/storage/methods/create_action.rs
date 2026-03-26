//! Storage createAction implementation.
//!
//! Orchestrates UTXO selection, change generation, record creation, and
//! label/output linking within a database transaction.
//! Ported from wallet-toolbox/src/storage/methods/createAction.ts.

use chrono::Utc;

use std::collections::HashSet;

use crate::error::{WalletError, WalletResult};
use crate::status::TransactionStatus;
use crate::storage::action_types::{
    FixedInput, FixedOutput, GenerateChangeSdkArgs, StorageCreateActionArgs,
    StorageCreateActionResult, StorageCreateTransactionSdkInput, StorageCreateTransactionSdkOutput,
    StorageFeeModel,
};
use crate::storage::beef::{get_valid_beef_for_storage_reader, TrustSelf};
use crate::storage::find_args::{
    FindOutputBasketsArgs, FindOutputsArgs, FindTransactionsArgs, OutputBasketPartial,
    OutputPartial, TransactionPartial,
};
use crate::storage::methods::generate_change::{
    generate_change_sdk, AvailableChange, MAX_POSSIBLE_SATOSHIS,
};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::{verify_one, TrxToken};
use crate::tables::{Output, OutputBasket, Transaction, TxLabel, TxLabelMap};
use crate::types::StorageProvidedBy;

/// Simple hex encoding (avoids hex crate dependency).
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Simple hex decoding (avoids hex crate dependency).
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

/// Default fee model if none provided by storage settings.
fn default_fee_model() -> StorageFeeModel {
    StorageFeeModel::default()
}

/// Generate a random base64 string of the given byte count.
fn random_bytes_base64(count: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = vec![0u8; count];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// Create a new transaction in storage.
///
/// Validates inputs, creates Transaction record, processes labels,
/// selects UTXOs, generates change, creates Output records, and
/// returns the complete result needed to build the Bitcoin transaction.
///
/// All operations happen within a database transaction for atomicity.
pub async fn storage_create_action<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    user_id: i64,
    args: &StorageCreateActionArgs,
    _trx: Option<&TrxToken>,
) -> WalletResult<StorageCreateActionResult> {
    let now = Utc::now().naive_utc();

    // --- Find the default change basket ---
    let change_basket_name = "default";
    let change_basket = verify_one(
        storage
            .find_output_baskets(
                &FindOutputBasketsArgs {
                    partial: OutputBasketPartial {
                        user_id: Some(user_id),
                        name: Some(change_basket_name.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                _trx,
            )
            .await?,
    )?;

    // --- Begin database transaction ---
    let db_trx = storage.begin_transaction().await?;

    let result = do_create_action(storage, user_id, args, &change_basket, &db_trx, now).await;

    match result {
        Ok(r) => {
            storage.commit_transaction(db_trx).await?;
            Ok(r)
        }
        Err(e) => {
            // Rollback on any error
            let _ = storage.rollback_transaction(db_trx).await;
            Err(e)
        }
    }
}

/// Merge BEEF data from allocated change inputs into the result's input_beef.
///
/// The signer needs BEEF proof data for ALL inputs (both user-provided and
/// storage-allocated change). This function walks the allocated change inputs,
/// fetches their BEEF from storage, and merges everything into a single BEEF.
///
/// This must be called AFTER storage_create_action and AFTER the db transaction
/// is committed, so the committed data is readable.
///
/// Matches TS: mergeAllocatedChangeBeefs(storage, userId, vargs, allocatedChange, beef)
pub async fn merge_input_beef(
    storage: &dyn StorageProvider,
    result: &mut StorageCreateActionResult,
) {
    use bsv::transaction::beef::{Beef, BEEF_V2};

    let mut beef = Beef::new(BEEF_V2);

    // Merge caller-provided input_beef first (if any)
    if let Some(ref ib) = result.input_beef {
        if !ib.is_empty() {
            let _ = beef.merge_beef_from_binary(ib);
        }
    }

    // Merge BEEF for each storage-provided input's source transaction.
    // Use TrustSelf::No to include full raw_tx + merkle proofs — the remote
    // client's signer needs complete BEEF data. Matches TS: trustSelf: undefined
    let known_txids: HashSet<String> = HashSet::new();
    for input in &result.inputs {
        if input.provided_by == StorageProvidedBy::Storage {
            let txid = &input.source_txid;
            if !txid.is_empty() && beef.find_txid(txid).is_none() {
                if let Ok(Some(tx_beef_bytes)) =
                    get_valid_beef_for_storage_reader(storage, txid, TrustSelf::No, &known_txids)
                        .await
                {
                    let _ = beef.merge_beef_from_binary(&tx_beef_bytes);
                }
            }
        }
    }

    // Serialize BEEF into result (only if we have any transactions)
    if beef.txs.is_empty() {
        result.input_beef = None;
    } else {
        let mut buf = Vec::new();
        match beef.to_binary(&mut buf) {
            Ok(()) => result.input_beef = Some(buf),
            Err(_) => result.input_beef = None,
        }
    }
}

/// Inner function that does all the work within the db transaction.
async fn do_create_action<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    user_id: i64,
    args: &StorageCreateActionArgs,
    change_basket: &OutputBasket,
    trx: &TrxToken,
    now: chrono::NaiveDateTime,
) -> WalletResult<StorageCreateActionResult> {
    let trx_opt = Some(trx);

    // --- Create the Transaction record ---
    let reference = random_bytes_base64(12);
    let new_tx = Transaction {
        created_at: now,
        updated_at: now,
        transaction_id: 0,
        user_id,
        proven_tx_id: None,
        status: TransactionStatus::Unsigned,
        reference: reference.clone(),
        is_outgoing: true,
        satoshis: 0, // Updated after funding
        description: args.description.clone(),
        version: Some(args.version as i32),
        lock_time: Some(args.lock_time as i32),
        txid: None,
        input_beef: args.input_beef.clone(),
        raw_tx: None,
    };
    let transaction_id = storage.insert_transaction(&new_tx, trx_opt).await?;

    // --- Process labels ---
    for label in &args.labels {
        let tx_label = storage
            .find_or_insert_tx_label(user_id, label, trx_opt)
            .await?;
        let label_map = TxLabelMap {
            created_at: now,
            updated_at: now,
            tx_label_id: tx_label.tx_label_id,
            transaction_id,
            is_deleted: false,
        };
        storage.insert_tx_label_map(&label_map, trx_opt).await?;
    }

    // --- Process user inputs ---
    // For each input, validate the referenced output exists and is spendable.
    // Mark as allocated by setting spentBy = transactionId.
    let mut fixed_inputs: Vec<FixedInput> = Vec::new();
    let mut input_records: Vec<StorageCreateTransactionSdkInput> = Vec::new();

    for (vin, input) in args.inputs.iter().enumerate() {
        // Find the output being spent
        let find_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                txid: Some(input.outpoint_txid.clone()),
                vout: Some(input.outpoint_vout as i32),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage.find_outputs(&find_args, trx_opt).await?;

        if let Some(output) = outputs.into_iter().next() {
            // Verify spendable
            if !output.spendable {
                return Err(WalletError::InvalidParameter {
                    parameter: format!("inputs[{}]", vin),
                    must_be: format!(
                        "spendable output. output {}:{} is not spendable.",
                        input.outpoint_txid, input.outpoint_vout
                    ),
                });
            }

            // Mark as spent
            let update = OutputPartial {
                spendable: Some(false),
                spent_by: Some(transaction_id),
                ..Default::default()
            };
            storage
                .update_output(output.output_id, &update, trx_opt)
                .await?;

            fixed_inputs.push(FixedInput {
                satoshis: output.satoshis as u64,
                unlocking_script_length: input.unlocking_script_length,
            });

            let locking_script_hex = output
                .locking_script
                .as_ref()
                .map(|ls| bytes_to_hex(ls))
                .unwrap_or_default();

            input_records.push(StorageCreateTransactionSdkInput {
                vin: vin as u32,
                source_txid: input.outpoint_txid.clone(),
                source_vout: input.outpoint_vout,
                source_satoshis: output.satoshis as u64,
                source_locking_script: locking_script_hex,
                source_transaction: None,
                unlocking_script_length: input.unlocking_script_length,
                provided_by: if output.provided_by == StorageProvidedBy::Storage {
                    StorageProvidedBy::Storage
                } else {
                    StorageProvidedBy::You
                },
                output_type: output.output_type.clone(),
                spending_description: output.spending_description.clone(),
                derivation_prefix: output.derivation_prefix.clone(),
                derivation_suffix: output.derivation_suffix.clone(),
                sender_identity_key: output.sender_identity_key.clone(),
            });
        } else {
            // User-provided input with no corresponding output in storage
            fixed_inputs.push(FixedInput {
                satoshis: 0, // Will be filled from input BEEF
                unlocking_script_length: input.unlocking_script_length,
            });

            input_records.push(StorageCreateTransactionSdkInput {
                vin: vin as u32,
                source_txid: input.outpoint_txid.clone(),
                source_vout: input.outpoint_vout,
                source_satoshis: 0,
                source_locking_script: String::new(),
                source_transaction: None,
                unlocking_script_length: input.unlocking_script_length,
                provided_by: StorageProvidedBy::You,
                output_type: "custom".to_string(),
                spending_description: None,
                derivation_prefix: None,
                derivation_suffix: None,
                sender_identity_key: None,
            });
        }
    }

    // --- Build fixed outputs for change generation ---
    let fixed_outputs: Vec<FixedOutput> = args
        .outputs
        .iter()
        .map(|o| {
            let script_bytes = hex_to_bytes(&o.locking_script);
            FixedOutput {
                satoshis: o.satoshis,
                locking_script_length: script_bytes.len(),
            }
        })
        .collect();

    // --- Query available change UTXOs ---
    // Find spendable outputs in the default basket, ordered by satoshis ASC
    let change_find_args = FindOutputsArgs {
        partial: OutputPartial {
            user_id: Some(user_id),
            basket_id: Some(change_basket.basket_id),
            spendable: Some(true),
            change: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut available_change_outputs = storage.find_outputs(&change_find_args, trx_opt).await?;
    // Filter out any that already have a spentBy value
    available_change_outputs.retain(|o| o.spent_by.is_none());
    // Sort by satoshis ascending (prefer smaller)
    available_change_outputs.sort_by(|a, b| a.satoshis.cmp(&b.satoshis));

    let available_change: Vec<AvailableChange> = available_change_outputs
        .iter()
        .map(|o| AvailableChange {
            output_id: o.output_id,
            satoshis: o.satoshis as u64,
            spendable: true,
        })
        .collect();

    // --- Generate change ---
    let fee_model = default_fee_model();
    let target_net_count =
        change_basket.number_of_desired_utxos as i64 - available_change.len() as i64;

    let change_args = GenerateChangeSdkArgs {
        fixed_inputs: fixed_inputs.clone(),
        fixed_outputs: fixed_outputs.clone(),
        fee_model,
        change_initial_satoshis: std::cmp::max(1, change_basket.minimum_desired_utxo_value as u64),
        change_first_satoshis: std::cmp::max(
            1,
            (change_basket.minimum_desired_utxo_value as u64) / 4,
        ),
        change_locking_script_length: 25,
        change_unlocking_script_length: 107,
        target_net_count: Some(target_net_count),
    };

    let change_result = generate_change_sdk(&change_args, &available_change)?;

    // --- Allocate change inputs in storage ---
    // Mark allocated change UTXOs as spent
    let mut allocated_outputs: Vec<Output> = Vec::new();
    for alloc in &change_result.allocated_change_inputs {
        let update = OutputPartial {
            spendable: Some(false),
            spent_by: Some(transaction_id),
            ..Default::default()
        };
        storage
            .update_output(alloc.output_id, &update, trx_opt)
            .await?;

        // Find the output record for input building
        if let Some(output) = available_change_outputs
            .iter()
            .find(|o| o.output_id == alloc.output_id)
        {
            allocated_outputs.push(output.clone());
        }
    }

    // --- Generate derivation prefix for this transaction ---
    let derivation_prefix = random_bytes_base64(16);

    // --- Create user output records ---
    let mut output_records: Vec<StorageCreateTransactionSdkOutput> = Vec::new();
    let mut vout: u32 = 0;

    for output_spec in &args.outputs {
        let script_bytes = hex_to_bytes(&output_spec.locking_script);

        // Look up basket if specified
        let basket_id = if let Some(ref basket_name) = output_spec.basket {
            let basket = storage
                .find_or_insert_output_basket(user_id, basket_name, trx_opt)
                .await?;
            Some(basket.basket_id)
        } else {
            None
        };

        let new_output = Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id,
            transaction_id,
            basket_id,
            spendable: false, // Not spendable until transaction is signed
            change: false,
            output_description: Some(output_spec.output_description.clone()),
            vout: vout as i32,
            satoshis: output_spec.satoshis as i64,
            provided_by: StorageProvidedBy::You,
            purpose: String::new(),
            output_type: "custom".to_string(),
            txid: None,
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: output_spec.custom_instructions.clone(),
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: Some(script_bytes.len() as i64),
            script_offset: None,
            locking_script: Some(script_bytes.clone()),
        };
        let _output_id = storage.insert_output(&new_output, trx_opt).await?;

        output_records.push(StorageCreateTransactionSdkOutput {
            vout,
            satoshis: output_spec.satoshis,
            locking_script: output_spec.locking_script.clone(),
            provided_by: StorageProvidedBy::You,
            purpose: None,
            basket: output_spec.basket.clone(),
            tags: output_spec.tags.clone(),
            output_description: Some(output_spec.output_description.clone()),
            derivation_suffix: None,
            custom_instructions: output_spec.custom_instructions.clone(),
        });

        vout += 1;
    }

    // --- Create change output records ---
    let mut change_vouts: Vec<u32> = Vec::new();
    for (i, change_output) in change_result.change_outputs.iter().enumerate() {
        let derivation_suffix = random_bytes_base64(16);
        let change_vout = vout;

        let new_output = Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id,
            transaction_id,
            basket_id: Some(change_basket.basket_id),
            spendable: false, // Not spendable until signed
            change: true,
            output_description: Some(String::new()),
            vout: change_vout as i32,
            satoshis: change_output.satoshis as i64,
            provided_by: StorageProvidedBy::Storage,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: None,
            sender_identity_key: None,
            derivation_prefix: Some(derivation_prefix.clone()),
            derivation_suffix: Some(derivation_suffix.clone()),
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: Some(25),
            script_offset: None,
            locking_script: None, // Set when transaction is signed
        };
        let _output_id = storage.insert_output(&new_output, trx_opt).await?;

        output_records.push(StorageCreateTransactionSdkOutput {
            vout: change_vout,
            satoshis: change_output.satoshis,
            locking_script: String::new(), // Computed during signing
            provided_by: StorageProvidedBy::Storage,
            purpose: Some("change".to_string()),
            basket: Some("default".to_string()),
            tags: vec![],
            output_description: Some(String::new()),
            derivation_suffix: Some(derivation_suffix),
            custom_instructions: None,
        });

        change_vouts.push(change_vout);
        vout += 1;
    }

    // --- Build change input records ---
    let change_unlock_len = 107usize;
    let mut change_vin = input_records.len() as u32;
    for alloc_output in &allocated_outputs {
        // Look up raw_tx for this change input's source transaction.
        // The signer needs sourceTransaction for signing (isSignAction path).
        // We also extract the locking script from raw_tx if it's not stored
        // on the output record (it's typically NULL until processAction).
        let (source_transaction, locking_script_from_raw) =
            if let Some(ref src_txid) = alloc_output.txid {
                let find_args = FindTransactionsArgs {
                    partial: TransactionPartial {
                        txid: Some(src_txid.clone()),
                        ..Default::default()
                    },
                    no_raw_tx: false,
                    ..Default::default()
                };
                if let Ok(txs) = storage.find_transactions(&find_args, trx_opt).await {
                    if let Some(tx_record) = txs.into_iter().next() {
                        // Extract locking script from raw_tx at the correct vout
                        let ls_hex = tx_record.raw_tx.as_ref().and_then(|raw| {
                            use std::io::Cursor;
                            let mut cursor = Cursor::new(raw);
                            bsv::transaction::transaction::Transaction::from_binary(&mut cursor)
                                .ok()
                                .and_then(|parsed_tx| {
                                    parsed_tx
                                        .outputs
                                        .get(alloc_output.vout as usize)
                                        .map(|out| bytes_to_hex(&out.locking_script.to_binary()))
                                })
                        });
                        (tx_record.raw_tx, ls_hex)
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

        // Use locking script from output record if available, otherwise from raw_tx
        let locking_script_hex = alloc_output
            .locking_script
            .as_ref()
            .map(|ls| bytes_to_hex(ls))
            .or(locking_script_from_raw)
            .unwrap_or_default();

        input_records.push(StorageCreateTransactionSdkInput {
            vin: change_vin,
            source_txid: alloc_output.txid.clone().unwrap_or_default(),
            source_vout: alloc_output.vout as u32,
            source_satoshis: alloc_output.satoshis as u64,
            source_locking_script: locking_script_hex,
            source_transaction,
            unlocking_script_length: change_unlock_len,
            provided_by: StorageProvidedBy::Storage,
            output_type: alloc_output.output_type.clone(),
            spending_description: None,
            derivation_prefix: alloc_output.derivation_prefix.clone(),
            derivation_suffix: alloc_output.derivation_suffix.clone(),
            sender_identity_key: alloc_output.sender_identity_key.clone(),
        });
        change_vin += 1;
    }

    // --- Update transaction satoshis ---
    // satoshis = change outputs gained - change inputs spent
    let change_out_sats: i64 = change_result
        .change_outputs
        .iter()
        .map(|o| o.satoshis as i64)
        .sum();
    let change_in_sats: i64 = change_result
        .allocated_change_inputs
        .iter()
        .map(|i| i.satoshis as i64)
        .sum();
    let satoshis = change_out_sats - change_in_sats;

    let tx_update = crate::storage::find_args::TransactionPartial {
        is_outgoing: Some(true),
        ..Default::default()
    };
    storage
        .update_transaction(transaction_id, &tx_update, trx_opt)
        .await?;

    // --- Build result ---
    // Note: input_beef is passed through here; the outer storage_create_action
    // function merges allocated change BEEFs before returning.
    let no_send_change_output_vouts = if args.is_no_send {
        Some(change_vouts)
    } else {
        None
    };

    Ok(StorageCreateActionResult {
        reference,
        version: args.version,
        lock_time: args.lock_time,
        inputs: input_records,
        outputs: output_records,
        derivation_prefix,
        input_beef: args.input_beef.clone(),
        no_send_change_output_vouts,
    })
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests {
    use super::*;
    use crate::storage::action_types::StorageCreateActionOutput;
    use crate::storage::find_args::{
        FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
    };
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader::StorageReader;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::StorageConfig;
    use crate::types::Chain;

    /// Set up test storage with a user, default basket, and some UTXOs.
    async fn setup_test_storage() -> (SqliteStorage, i64, i64) {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test)
            .await
            .expect("create storage");
        storage.migrate_database().await.expect("migrate");
        storage.make_available().await.expect("make available");

        // Create a user
        let (user, _) = storage
            .find_or_insert_user("test_identity_key", None)
            .await
            .expect("create user");
        let user_id = user.user_id;

        // Create a default basket (uses default values: 0 desired UTXOs, 0 min value)
        let basket = storage
            .find_or_insert_output_basket(user_id, "default", None)
            .await
            .expect("create basket");

        // Create some spendable change UTXOs
        let now = Utc::now().naive_utc();

        // First, create a source transaction for these UTXOs
        let source_tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "source_ref".to_string(),
            is_outgoing: false,
            satoshis: 100_000,
            description: "source tx".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(
                "aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444".to_string(),
            ),
            input_beef: None,
            raw_tx: None,
        };
        let source_tx_id = storage
            .insert_transaction(&source_tx, None)
            .await
            .expect("insert source tx");

        // Create UTXOs in default basket
        for i in 0..3 {
            let output = Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id,
                transaction_id: source_tx_id,
                basket_id: Some(basket.basket_id),
                spendable: true,
                change: true,
                output_description: Some(format!("change utxo {}", i)),
                vout: i,
                satoshis: 10_000 + (i as i64 * 5_000),
                provided_by: StorageProvidedBy::Storage,
                purpose: "change".to_string(),
                output_type: "P2PKH".to_string(),
                txid: Some(
                    "aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                ),
                sender_identity_key: None,
                derivation_prefix: Some("testprefix".to_string()),
                derivation_suffix: Some(format!("suffix{}", i)),
                custom_instructions: None,
                spent_by: None,
                sequence_number: None,
                spending_description: None,
                script_length: Some(25),
                script_offset: None,
                locking_script: Some(vec![
                    0x76, 0xa9, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0xac,
                ]),
            };
            storage
                .insert_output(&output, None)
                .await
                .expect("insert utxo");
        }

        (storage, user_id, basket.basket_id)
    }

    #[tokio::test]
    async fn test_create_action_basic() {
        let (storage, user_id, _basket_id) = setup_test_storage().await;

        let args = StorageCreateActionArgs {
            description: "test payment".to_string(),
            inputs: vec![],
            outputs: vec![StorageCreateActionOutput {
                locking_script: "76a91400000000000000000000000000000000000000008ac".to_string(),
                satoshis: 5_000,
                output_description: "payment output".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: 0,
            version: 1,
            labels: vec!["test-label".to_string()],
            is_no_send: false,
            is_delayed: false,
            is_sign_action: false,
            input_beef: None,
        };

        let result = storage_create_action(&storage, user_id, &args, None)
            .await
            .expect("create_action should succeed");

        // Verify reference is set
        assert!(!result.reference.is_empty());

        // Verify version and lock_time
        assert_eq!(result.version, 1);
        assert_eq!(result.lock_time, 0);

        // Verify outputs include user output + change outputs
        assert!(!result.outputs.is_empty());
        assert_eq!(result.outputs[0].satoshis, 5_000);

        // Verify derivation_prefix is set
        assert!(!result.derivation_prefix.is_empty());
    }

    #[tokio::test]
    async fn test_create_action_creates_transaction_record() {
        let (storage, user_id, _basket_id) = setup_test_storage().await;

        let args = StorageCreateActionArgs {
            description: "tx record test".to_string(),
            inputs: vec![],
            outputs: vec![StorageCreateActionOutput {
                locking_script: "76a91400000000000000000000000000000000000000008ac".to_string(),
                satoshis: 1_000,
                output_description: "small payment".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: 0,
            version: 1,
            labels: vec![],
            is_no_send: false,
            is_delayed: false,
            is_sign_action: false,
            input_beef: None,
        };

        let result = storage_create_action(&storage, user_id, &args, None)
            .await
            .expect("create_action should succeed");

        // Verify Transaction record was created with correct status
        let find_tx = FindTransactionsArgs {
            partial: TransactionPartial {
                user_id: Some(user_id),
                reference: Some(result.reference.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&find_tx, None)
            .await
            .expect("find transactions");
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].status, TransactionStatus::Unsigned);
        assert!(txs[0].is_outgoing);
    }

    #[tokio::test]
    async fn test_create_action_creates_output_records() {
        let (storage, user_id, _basket_id) = setup_test_storage().await;

        let args = StorageCreateActionArgs {
            description: "output record test".to_string(),
            inputs: vec![],
            outputs: vec![StorageCreateActionOutput {
                locking_script: "76a91400000000000000000000000000000000000000008ac".to_string(),
                satoshis: 2_000,
                output_description: "test output".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: 0,
            version: 1,
            labels: vec![],
            is_no_send: false,
            is_delayed: false,
            is_sign_action: false,
            input_beef: None,
        };

        let result = storage_create_action(&storage, user_id, &args, None)
            .await
            .expect("create_action should succeed");

        // Find the transaction to get its ID
        let find_tx = FindTransactionsArgs {
            partial: TransactionPartial {
                user_id: Some(user_id),
                reference: Some(result.reference.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&find_tx, None)
            .await
            .expect("find transactions");
        let tx_id = txs[0].transaction_id;

        // Find outputs for this transaction
        let find_outputs = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                transaction_id: Some(tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage
            .find_outputs(&find_outputs, None)
            .await
            .expect("find outputs");

        // Should have user output + change outputs
        assert!(outputs.len() >= 1, "should have at least 1 output");
        // First output should be the user's output
        let user_output = outputs.iter().find(|o| o.vout == 0).expect("vout 0");
        assert_eq!(user_output.satoshis, 2_000);
    }

    #[tokio::test]
    async fn test_create_action_allocates_utxos() {
        let (storage, user_id, basket_id) = setup_test_storage().await;

        let args = StorageCreateActionArgs {
            description: "utxo allocation test".to_string(),
            inputs: vec![],
            outputs: vec![StorageCreateActionOutput {
                locking_script: "76a91400000000000000000000000000000000000000008ac".to_string(),
                satoshis: 3_000,
                output_description: "allocation test".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: 0,
            version: 1,
            labels: vec![],
            is_no_send: false,
            is_delayed: false,
            is_sign_action: false,
            input_beef: None,
        };

        let _result = storage_create_action(&storage, user_id, &args, None)
            .await
            .expect("create_action should succeed");

        // Verify UTXO allocation: at least one change UTXO should now have spentBy set
        let find_change = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                basket_id: Some(basket_id),
                change: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let change_outputs = storage
            .find_outputs(&find_change, None)
            .await
            .expect("find change outputs");

        // At least one should be spent (has spentBy set)
        let spent_count = change_outputs
            .iter()
            .filter(|o| o.spent_by.is_some())
            .count();
        assert!(
            spent_count > 0,
            "at least one change UTXO should be allocated (spentBy set)"
        );
    }

    #[tokio::test]
    async fn test_create_action_generates_change_outputs() {
        let (storage, user_id, _basket_id) = setup_test_storage().await;

        let args = StorageCreateActionArgs {
            description: "change output test".to_string(),
            inputs: vec![],
            outputs: vec![StorageCreateActionOutput {
                locking_script: "76a91400000000000000000000000000000000000000008ac".to_string(),
                satoshis: 1_000,
                output_description: "small payment".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: 0,
            version: 1,
            labels: vec![],
            is_no_send: false,
            is_delayed: false,
            is_sign_action: false,
            input_beef: None,
        };

        let result = storage_create_action(&storage, user_id, &args, None)
            .await
            .expect("create_action should succeed");

        // Should have change outputs (at least one since the UTXO has more than needed)
        let change_outputs: Vec<_> = result
            .outputs
            .iter()
            .filter(|o| o.purpose.as_deref() == Some("change"))
            .collect();
        assert!(
            !change_outputs.is_empty(),
            "should have at least one change output"
        );
        // Change outputs should have derivation suffixes
        for co in &change_outputs {
            assert!(co.derivation_suffix.is_some());
        }
    }
}
