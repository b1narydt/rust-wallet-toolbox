//! Sync protocol integration tests.
//!
//! Tests verify getSyncChunk and processSyncChunk behavior including
//! incremental entity retrieval, merge logic, and foreign key remapping.
//!
//! All tests use in-memory SQLite databases and are gated with `#[cfg(feature = "sqlite")]`.

#[cfg(feature = "sqlite")]
mod sync_tests {
    use chrono::NaiveDateTime;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::sync::get_sync_chunk::{get_sync_chunk, GetSyncChunkArgs};
    use bsv_wallet_toolbox::storage::sync::process_sync_chunk::process_sync_chunk;
    use bsv_wallet_toolbox::storage::sync::sync_map::{SyncChunk, SyncMap};
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

    /// Helper to create a fresh in-memory SQLite storage with migrations.
    async fn setup_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;
        storage.make_available().await?;
        Ok(storage)
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    /// Insert a user and return the local user_id.
    async fn insert_test_user(storage: &SqliteStorage, identity_key: &str) -> i64 {
        let now = dt("2024-01-15 10:00:00");
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: identity_key.to_string(),
            active_storage: "default".to_string(),
        };
        storage.insert_user(&user, None).await.unwrap()
    }

    // -----------------------------------------------------------------------
    // Test 1: getSyncChunk from empty storage returns empty chunk
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_sync_chunk_empty_storage() {
        let storage = setup_storage().await.unwrap();
        let _user_id = insert_test_user(&storage, "02abc111").await;

        let sync_map = SyncMap::new();
        let chunk = get_sync_chunk(
            &storage,
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: "02abc111".to_string(),
                sync_map: &sync_map,
                max_items_per_entity: 1000,
            },
            None,
        )
        .await
        .unwrap();

        assert!(chunk.proven_txs.is_none(), "no proven_txs expected");
        assert!(chunk.transactions.is_none(), "no transactions expected");
        assert!(chunk.outputs.is_none(), "no outputs expected");
        assert!(chunk.output_baskets.is_none(), "no baskets expected");
        assert!(chunk.tx_labels.is_none(), "no tx_labels expected");
        assert!(chunk.certificates.is_none(), "no certificates expected");
    }

    // -----------------------------------------------------------------------
    // Test 2: Insert entities then getSyncChunk returns them
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_sync_chunk_returns_inserted_entities() {
        let storage = setup_storage().await.unwrap();
        let user_id = insert_test_user(&storage, "02abc222").await;
        let now = dt("2024-01-15 11:00:00");

        // Insert a transaction
        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-sync-test".to_string(),
            is_outgoing: true,
            satoshis: 5000,
            description: "sync test tx".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("deadbeef0001".to_string()),
            input_beef: None,
            raw_tx: None,
        };
        let _tx_id = storage.insert_transaction(&tx, None).await.unwrap();

        // Insert an output basket
        let basket = OutputBasket {
            created_at: now,
            updated_at: now,
            basket_id: 0,
            user_id,
            name: "test-basket".to_string(),
            number_of_desired_utxos: 10,
            minimum_desired_utxo_value: 1000,
            is_deleted: false,
        };
        let _basket_id = storage.insert_output_basket(&basket, None).await.unwrap();

        let sync_map = SyncMap::new();
        let chunk = get_sync_chunk(
            &storage,
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: "02abc222".to_string(),
                sync_map: &sync_map,
                max_items_per_entity: 1000,
            },
            None,
        )
        .await
        .unwrap();

        let txs = chunk.transactions.expect("should have transactions");
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].reference, "ref-sync-test");

        let baskets = chunk.output_baskets.expect("should have baskets");
        assert_eq!(baskets.len(), 1);
        assert_eq!(baskets[0].name, "test-basket");
    }

    // -----------------------------------------------------------------------
    // Test 3: processSyncChunk inserts new entities into a second storage
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_process_sync_chunk_inserts_new_entities() {
        let source = setup_storage().await.unwrap();
        let target = setup_storage().await.unwrap();
        let source_user_id = insert_test_user(&source, "02abc333").await;
        let _target_user_id = insert_test_user(&target, "02abc333").await;
        let now = dt("2024-01-15 12:00:00");

        // Insert a transaction into source
        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id: source_user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-process-test".to_string(),
            is_outgoing: false,
            satoshis: 3000,
            description: "process test".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("cafebabe0001".to_string()),
            input_beef: None,
            raw_tx: None,
        };
        let _tx_id = source.insert_transaction(&tx, None).await.unwrap();

        // Get sync chunk from source
        let sync_map = SyncMap::new();
        let chunk = get_sync_chunk(
            &source,
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: "02abc333".to_string(),
                sync_map: &sync_map,
                max_items_per_entity: 1000,
            },
            None,
        )
        .await
        .unwrap();

        // Process chunk into target
        let mut target_sync_map = SyncMap::new();
        let result = process_sync_chunk(&target, chunk, &mut target_sync_map, None)
            .await
            .unwrap();
        assert!(result.inserts > 0, "should have inserted at least one entity");

        // Verify transaction exists in target
        let target_txs = target
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        reference: Some("ref-process-test".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(target_txs.len(), 1, "transaction should be synced to target");
        assert_eq!(target_txs[0].reference, "ref-process-test");
    }

    // -----------------------------------------------------------------------
    // Test 4: processSyncChunk with existing entity merges correctly (newer wins)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_process_sync_chunk_merge_newer_wins() {
        let target = setup_storage().await.unwrap();
        let target_user_id = insert_test_user(&target, "02abc444").await;

        let old_time = dt("2024-01-15 10:00:00");
        let new_time = dt("2024-01-15 14:00:00");

        // Insert an old basket into target
        let basket = OutputBasket {
            created_at: old_time,
            updated_at: old_time,
            basket_id: 0,
            user_id: target_user_id,
            name: "merge-basket".to_string(),
            number_of_desired_utxos: 5,
            minimum_desired_utxo_value: 500,
            is_deleted: false,
        };
        let _local_basket_id = target
            .insert_output_basket(&basket, None)
            .await
            .unwrap();

        // Build a SyncChunk with a newer version of the same basket
        let incoming_basket = OutputBasket {
            created_at: old_time,
            updated_at: new_time, // newer
            basket_id: 99, // foreign ID
            user_id: 77,   // foreign user_id
            name: "merge-basket".to_string(),
            number_of_desired_utxos: 20,
            minimum_desired_utxo_value: 2000,
            is_deleted: true, // changed to deleted
        };

        let chunk = SyncChunk {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: "02abc444".to_string(),
            user: None,
            proven_txs: None,
            output_baskets: Some(vec![incoming_basket]),
            transactions: None,
            outputs: None,
            tx_labels: None,
            tx_label_maps: None,
            output_tags: None,
            output_tag_maps: None,
            certificates: None,
            certificate_fields: None,
            commissions: None,
            proven_tx_reqs: None,
        };

        let mut sync_map = SyncMap::new();
        let result = process_sync_chunk(&target, chunk, &mut sync_map, None)
            .await
            .unwrap();

        // Should have updated the existing basket
        assert!(result.updates > 0, "should have updated the basket");

        // Verify basket was updated (is_deleted = true)
        let baskets = target
            .find_output_baskets(
                &FindOutputBasketsArgs {
                    partial: OutputBasketPartial {
                        user_id: Some(target_user_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(baskets.len(), 1);
        assert!(baskets[0].is_deleted, "basket should be marked as deleted after merge");

        // Verify ID mapping
        let local_id = sync_map.output_basket.get_local_id(99);
        assert!(local_id.is_some(), "foreign basket ID 99 should be mapped");
    }

    // -----------------------------------------------------------------------
    // Test 5: Foreign key remapping -- Output references Transaction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_process_sync_chunk_fk_remapping() {
        let target = setup_storage().await.unwrap();
        let _target_user_id = insert_test_user(&target, "02abc555").await;

        let now = dt("2024-01-15 12:00:00");

        // Build a chunk with Transaction (foreign_id=100) and Output referencing it
        let foreign_tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 100, // foreign ID
            user_id: 77,        // foreign user_id
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-fk-remap".to_string(),
            is_outgoing: true,
            satoshis: 8000,
            description: "fk remap test".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("fk_remap_txid_001".to_string()),
            input_beef: None,
            raw_tx: None,
        };

        let foreign_output = Output {
            created_at: now,
            updated_at: now,
            output_id: 200, // foreign ID
            user_id: 77,    // foreign user_id
            transaction_id: 100, // references foreign tx ID
            basket_id: None,
            spendable: true,
            change: false,
            output_description: Some("test output".to_string()),
            vout: 0,
            satoshis: 4000,
            provided_by: StorageProvidedBy::You,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: Some("fk_remap_txid_001".to_string()),
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: None,
            script_offset: None,
            locking_script: None,
        };

        let chunk = SyncChunk {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: "02abc555".to_string(),
            user: None,
            proven_txs: None,
            output_baskets: None,
            transactions: Some(vec![foreign_tx]),
            outputs: Some(vec![foreign_output]),
            tx_labels: None,
            tx_label_maps: None,
            output_tags: None,
            output_tag_maps: None,
            certificates: None,
            certificate_fields: None,
            commissions: None,
            proven_tx_reqs: None,
        };

        let mut sync_map = SyncMap::new();
        let result = process_sync_chunk(&target, chunk, &mut sync_map, None)
            .await
            .unwrap();

        assert_eq!(result.inserts, 2, "should insert transaction + output");

        // Get the local transaction ID that was assigned
        let local_tx_id = sync_map.transaction.get_local_id(100)
            .expect("foreign tx ID 100 should be mapped to local ID");

        // Get the local output and verify its transaction_id was remapped
        let local_outputs = target
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        transaction_id: Some(local_tx_id),
                        vout: Some(0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(local_outputs.len(), 1, "output should exist with remapped transaction_id");
        assert_eq!(
            local_outputs[0].transaction_id, local_tx_id,
            "output's transaction_id should point to the local transaction"
        );
        assert_eq!(local_outputs[0].satoshis, 4000);

        // Verify output ID mapping
        let local_output_id = sync_map.output.get_local_id(200);
        assert!(local_output_id.is_some(), "foreign output ID 200 should be mapped");
    }
}
