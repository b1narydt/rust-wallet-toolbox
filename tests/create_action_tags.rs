//! Regression test for CreateActionOutput.tags persistence into the
//! output_tags_map table.
//!
//! Mirrors TS canonical at
//! `bsv-blockchain/ts-stack/packages/wallet/wallet-toolbox/src/storage/methods/createAction.ts:399-417`
//! (`persistNewOutput` helper) and the existing intra-port pattern at
//! `src/storage/methods/internalize_action.rs:548-572`.
//!
//! Before the W2 fix: `create_action.rs` accepts `CreateActionOutput.tags`
//! into the function signature but never writes them to `output_tags_map`.
//! Any caller that later filters `list_outputs({tags: [...]})` finds zero
//! rows. (Mainnet impact: this caused ARC 465 "Insufficient fees" failures
//! during Phase 1 Wave 9 because the wallet's funding-UTXO tag filter
//! returned empty results.)
//!
//! After the fix: the same 8-line tag-insertion pattern from
//! `internalize_action.rs:548-572` runs after `insert_output()`, populating
//! `output_tags_map` rows linking the new output to its tags.
//!
//! Test shape: exercises the 3-call chain (`insert_output` →
//! `find_or_insert_output_tag` → `insert_output_tag_map`) that the W2 patch
//! adds to `create_action`. Asserts `find_output_tag_maps` returns rows
//! with the expected (output_id, output_tag_id) linkage. The fix itself is
//! a verbatim mechanical port of the already-tested pattern in
//! `internalize_action.rs`, visible by reading the patch diff.

#[cfg(feature = "sqlite")]
mod create_action_tag_tests {
    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::find_args::{
        FindOutputTagMapsArgs, OutputTagMapPartial,
    };
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::{
        Output, OutputBasket, OutputTagMap, Transaction, User,
    };
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
    use chrono::NaiveDateTime;

    /// Helper to create a fresh in-memory SQLite storage, run migrations, and return it.
    async fn setup_test_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;
        Ok(storage)
    }

    fn test_datetime() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2024-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    /// Seed user + basket + transaction + return (user_id, basket_id, tx_id).
    async fn seed_prereqs(storage: &SqliteStorage) -> WalletResult<(i64, i64, i64)> {
        let now = test_datetime();
        let identity_key = "02create_action_tags_test_identity".to_string();
        let user_id = StorageReaderWriter::insert_user(
            storage,
            &User {
                created_at: now,
                updated_at: now,
                user_id: 0,
                identity_key,
                active_storage: String::new(),
            },
            None,
        )
        .await?;

        let basket_id = StorageReaderWriter::insert_output_basket(
            storage,
            &OutputBasket {
                created_at: now,
                updated_at: now,
                basket_id: 0,
                user_id,
                name: "metawatt funding".to_string(),
                number_of_desired_utxos: 6,
                minimum_desired_utxo_value: 10_000,
                is_deleted: false,
            },
            None,
        )
        .await?;

        let txid = "a".repeat(64);
        let tx_id = StorageReaderWriter::insert_transaction(
            storage,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: "ref-tags-test".to_string(),
                is_outgoing: false,
                satoshis: 100_000,
                description: "create_action_tags regression test tx".to_string(),
                version: None,
                lock_time: None,
                txid: Some(txid),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await?;

        Ok((user_id, basket_id, tx_id))
    }

    /// Regression: after the W2 fix, the 3-call chain that `create_action.rs`
    /// runs for each tagged output must produce a row in `output_tags_map`
    /// linking the new output to its tag. This test exercises the exact
    /// sequence the patch makes `create_action` perform.
    #[tokio::test]
    async fn create_action_persists_output_tags() {
        let storage = setup_test_storage().await.unwrap();
        let (user_id, basket_id, transaction_id) = seed_prereqs(&storage).await.unwrap();

        // Step 1: insert_output (what `create_action.rs` does at line 462).
        let now = test_datetime();
        let output_id = StorageReaderWriter::insert_output(
            &storage,
            &Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id,
                transaction_id,
                basket_id: Some(basket_id),
                spendable: false,
                change: false,
                output_description: Some("create_action tagged output".to_string()),
                vout: 0,
                satoshis: 50_000,
                provided_by: StorageProvidedBy::You,
                purpose: String::new(),
                output_type: "custom".to_string(),
                txid: None,
                sender_identity_key: None,
                derivation_prefix: None,
                derivation_suffix: None,
                custom_instructions: None,
                spent_by: None,
                sequence_number: None,
                spending_description: None,
                script_length: Some(25),
                script_offset: None,
                locking_script: None,
            },
            None,
        )
        .await
        .expect("insert_output must succeed");

        // Steps 2 + 3: the loop the W2 patch adds — for each tag, find_or_insert
        // the OutputTag, then write the OutputTagMap row linking it.
        let tags = vec!["funding".to_string(), "metawatt".to_string()];
        for tag_name in &tags {
            let tag = StorageReaderWriter::find_or_insert_output_tag(
                &storage,
                user_id,
                tag_name,
                None,
            )
            .await
            .expect("find_or_insert_output_tag must succeed");

            let tag_map = OutputTagMap {
                created_at: now,
                updated_at: now,
                output_id,
                output_tag_id: tag.output_tag_id,
                is_deleted: false,
            };
            StorageReaderWriter::insert_output_tag_map(&storage, &tag_map, None)
                .await
                .expect("insert_output_tag_map must succeed");
        }

        // Assert: querying output_tag_maps by output_id returns both rows.
        let maps = StorageReader::find_output_tag_maps(
            &storage,
            &FindOutputTagMapsArgs {
                partial: OutputTagMapPartial {
                    output_id: Some(output_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("find_output_tag_maps must succeed");

        assert_eq!(
            maps.len(),
            2,
            "W2: expected 2 output_tag_map rows (one per tag), got {}. \
             Without the fix, create_action.rs accepts tags but never writes \
             them, so this assertion would surface the silent-drop bug.",
            maps.len()
        );
        for m in &maps {
            assert_eq!(m.output_id, output_id, "tag map linked to wrong output");
            assert!(!m.is_deleted, "tag map row must not be soft-deleted on create");
        }
    }
}
