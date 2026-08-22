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
    let output_id = seed_consumed_output(&s, user_id, tx_id).await;

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
    let output_id = seed_consumed_output(&s, user_id, tx_id).await;

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

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
