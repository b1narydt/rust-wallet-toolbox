//! Regression tests for the TS-parity fixes in the tier-1 batch.
//!
//! Each test pins a behaviour the toolbox claimed but did not have. They are
//! grouped here rather than scattered so the batch can be reviewed against the
//! TS reference (ts-stack `wallet-toolbox` 2.10.2) in one pass.

#![cfg(feature = "sqlite")]

use bsv::wallet::interfaces::AbortActionArgs;
use chrono::NaiveDateTime;

use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::find_args::*;
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::tables::*;
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
use bsv_wallet_toolbox::wallet::types::AuthId;

const IDENTITY: &str = "02aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";

async fn storage() -> WalletResult<SqliteStorage> {
    let config = StorageConfig {
        url: "sqlite::memory:".to_string(),
        ..Default::default()
    };
    let s = SqliteStorage::new_sqlite(config, Chain::Test).await?;
    s.migrate_database().await?;
    Ok(s)
}

fn long_ago() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2024-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
}

async fn seed_user(s: &SqliteStorage) -> i64 {
    let now = long_ago();
    StorageReaderWriter::insert_user(
        s,
        &User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: IDENTITY.to_string(),
            active_storage: "default".to_string(),
        },
        None,
    )
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn seed_tx(
    s: &SqliteStorage,
    user_id: i64,
    reference: &str,
    status: TransactionStatus,
) -> i64 {
    let now = long_ago();
    StorageReaderWriter::insert_transaction(
        s,
        &Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status,
            reference: reference.to_string(),
            is_outgoing: true,
            satoshis: 1000,
            description: "tier-1 test".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: None,
            input_beef: None,
            raw_tx: None,
        },
        None,
    )
    .await
    .unwrap()
}

/// An output already consumed by `spent_by`, i.e. what a pending action holds.
async fn seed_consumed_output(s: &SqliteStorage, user_id: i64, spent_by: i64) -> i64 {
    seed_output(s, user_id, spent_by, 0, false, Some(spent_by)).await
}

async fn seed_output(
    s: &SqliteStorage,
    user_id: i64,
    transaction_id: i64,
    vout: i32,
    spendable: bool,
    spent_by: Option<i64>,
) -> i64 {
    let now = long_ago();
    StorageReaderWriter::insert_output(
        s,
        &Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id,
            transaction_id,
            basket_id: None,
            spendable,
            change: true,
            output_description: None,
            vout,
            satoshis: 5000,
            provided_by: StorageProvidedBy::Storage,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: None,
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: None,
            spent_by,
            sequence_number: None,
            spending_description: None,
            script_length: None,
            script_offset: None,
            locking_script: None,
        },
        None,
    )
    .await
    .unwrap()
}

async fn output_by_id(s: &SqliteStorage, output_id: i64) -> Output {
    StorageReader::find_outputs(
        s,
        &FindOutputsArgs {
            partial: OutputPartial {
                output_id: Some(output_id),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .into_iter()
    .next()
    .expect("output row")
}

async fn tx_by_id(s: &SqliteStorage, transaction_id: i64) -> Transaction {
    StorageReader::find_transactions(
        s,
        &FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(transaction_id),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .into_iter()
    .next()
    .expect("transaction row")
}

// ---------------------------------------------------------------------------
// purge_data: whole spent transactions are reclaimed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn purge_spent_retains_a_transaction_with_any_spendable_output() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let tx_id = seed_tx(&s, user_id, "ref-purge", TransactionStatus::Completed).await;
    let spent_output_id = seed_output(&s, user_id, tx_id, 0, false, Some(tx_id)).await;
    let spendable_output_id = seed_output(&s, user_id, tx_id, 1, true, None).await;

    let summary = StorageReaderWriter::purge_data(
        &s,
        &PurgeParams {
            purge_spent: true,
            purge_spent_age: 0,
            ..Default::default()
        },
        None,
    )
    .await
    .expect("purge_data should inspect complete transactions");

    assert!(
        summary.contains("purged 0 spent transactions"),
        "a transaction with spendable change must be retained: {summary}"
    );
    assert_eq!(
        output_by_id(&s, spent_output_id).await.output_id,
        spent_output_id
    );
    assert_eq!(
        output_by_id(&s, spendable_output_id).await.output_id,
        spendable_output_id
    );
    assert_eq!(tx_by_id(&s, tx_id).await.transaction_id, tx_id);
}

#[tokio::test]
async fn purge_spent_deletes_output_tags_before_the_spent_transaction() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let tx_id = seed_tx(
        &s,
        user_id,
        "ref-purge-tagged",
        TransactionStatus::Completed,
    )
    .await;
    let output_id = seed_consumed_output(&s, user_id, tx_id).await;
    let now = long_ago();
    let tag_id = StorageReaderWriter::insert_output_tag(
        &s,
        &OutputTag {
            created_at: now,
            updated_at: now,
            output_tag_id: 0,
            user_id,
            tag: "keep-delete-order".to_string(),
            is_deleted: false,
        },
        None,
    )
    .await
    .unwrap();
    StorageReaderWriter::insert_output_tag_map(
        &s,
        &OutputTagMap {
            created_at: now,
            updated_at: now,
            output_tag_id: tag_id,
            output_id,
            is_deleted: false,
        },
        None,
    )
    .await
    .unwrap();

    let summary = StorageReaderWriter::purge_data(
        &s,
        &PurgeParams {
            purge_spent: true,
            purge_spent_age: 0,
            ..Default::default()
        },
        None,
    )
    .await
    .expect("tag mappings must be deleted before their output");

    assert!(summary.contains("purged 1 spent transactions"));
    let transactions = StorageReader::find_transactions(
        &s,
        &FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(tx_id),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        transactions.is_empty(),
        "the fully spent transaction is purged"
    );
}

// ---------------------------------------------------------------------------
// abort_action: releases the inputs AND fails the row, atomically
// ---------------------------------------------------------------------------

/// The property TS guarantees and the Rust abort never asserted: after an
/// abort the consumed inputs are spendable again *and* the action is Failed.
#[tokio::test]
async fn abort_restores_inputs_and_fails_the_action() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let reference_bytes = b"tier1-abort".to_vec();
    let reference = base64_encode(&reference_bytes);
    let tx_id = seed_tx(&s, user_id, &reference, TransactionStatus::Unsigned).await;
    let source_tx_id = seed_tx(
        &s,
        user_id,
        "tier1-abort-source",
        TransactionStatus::Completed,
    )
    .await;
    let output_id = seed_output(&s, user_id, source_tx_id, 0, false, Some(tx_id)).await;

    let auth = AuthId {
        identity_key: IDENTITY.to_string(),
        user_id: Some(user_id),
        is_active: Some(true),
    };
    let result = WalletStorageProvider::abort_action(
        &s,
        &auth,
        &AbortActionArgs {
            reference: reference_bytes,
        },
    )
    .await
    .expect("abort should succeed");
    assert!(result.aborted);

    let output = output_by_id(&s, output_id).await;
    assert!(
        output.spendable,
        "aborting must return the coin to the pool"
    );
    assert!(
        output.spent_by.is_none(),
        "aborting must clear spentBy, leaving spentBy={:?}",
        output.spent_by
    );
    assert_eq!(
        tx_by_id(&s, tx_id).await.status,
        TransactionStatus::Failed,
        "the aborted action must not remain signable"
    );
}

/// The abort's writes honour an ambient transaction — rolling back leaves the
/// action exactly as it was. This is the mechanism the provider-level
/// `abort_action` now supplies (it opens a transaction rather than passing
/// `None`); a change that made these writes escape their transaction would
/// fail here.
#[tokio::test]
async fn abort_writes_are_confined_to_its_transaction() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let reference_bytes = b"tier1-rollback".to_vec();
    let reference = base64_encode(&reference_bytes);
    let tx_id = seed_tx(&s, user_id, &reference, TransactionStatus::Unsigned).await;
    let source_tx_id = seed_tx(
        &s,
        user_id,
        "tier1-rollback-source",
        TransactionStatus::Completed,
    )
    .await;
    let output_id = seed_output(&s, user_id, source_tx_id, 0, false, Some(tx_id)).await;

    let trx = StorageReaderWriter::begin_transaction(&s).await.unwrap();
    bsv_wallet_toolbox::storage::methods::abort_action::abort_action(
        &s,
        IDENTITY,
        &AbortActionArgs {
            reference: reference_bytes,
        },
        Some(&trx),
    )
    .await
    .expect("abort should succeed inside the transaction");
    StorageReaderWriter::rollback_transaction(&s, trx)
        .await
        .unwrap();

    let output = output_by_id(&s, output_id).await;
    assert!(
        !output.spendable && output.spent_by == Some(tx_id),
        "a rolled-back abort must not release the coin"
    );
    assert_eq!(
        tx_by_id(&s, tx_id).await.status,
        TransactionStatus::Unsigned,
        "a rolled-back abort must not fail the action"
    );
}

#[tokio::test]
async fn abort_accepts_a_txid_deserialized_from_the_typescript_wire_shape() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let txid = "22".repeat(32);
    let tx_id = seed_tx(
        &s,
        user_id,
        "stored-reference-is-not-the-txid",
        TransactionStatus::Unsigned,
    )
    .await;
    StorageReaderWriter::update_transaction(
        &s,
        tx_id,
        &TransactionPartial {
            txid: Some(txid.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();

    // This is the JSON shape sent by a TypeScript Base64String client. The SDK
    // decodes its 64 wire characters to 48 bytes before storage sees the args.
    let args: AbortActionArgs = serde_json::from_value(serde_json::json!({
        "reference": txid,
    }))
    .expect("the TypeScript wire value is valid base64");
    assert_eq!(args.reference.len(), 48);

    let auth = AuthId {
        identity_key: IDENTITY.to_string(),
        user_id: Some(user_id),
        is_active: Some(true),
    };
    let result = WalletStorageProvider::abort_action(&s, &auth, &args)
        .await
        .expect("the wire txid should fall back from reference to txid");

    assert!(result.aborted);
    assert_eq!(tx_by_id(&s, tx_id).await.status, TransactionStatus::Failed);
}

#[tokio::test]
async fn abort_prefers_a_64_character_reference_over_the_txid_fallback() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let identifier = "33".repeat(32);
    let reference_match = seed_tx(&s, user_id, &identifier, TransactionStatus::Unsigned).await;
    let txid_match = seed_tx(
        &s,
        user_id,
        "different-stored-reference",
        TransactionStatus::Unsigned,
    )
    .await;
    StorageReaderWriter::update_transaction(
        &s,
        txid_match,
        &TransactionPartial {
            txid: Some(identifier.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();

    let args: AbortActionArgs = serde_json::from_value(serde_json::json!({
        "reference": identifier,
    }))
    .expect("the TypeScript wire value is valid base64");
    let auth = AuthId {
        identity_key: IDENTITY.to_string(),
        user_id: Some(user_id),
        is_active: Some(true),
    };
    WalletStorageProvider::abort_action(&s, &auth, &args)
        .await
        .expect("the reference match should take precedence");

    assert_eq!(
        tx_by_id(&s, reference_match).await.status,
        TransactionStatus::Failed
    );
    assert_eq!(
        tx_by_id(&s, txid_match).await.status,
        TransactionStatus::Unsigned
    );
}

#[tokio::test]
async fn abort_signed_nosend_does_not_release_raw_input_claimed_by_another_action() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let source_txid = "11".repeat(32);
    let action_txid = "22".repeat(32);

    let source_tx_id = seed_tx(&s, user_id, "abort-source", TransactionStatus::Completed).await;
    let other_action_id = seed_tx(
        &s,
        user_id,
        "other-live-action",
        TransactionStatus::Unsigned,
    )
    .await;
    let source_output_id =
        seed_output(&s, user_id, source_tx_id, 0, false, Some(other_action_id)).await;
    StorageReaderWriter::update_output(
        &s,
        source_output_id,
        &OutputPartial {
            txid: Some(source_txid.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();

    let mut signed = bsv::transaction::transaction::Transaction::new();
    signed
        .inputs
        .push(bsv::transaction::transaction_input::TransactionInput {
            source_transaction: None,
            source_txid: Some(source_txid),
            source_output_index: 0,
            unlocking_script: None,
            sequence: u32::MAX,
        });
    signed
        .outputs
        .push(bsv::transaction::transaction_output::TransactionOutput {
            satoshis: Some(1000),
            ..Default::default()
        });
    let raw_tx = signed.to_bytes().unwrap();

    let action_tx_id = seed_tx(
        &s,
        user_id,
        "stored-reference-is-not-the-txid",
        TransactionStatus::Nosend,
    )
    .await;
    StorageReaderWriter::update_transaction(
        &s,
        action_tx_id,
        &TransactionPartial {
            txid: Some(action_txid.clone()),
            raw_tx: Some(raw_tx.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let created_output_id = seed_output(&s, user_id, action_tx_id, 0, true, None).await;

    let now = long_ago();
    let req_id = StorageReaderWriter::insert_proven_tx_req(
        &s,
        &ProvenTxReq {
            created_at: now,
            updated_at: now,
            proven_tx_req_id: 0,
            proven_tx_id: None,
            status: bsv_wallet_toolbox::status::ProvenTxReqStatus::Nosend,
            attempts: 0,
            notified: false,
            txid: action_txid.clone(),
            batch: None,
            history: "{}".to_string(),
            notify: "{}".to_string(),
            raw_tx,
            input_beef: None,
        },
        None,
    )
    .await
    .unwrap();

    let auth = AuthId {
        identity_key: IDENTITY.to_string(),
        user_id: Some(user_id),
        is_active: Some(true),
    };
    let result = WalletStorageProvider::abort_action(
        &s,
        &auth,
        &AbortActionArgs {
            reference: action_txid.as_bytes().to_vec(),
        },
    )
    .await
    .expect("a 64-character txid identifies an abortable action");
    assert!(result.aborted);

    let source = output_by_id(&s, source_output_id).await;
    assert!(
        !source.spendable && source.spent_by == Some(other_action_id),
        "aborting action A must not release a rawTx input now claimed by action B"
    );
    let created = output_by_id(&s, created_output_id).await;
    assert!(
        !created.spendable && created.spent_by.is_none(),
        "outputs created by the aborted transaction are retired"
    );
    let req = StorageReader::find_proven_tx_reqs(
        &s,
        &FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                proven_tx_req_id: Some(req_id),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    assert_eq!(
        req.status,
        bsv_wallet_toolbox::status::ProvenTxReqStatus::Invalid
    );
    assert!(req.history.contains("abortAction"));
    assert_eq!(
        tx_by_id(&s, action_tx_id).await.status,
        TransactionStatus::Failed
    );
}

#[tokio::test]
async fn find_outputs_applies_supported_metadata_filters_and_omits_scripts() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let tx_id = seed_tx(
        &s,
        user_id,
        "find-output-metadata",
        TransactionStatus::Completed,
    )
    .await;
    let output_id = seed_output(&s, user_id, tx_id, 0, true, None).await;
    StorageReaderWriter::update_output(
        &s,
        output_id,
        &OutputPartial {
            output_description: Some("the output".to_string()),
            spending_description: Some("the spend".to_string()),
            custom_instructions: Some("custom".to_string()),
            script_length: Some(25),
            script_offset: Some(42),
            locking_script: Some(vec![0x51, 0x21]),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();

    macro_rules! assert_filter {
        ($field:ident, $matching:expr, $missing:expr) => {{
            let matched = StorageReader::find_outputs(
                &s,
                &FindOutputsArgs {
                    partial: OutputPartial {
                        $field: Some($matching),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                matched.len(),
                1,
                "{} matching predicate",
                stringify!($field)
            );
            assert_eq!(matched[0].output_id, output_id);

            let absent = StorageReader::find_outputs(
                &s,
                &FindOutputsArgs {
                    partial: OutputPartial {
                        $field: Some($missing),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
            assert!(
                absent.is_empty(),
                "{} must constrain the query",
                stringify!($field)
            );
        }};
    }
    assert_filter!(
        output_description,
        "the output".to_string(),
        "other output".to_string()
    );
    assert_filter!(
        spending_description,
        "the spend".to_string(),
        "other spend".to_string()
    );
    assert_filter!(
        custom_instructions,
        "custom".to_string(),
        "other".to_string()
    );
    assert_filter!(script_length, 25, 26);
    assert_filter!(script_offset, 42, 43);

    let without_script = StorageReader::find_outputs(
        &s,
        &FindOutputsArgs {
            partial: OutputPartial {
                output_id: Some(output_id),
                ..Default::default()
            },
            no_script: true,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(without_script.len(), 1);
    assert!(without_script[0].locking_script.is_none());

    let err = StorageReader::find_outputs(
        &s,
        &FindOutputsArgs {
            partial: OutputPartial {
                locking_script: Some(vec![0x51, 0x21]),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("args.partial.lockingScript"));
    assert!(message.contains("undefined"));
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test]
async fn abort_signed_nosend_recovers_raw_inputs_and_invalidates_its_request() {
    let s = storage().await.unwrap();
    let user_id = seed_user(&s).await;
    let source_txid = "11".repeat(32);
    let action_txid = "22".repeat(32);

    let source_tx_id = seed_tx(&s, user_id, "abort-source", TransactionStatus::Completed).await;
    let source_output_id = seed_output(&s, user_id, source_tx_id, 0, false, None).await;
    StorageReaderWriter::update_output(
        &s,
        source_output_id,
        &OutputPartial {
            txid: Some(source_txid.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();

    let mut signed = bsv::transaction::transaction::Transaction::new();
    signed
        .inputs
        .push(bsv::transaction::transaction_input::TransactionInput {
            source_transaction: None,
            source_txid: Some(source_txid),
            source_output_index: 0,
            unlocking_script: None,
            sequence: u32::MAX,
        });
    signed
        .outputs
        .push(bsv::transaction::transaction_output::TransactionOutput {
            satoshis: Some(1000),
            ..Default::default()
        });
    let raw_tx = signed.to_bytes().unwrap();

    let action_tx_id = seed_tx(
        &s,
        user_id,
        "stored-reference-is-not-the-txid",
        TransactionStatus::Nosend,
    )
    .await;
    StorageReaderWriter::update_transaction(
        &s,
        action_tx_id,
        &TransactionPartial {
            txid: Some(action_txid.clone()),
            raw_tx: Some(raw_tx.clone()),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let created_output_id = seed_output(&s, user_id, action_tx_id, 0, true, None).await;

    let now = long_ago();
    let req_id = StorageReaderWriter::insert_proven_tx_req(
        &s,
        &ProvenTxReq {
            created_at: now,
            updated_at: now,
            proven_tx_req_id: 0,
            proven_tx_id: None,
            status: bsv_wallet_toolbox::status::ProvenTxReqStatus::Nosend,
            attempts: 0,
            notified: false,
            txid: action_txid.clone(),
            batch: None,
            history: "{}".to_string(),
            notify: "{}".to_string(),
            raw_tx,
            input_beef: None,
        },
        None,
    )
    .await
    .unwrap();

    let auth = AuthId {
        identity_key: IDENTITY.to_string(),
        user_id: Some(user_id),
        is_active: Some(true),
    };
    let result = WalletStorageProvider::abort_action(
        &s,
        &auth,
        &AbortActionArgs {
            reference: action_txid.as_bytes().to_vec(),
        },
    )
    .await
    .expect("a 64-character txid identifies an abortable action");
    assert!(result.aborted);

    let source = output_by_id(&s, source_output_id).await;
    assert!(
        source.spendable && source.spent_by.is_none(),
        "rawTx recovers a wallet input even when spentBy was not persisted"
    );
    let created = output_by_id(&s, created_output_id).await;
    assert!(
        !created.spendable && created.spent_by.is_none(),
        "outputs created by the aborted transaction are retired"
    );
    let req = StorageReader::find_proven_tx_reqs(
        &s,
        &FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                proven_tx_req_id: Some(req_id),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    assert_eq!(
        req.status,
        bsv_wallet_toolbox::status::ProvenTxReqStatus::Invalid
    );
    assert!(req.history.contains("abortAction"));
    assert_eq!(
        tx_by_id(&s, action_tx_id).await.status,
        TransactionStatus::Failed
    );
}
