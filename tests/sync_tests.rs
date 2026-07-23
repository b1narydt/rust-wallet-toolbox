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
                offsets: Default::default(),
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
                offsets: Default::default(),
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
                offsets: Default::default(),
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
        assert!(
            result.inserts > 0,
            "should have inserted at least one entity"
        );

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
        assert_eq!(
            target_txs.len(),
            1,
            "transaction should be synced to target"
        );
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
        let _local_basket_id = target.insert_output_basket(&basket, None).await.unwrap();

        // Build a SyncChunk with a newer version of the same basket
        let incoming_basket = OutputBasket {
            created_at: old_time,
            updated_at: new_time, // newer
            basket_id: 99,        // foreign ID
            user_id: 77,          // foreign user_id
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
        assert!(
            baskets[0].is_deleted,
            "basket should be marked as deleted after merge"
        );

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
            user_id: 77,         // foreign user_id
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
            output_id: 200,      // foreign ID
            user_id: 77,         // foreign user_id
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
        let local_tx_id = sync_map
            .transaction
            .get_local_id(100)
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

        assert_eq!(
            local_outputs.len(),
            1,
            "output should exist with remapped transaction_id"
        );
        assert_eq!(
            local_outputs[0].transaction_id, local_tx_id,
            "output's transaction_id should point to the local transaction"
        );
        assert_eq!(local_outputs[0].satoshis, 4000);

        // Verify output ID mapping
        let local_output_id = sync_map.output.get_local_id(200);
        assert!(
            local_output_id.is_some(),
            "foreign output ID 200 should be mapped"
        );
    }

    // -----------------------------------------------------------------------
    // Builders shared by the parity regression tests below.
    // -----------------------------------------------------------------------

    fn foreign_tx(
        foreign_id: i64,
        reference: &str,
        txid: &str,
        updated: NaiveDateTime,
    ) -> Transaction {
        Transaction {
            created_at: updated,
            updated_at: updated,
            transaction_id: foreign_id,
            user_id: 77, // foreign user_id — remapped to the local user on merge
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: reference.to_string(),
            is_outgoing: true,
            satoshis: 1000,
            description: "sync tx".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(txid.to_string()),
            input_beef: None,
            raw_tx: None,
        }
    }

    fn foreign_output(
        foreign_id: i64,
        tx_foreign_id: i64,
        vout: i32,
        txid: &str,
        updated: NaiveDateTime,
    ) -> Output {
        Output {
            created_at: updated,
            updated_at: updated,
            output_id: foreign_id,
            user_id: 77, // foreign user_id
            transaction_id: tx_foreign_id,
            basket_id: None,
            spendable: true,
            change: false,
            output_description: Some("o".to_string()),
            vout,
            satoshis: 500,
            provided_by: StorageProvidedBy::You,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: Some(txid.to_string()),
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
        }
    }

    fn chunk_with(
        user_key: &str,
        transactions: Option<Vec<Transaction>>,
        outputs: Option<Vec<Output>>,
    ) -> SyncChunk {
        SyncChunk {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: user_key.to_string(),
            user: None,
            proven_txs: None,
            output_baskets: None,
            transactions,
            outputs,
            tx_labels: None,
            tx_label_maps: None,
            output_tags: None,
            output_tag_maps: None,
            certificates: None,
            certificate_fields: None,
            commissions: None,
            proven_tx_reqs: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 6 (#9): an unmapped REQUIRED foreign key fails loudly instead of
    // silently falling back to the raw foreign id.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_process_sync_chunk_unmapped_required_fk_errors() {
        let target = setup_storage().await.unwrap();
        let local_uid = insert_test_user(&target, "02abc666").await;
        let now = dt("2024-01-15 12:00:00");

        // A DECOY local transaction. As the first insert it takes local id 1,
        // which is exactly the foreign id the orphaned output will reference.
        // The raw-foreign-id fallback would silently attach the output HERE — a
        // wrong, unrelated row — without any FK constraint failure. The strict
        // remap must fail instead.
        let mut decoy = foreign_tx(1, "decoy-ref", "decoy_txid_001", now);
        decoy.transaction_id = 0; // auto-increment -> local id 1
        decoy.user_id = local_uid;
        let decoy_local_id = StorageReaderWriter::insert_transaction(&target, &decoy, None)
            .await
            .unwrap();
        assert_eq!(
            decoy_local_id, 1,
            "decoy must occupy local id 1 for this test"
        );

        // An output referencing transaction_id=1 with NO transaction in the chunk
        // and an empty idMap — the required FK is unmapped.
        let orphan = foreign_output(200, 1, 0, "orphan_txid_001", now);
        let chunk = chunk_with("02abc666", None, Some(vec![orphan]));

        let mut sync_map = SyncMap::new();
        let result = process_sync_chunk(&target, chunk, &mut sync_map, None).await;

        assert!(
            result.is_err(),
            "unmapped required FK must fail loudly (TS: undefined FK -> insert throws), \
             not silently attach the output to the unrelated local row at the raw foreign id"
        );

        // The decoy transaction must NOT have had a foreign output mis-attached to it.
        let leaked = StorageReader::find_outputs(
            &target,
            &FindOutputsArgs {
                partial: OutputPartial {
                    transaction_id: Some(decoy_local_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert!(
            leaked.is_empty(),
            "no output must be mis-attached to the unrelated decoy transaction"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7 (#8): a parent/child pair split across two chunks resolves the
    // child FK to the correct LOCAL id via the persisted (reloaded) idMap.
    // Exercised through the WalletStorageProvider writer path, which loads and
    // persists the SyncMap in the sync_states row.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cross_chunk_fk_uses_persisted_idmap() {
        use bsv_wallet_toolbox::storage::sync::request_args::RequestSyncChunkArgs;
        use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
        use bsv_wallet_toolbox::wallet::types::AuthId;

        let target = setup_storage().await.unwrap();
        let _uid = insert_test_user(&target, "02abc777").await;
        let now = dt("2024-01-15 12:00:00");

        // Create the sync_state row so the writer path can persist/reload its map.
        let auth = AuthId {
            identity_key: "02abc777".to_string(),
            user_id: None,
            is_active: None,
        };
        let _ = WalletStorageProvider::find_or_insert_sync_state_auth(
            &target,
            &auth,
            "storage-a", // reader identity key — must match args.from_storage_identity_key
            "reader-store",
        )
        .await
        .unwrap();

        let args = RequestSyncChunkArgs {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            identity_key: "02abc777".to_string(),
            since: None,
            max_rough_size: 10_000_000,
            max_items: 1000,
            offsets: vec![],
        };

        // Chunk 1: only the parent transaction (foreign id 100).
        let chunk1 = chunk_with(
            "02abc777",
            Some(vec![foreign_tx(100, "ref-xchunk", "xchunk_txid_001", now)]),
            None,
        );
        WalletStorageProvider::process_sync_chunk(&target, &args, &chunk1)
            .await
            .unwrap();

        // Resolve the LOCAL transaction id assigned in chunk 1.
        let local_tx = StorageReader::find_transactions(
            &target,
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    reference: Some("ref-xchunk".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(local_tx.len(), 1, "parent tx should be synced in chunk 1");
        let local_tx_id = local_tx[0].transaction_id;
        assert_ne!(
            local_tx_id, 100,
            "local id must differ from the foreign id for the test to be meaningful"
        );

        // Chunk 2: the child output, referencing the foreign transaction id 100.
        let chunk2 = chunk_with(
            "02abc777",
            None,
            Some(vec![foreign_output(200, 100, 0, "xchunk_txid_001", now)]),
        );
        WalletStorageProvider::process_sync_chunk(&target, &args, &chunk2)
            .await
            .expect("child output must resolve its FK via the persisted idMap");

        // The output must be attached to the LOCAL transaction id, not the foreign 100.
        let outs = StorageReader::find_outputs(
            &target,
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
        assert_eq!(
            outs.len(),
            1,
            "child output must attach to the LOCAL transaction id across chunks"
        );
        assert_eq!(outs[0].transaction_id, local_tx_id);

        // And nothing leaked in against the raw foreign id.
        let leaked = StorageReader::find_outputs(
            &target,
            &FindOutputsArgs {
                partial: OutputPartial {
                    transaction_id: Some(100),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert!(
            leaked.is_empty(),
            "no output should point at the foreign id"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8 (#10): a multi-chunk sync round (dataset larger than max_items)
    // must not skip rows. Drives the real reader/writer loop with a small
    // max_items so the window/offset bookkeeping is exercised end to end.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_multi_chunk_round_does_not_skip_rows() {
        use bsv_wallet_toolbox::storage::manager::make_request_sync_chunk_args;
        use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
        use bsv_wallet_toolbox::wallet::types::AuthId;

        let source = setup_storage().await.unwrap();
        let target = setup_storage().await.unwrap();
        let key = "02abc888";
        let _src_uid = insert_test_user(&source, key).await;
        let _tgt_uid = insert_test_user(&target, key).await;

        // Insert N transactions into the source, N > max_items so the round spans
        // multiple chunks. Distinct updated_at values so the watermark is well-defined.
        let n = 5;
        for i in 0..n {
            let updated = dt(&format!("2024-01-15 12:0{i}:00"));
            let tx = foreign_tx(
                1000 + i as i64,
                &format!("ref-multi-{i}"),
                &format!("multi_txid_{i}"),
                updated,
            );
            let mut tx = tx;
            tx.user_id = _src_uid;
            StorageReaderWriter::insert_transaction(&source, &tx, None)
                .await
                .unwrap();
        }

        // Create the sync_state row on the target for the source storage.
        let auth = AuthId {
            identity_key: key.to_string(),
            user_id: None,
            is_active: None,
        };
        let _ = WalletStorageProvider::find_or_insert_sync_state_auth(
            &target,
            &auth,
            "src-sik",
            "src-store",
        )
        .await
        .unwrap();

        // Drive the real sync loop with a small max_items, re-reading the
        // persisted sync_state each iteration (as WalletStorageManager does).
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(
                guard < 50,
                "sync loop failed to terminate — window not advancing"
            );

            let (ss, _) = WalletStorageProvider::find_or_insert_sync_state_auth(
                &target,
                &auth,
                "src-sik",
                "src-store",
            )
            .await
            .unwrap();
            let mut args = make_request_sync_chunk_args(&ss, key, "tgt-sik").unwrap();
            args.max_items = 2; // force multiple chunks per round

            let chunk = WalletStorageProvider::get_sync_chunk(&source, &args)
                .await
                .unwrap();
            let r = WalletStorageProvider::process_sync_chunk(&target, &args, &chunk)
                .await
                .unwrap();
            if r.done {
                break;
            }
        }

        // Every source transaction must be present in the target — no rows skipped.
        for i in 0..n {
            let found = StorageReader::find_transactions(
                &target,
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        reference: Some(format!("ref-multi-{i}")),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
            assert_eq!(
                found.len(),
                1,
                "transaction ref-multi-{i} must be synced — the multi-chunk round skipped it"
            );
        }
    }
}
