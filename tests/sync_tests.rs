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
                max_items: 1000,
                max_rough_size: 10_000_000,
                offsets: Default::default(),
            },
            None,
        )
        .await
        .unwrap();

        // PRESENT AND EMPTY, not absent. BRC-40 draws the distinction and the
        // conformance corpus states it outright: "completion requires all 12
        // entity arrays present AND empty", while an omitted property means
        // "no attempt to update it" (WalletStorage.interfaces.ts:542).
        //
        // This test previously asserted `is_none()`, which pinned the opposite
        // convention: the producer collapsed empty to `None` and the consumer
        // read absent as done, so the pair agreed with each other and disagreed
        // with the protocol. That combination made a truncated or erroring
        // remote look like a completed round — `since` advanced past rows never
        // received and every offset reset. Both sides were corrected together;
        // this assertion is the producer half.
        macro_rules! present_and_empty {
            ($field:ident) => {
                let rows = chunk.$field.as_ref().unwrap_or_else(|| {
                    panic!(
                        "{} was queried and must be PRESENT, not absent",
                        stringify!($field)
                    )
                });
                assert!(
                    rows.is_empty(),
                    "{} must be empty on an empty storage",
                    stringify!($field)
                );
            };
        }
        present_and_empty!(proven_txs);
        present_and_empty!(transactions);
        present_and_empty!(outputs);
        present_and_empty!(output_baskets);
        present_and_empty!(tx_labels);
        present_and_empty!(certificates);
    }

    #[tokio::test]
    async fn test_get_sync_chunk_rejects_unknown_identity() {
        let storage = setup_storage().await.unwrap();
        let identity = "02missing-sync-identity";
        let sync_map = SyncMap::new();

        let err = get_sync_chunk(
            &storage,
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: identity.to_string(),
                sync_map: &sync_map,
                max_items: 1000,
                max_rough_size: 10_000_000,
                offsets: Default::default(),
            },
            None,
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("identityKey"));
        assert!(message.contains(identity));
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
                max_items: 1000,
                max_rough_size: 10_000_000,
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
                max_items: 1000,
                max_rough_size: 10_000_000,
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

    // -----------------------------------------------------------------------
    // A round that changes nothing is still a round that ADVANCED
    // -----------------------------------------------------------------------

    /// `inserts == 0 && updates == 0` does not mean the sync is stuck.
    ///
    /// `process_sync_chunk` counts every row it RECEIVES, before deciding
    /// whether to insert, update or skip it, and those counts become the next
    /// request's pagination offsets. So a backup that is already current skips
    /// every row while still marching through the table — the next request is
    /// a different question, and the answer will eventually be `done`.
    ///
    /// `WalletStorageManager`'s no-progress guard keys on the request rather
    /// than on these counters precisely because of this. Keying it on
    /// `inserts`/`updates` alone made re-attaching an up-to-date backup look
    /// like a stall, and since `WalletBuilder::build()` propagates that error,
    /// the wallet failed to boot on healthy data.
    ///
    /// The mutation this must catch: moving `count += 1` below the
    /// insert/update/skip decision in `process_sync_chunk`.
    #[tokio::test]
    async fn a_chunk_of_already_present_rows_still_advances_the_offsets() {
        let source = setup_storage().await.unwrap();
        let target = setup_storage().await.unwrap();
        let source_user_id = insert_test_user(&source, "02abc999").await;
        let _target_user_id = insert_test_user(&target, "02abc999").await;
        let now = dt("2024-01-15 12:00:00");

        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id: source_user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "ref-idempotent".to_string(),
            is_outgoing: false,
            satoshis: 4200,
            description: "already current".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("cafebabe9999".to_string()),
            input_beef: None,
            raw_tx: None,
        };
        source.insert_transaction(&tx, None).await.unwrap();

        let empty_map = SyncMap::new();
        let args = || GetSyncChunkArgs {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: "02abc999".to_string(),
            sync_map: &empty_map,
            max_items: 1000,
            max_rough_size: 10_000_000,
            offsets: Default::default(),
        };

        // First pass populates the target.
        let first_chunk = get_sync_chunk(&source, args(), None).await.unwrap();
        let mut first_map = SyncMap::new();
        let first = process_sync_chunk(&target, first_chunk, &mut first_map, None)
            .await
            .unwrap();
        assert!(first.inserts > 0, "the first pass must insert the row");

        // Second pass over the same data, from a FRESH sync map — exactly what
        // re-attaching an already-current backup looks like.
        let second_chunk = get_sync_chunk(&source, args(), None).await.unwrap();
        let mut second_map = SyncMap::new();
        let second = process_sync_chunk(&target, second_chunk, &mut second_map, None)
            .await
            .unwrap();

        assert_eq!(
            (second.inserts, second.updates),
            (0, 0),
            "the row is already present and current, so nothing should be written"
        );
        assert!(
            second_map.transaction.count > 0,
            "the skipped row must still be COUNTED — that count is the next \
             request's offset, and treating this round as no-progress fails \
             the wallet to boot on a healthy backup"
        );
    }

    // -----------------------------------------------------------------------
    // Merge-update fidelity: the update path must carry the full TS field set
    // -----------------------------------------------------------------------

    /// A transaction replicated between createAction and signAction only ever
    /// receives its rawTx/inputBEEF through the merge-UPDATE path. TS writes
    /// the full field set (EntityTransaction/EntityOutput.mergeExisting); a
    /// narrower Rust set silently freezes backup rows at insert-time content
    /// while stamping them current.
    #[tokio::test]
    async fn merge_update_carries_raw_tx_and_locking_script() {
        let target = setup_storage().await.unwrap();
        let _uid = insert_test_user(&target, "02mergewide").await;

        let t1 = dt("2024-01-15 10:00:00");
        let t2 = dt("2024-01-15 11:00:00");

        let tx_at = |when, raw_tx: Option<Vec<u8>>, beef: Option<Vec<u8>>| Transaction {
            created_at: t1,
            updated_at: when,
            transaction_id: 100,
            user_id: 77,
            proven_tx_id: None,
            status: TransactionStatus::Unsigned,
            reference: "ref-widen".to_string(),
            is_outgoing: true,
            satoshis: 5000,
            description: "widen test".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("widen_txid".to_string()),
            input_beef: beef,
            raw_tx,
        };
        let out_at = |when, script: Option<Vec<u8>>, instructions: Option<String>| Output {
            created_at: t1,
            updated_at: when,
            output_id: 200,
            user_id: 77,
            transaction_id: 100,
            basket_id: None,
            spendable: true,
            change: false,
            output_description: Some("widen out".to_string()),
            vout: 0,
            satoshis: 4000,
            provided_by: StorageProvidedBy::You,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: Some("widen_txid".to_string()),
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: instructions,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: script.as_ref().map(|s| s.len() as i64),
            script_offset: None,
            locking_script: script,
        };
        let chunk_with = |tx: Transaction, out: Output| SyncChunk {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: "02mergewide".to_string(),
            user: None,
            proven_txs: None,
            output_baskets: None,
            transactions: Some(vec![tx]),
            outputs: Some(vec![out]),
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

        // Round 1: the row replicates before signing — no rawTx, no script.
        process_sync_chunk(
            &target,
            chunk_with(tx_at(t1, None, None), out_at(t1, None, None)),
            &mut sync_map,
            None,
        )
        .await
        .unwrap();

        // Round 2: the source signed — rawTx/inputBEEF/lockingScript now exist,
        // arriving as an UPDATE to the already-replicated rows.
        let raw = vec![0xde, 0xad, 0xbe, 0xef];
        let beef = vec![0xbe, 0xef];
        let script = vec![0x76, 0xa9, 0x14];
        process_sync_chunk(
            &target,
            chunk_with(
                tx_at(t2, Some(raw.clone()), Some(beef.clone())),
                out_at(t2, Some(script.clone()), Some("spend me".to_string())),
            ),
            &mut sync_map,
            None,
        )
        .await
        .unwrap();

        let txs = target
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        reference: Some("ref-widen".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(
            txs[0].raw_tx.as_deref(),
            Some(raw.as_slice()),
            "rawTx set after first replication must reach the backup via the \
             merge-update path — a restore cannot rebroadcast without it"
        );
        assert_eq!(txs[0].input_beef.as_deref(), Some(beef.as_slice()));
        assert_eq!(
            txs[0].updated_at, t2,
            "the merged row must carry the SOURCE timestamp, not the local clock"
        );

        let outs = target
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        vout: Some(0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(outs.len(), 1);
        assert_eq!(
            outs[0].locking_script.as_deref(),
            Some(script.as_slice()),
            "lockingScript must reach the backup via the merge-update path"
        );
        assert_eq!(outs[0].custom_instructions.as_deref(), Some("spend me"));
        assert_eq!(outs[0].updated_at, t2);
    }

    // -----------------------------------------------------------------------
    // updated_at pass-through: a merge must never stamp a row with local time
    // -----------------------------------------------------------------------

    /// TS writes max(incoming, local) through the update
    /// (StorageKnex.validatePartialForUpdate honors a supplied updated_at).
    /// If the merge instead auto-touches with the local clock, the backup row
    /// is stamped AHEAD of its source, and the strictly-newer merge rule then
    /// rejects every future legitimate update — permanent silent divergence.
    #[tokio::test]
    async fn merge_updated_at_passthrough_prevents_clock_skew_wedge() {
        let target = setup_storage().await.unwrap();
        let _uid = insert_test_user(&target, "02skew").await;

        let mk_label = |when, deleted| TxLabel {
            created_at: dt("2024-01-15 10:00:00"),
            updated_at: when,
            tx_label_id: 300,
            user_id: 77,
            label: "skew-label".to_string(),
            is_deleted: deleted,
        };
        let chunk_with = |label: TxLabel| SyncChunk {
            from_storage_identity_key: "storage-a".to_string(),
            to_storage_identity_key: "storage-b".to_string(),
            user_identity_key: "02skew".to_string(),
            user: None,
            proven_txs: None,
            output_baskets: None,
            transactions: None,
            outputs: None,
            tx_labels: Some(vec![label]),
            tx_label_maps: None,
            output_tags: None,
            output_tag_maps: None,
            certificates: None,
            certificate_fields: None,
            commissions: None,
            proven_tx_reqs: None,
        };

        let t1 = dt("2024-01-15 10:00:00");
        let t2 = dt("2024-01-15 11:00:00");
        let t3 = dt("2024-01-15 12:00:00");

        let mut sync_map = SyncMap::new();
        // Insert at t1, then two consecutive source-side edits at t2 and t3.
        // All three timestamps are in the past relative to the local clock: if
        // the t2 merge stamps the row with "now", t3 is no longer strictly
        // newer and the third update is silently rejected.
        process_sync_chunk(
            &target,
            chunk_with(mk_label(t1, false)),
            &mut sync_map,
            None,
        )
        .await
        .unwrap();
        process_sync_chunk(&target, chunk_with(mk_label(t2, true)), &mut sync_map, None)
            .await
            .unwrap();
        let third = process_sync_chunk(
            &target,
            chunk_with(mk_label(t3, false)),
            &mut sync_map,
            None,
        )
        .await
        .unwrap();

        let labels = target
            .find_tx_labels(
                &FindTxLabelsArgs {
                    partial: TxLabelPartial {
                        label: Some("skew-label".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(third.updates, 1, "the t3 update must not be rejected");
        assert!(
            !labels[0].is_deleted,
            "the t3 edit (is_deleted=false) was rejected: the t2 merge stamped \
             the row ahead of its source and wedged it against the \
             strictly-newer rule"
        );
        assert_eq!(
            labels[0].updated_at, t3,
            "the merged row must carry the source timestamp"
        );
    }

    // -----------------------------------------------------------------------
    // Defect: the dependent-entity filter dropped cross-round children forever
    // -----------------------------------------------------------------------

    fn chunk_done(c: &SyncChunk) -> bool {
        c.proven_txs.as_ref().is_some_and(|v| v.is_empty())
            && c.output_baskets.as_ref().is_some_and(|v| v.is_empty())
            && c.transactions.as_ref().is_some_and(|v| v.is_empty())
            && c.outputs.as_ref().is_some_and(|v| v.is_empty())
            && c.tx_labels.as_ref().is_some_and(|v| v.is_empty())
            && c.tx_label_maps.as_ref().is_some_and(|v| v.is_empty())
            && c.output_tags.as_ref().is_some_and(|v| v.is_empty())
            && c.output_tag_maps.as_ref().is_some_and(|v| v.is_empty())
            && c.certificates.as_ref().is_some_and(|v| v.is_empty())
            && c.certificate_fields.as_ref().is_some_and(|v| v.is_empty())
            && c.commissions.as_ref().is_some_and(|v| v.is_empty())
            && c.proven_tx_reqs.as_ref().is_some_and(|v| v.is_empty())
    }

    fn offsets_from(
        map: &SyncMap,
    ) -> bsv_wallet_toolbox::storage::sync::get_sync_chunk::SyncChunkOffsets {
        bsv_wallet_toolbox::storage::sync::get_sync_chunk::SyncChunkOffsets {
            proven_tx: map.proven_tx.count,
            output_basket: map.output_basket.count,
            output_tag: map.output_tag.count,
            tx_label: map.tx_label.count,
            transaction: map.transaction.count,
            output: map.output.count,
            tx_label_map: map.tx_label_map.count,
            output_tag_map: map.output_tag_map.count,
            certificate: map.certificate.count,
            certificate_field: map.certificate_field.count,
            commission: map.commission.count,
            proven_tx_req: map.proven_tx_req.count,
        }
    }

    fn mk_tx(user_id: i64, when: NaiveDateTime, i: usize) -> Transaction {
        Transaction {
            created_at: when,
            updated_at: when,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: format!("ref-{i:03}"),
            is_outgoing: true,
            satoshis: 1000,
            description: "chunker test".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(format!("txid-{i:03}")),
            input_beef: None,
            raw_tx: None,
        }
    }

    /// A new child row whose parent was synced in an EARLIER round must ride
    /// the chunk and resolve through the consumer's persisted id-map. The old
    /// producer filtered out any dependent row whose parent was not in the
    /// same chunk — on an incremental round the month-old parent is never
    /// re-sent, so a new label on an old transaction was dropped on every
    /// round while `when` advanced past it: silent, permanent loss.
    #[tokio::test]
    async fn cross_round_child_of_old_parent_survives() {
        let source = setup_storage().await.unwrap();
        let target = setup_storage().await.unwrap();
        let user_id = insert_test_user(&source, "02crossround").await;

        let t1 = dt("2024-01-15 10:00:00");
        let t2 = dt("2024-02-01 00:00:00");
        let t3 = dt("2024-02-15 09:00:00");

        // Round-1 state: a transaction, two labels, one map.
        let tx_id = source
            .insert_transaction(&mk_tx(user_id, t1, 0), None)
            .await
            .unwrap();
        let mk_label = |label: &str| TxLabel {
            created_at: t1,
            updated_at: t1,
            tx_label_id: 0,
            user_id,
            label: label.to_string(),
            is_deleted: false,
        };
        let label1_id = source
            .insert_tx_label(&mk_label("cr-label-1"), None)
            .await
            .unwrap();
        let label2_id = source
            .insert_tx_label(&mk_label("cr-label-2"), None)
            .await
            .unwrap();
        source
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: t1,
                    updated_at: t1,
                    tx_label_id: label1_id,
                    transaction_id: tx_id,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        fn chunk_args(producer_map: &SyncMap) -> GetSyncChunkArgs<'_> {
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: "02crossround".to_string(),
                sync_map: producer_map,
                max_items: 1000,
                max_rough_size: 10_000_000,
                offsets: Default::default(),
            }
        }

        // Round 1: full window, everything replicates. The consumer's sync_map
        // (id-map included) persists across rounds, as the trait-level
        // process_sync_chunk persists it in the sync_states row.
        let producer_map = SyncMap::new();
        let chunk1 = get_sync_chunk(&source, chunk_args(&producer_map), None)
            .await
            .unwrap();
        let mut consumer_map = SyncMap::new();
        process_sync_chunk(&target, chunk1, &mut consumer_map, None)
            .await
            .unwrap();

        // Between rounds: a NEW map labels the month-old transaction with the
        // month-old second label. Neither parent will be in the next window.
        source
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: t3,
                    updated_at: t3,
                    tx_label_id: label2_id,
                    transaction_id: tx_id,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        // Round 2: incremental window starting after t1.
        let mut producer_map = SyncMap::new();
        for esm in producer_map.entity_maps_mut() {
            esm.max_updated_at = Some(t2);
        }
        let chunk2 = get_sync_chunk(&source, chunk_args(&producer_map), None)
            .await
            .unwrap();

        let maps = chunk2
            .tx_label_maps
            .as_ref()
            .expect("txLabelMaps must be present in round 2");
        assert_eq!(
            maps.len(),
            1,
            "the new map on an old parent must ride the chunk even though its \
             parents are not in the window — the producer must not filter it"
        );
        assert!(
            chunk2.transactions.as_ref().is_some_and(|v| v.is_empty()),
            "the old parent transaction is not in the incremental window"
        );

        process_sync_chunk(&target, chunk2, &mut consumer_map, None)
            .await
            .unwrap();

        let target_maps = target
            .find_tx_label_maps(&FindTxLabelMapsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(
            target_maps.len(),
            2,
            "the backup must hold both maps after the incremental round"
        );
    }

    // -----------------------------------------------------------------------
    // The chunker exhausts parents before children within a round
    // -----------------------------------------------------------------------

    /// TS's chunker spends ONE global budget across the 12 entities in
    /// dependency order, fully exhausting each entity for the window before
    /// the next emits a row. That is what makes the consumer's fail-closed
    /// FK remap safe on initial syncs of wallets larger than one chunk:
    /// children referencing late-page parents only ever appear after every
    /// parent in the window has been sent.
    #[tokio::test]
    async fn parents_exhaust_before_children_within_a_round() {
        let source = setup_storage().await.unwrap();
        let target = setup_storage().await.unwrap();
        let user_id = insert_test_user(&source, "02exhaust").await;

        let t1 = dt("2024-01-15 10:00:00");

        // 30 transactions; a label; maps on the LAST five transactions, so
        // every map references a parent beyond the first chunk's budget.
        let mut tx_ids = Vec::new();
        for i in 0..30 {
            tx_ids.push(
                source
                    .insert_transaction(&mk_tx(user_id, t1, i), None)
                    .await
                    .unwrap(),
            );
        }
        let label_id = source
            .insert_tx_label(
                &TxLabel {
                    created_at: t1,
                    updated_at: t1,
                    tx_label_id: 0,
                    user_id,
                    label: "late".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        for tx_id in &tx_ids[25..30] {
            source
                .insert_tx_label_map(
                    &TxLabelMap {
                        created_at: t1,
                        updated_at: t1,
                        tx_label_id: label_id,
                        transaction_id: *tx_id,
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
        }

        let producer_map = SyncMap::new();
        let mut consumer_map = SyncMap::new();
        let mut first = true;
        for round in 0..40 {
            assert!(round < 39, "sync loop failed to converge");
            let chunk = get_sync_chunk(
                &source,
                GetSyncChunkArgs {
                    from_storage_identity_key: "storage-a".to_string(),
                    to_storage_identity_key: "storage-b".to_string(),
                    user_identity_key: "02exhaust".to_string(),
                    sync_map: &producer_map,
                    max_items: 10,
                    max_rough_size: 10_000_000,
                    offsets: offsets_from(&consumer_map),
                },
                None,
            )
            .await
            .unwrap();

            if first {
                // The global budget (10) is spent on the label and the first
                // nine transactions; the child entities must be ABSENT, not
                // paged in parallel with their parents.
                assert_eq!(
                    chunk.transactions.as_ref().map(|v| v.len()),
                    Some(9),
                    "chunk 1 spends the remaining budget on transactions"
                );
                assert!(
                    chunk.tx_label_maps.is_none(),
                    "child rows must not appear before their parents are exhausted"
                );
                assert!(chunk.outputs.is_none());
                first = false;
            }

            let done = chunk_done(&chunk);
            process_sync_chunk(&target, chunk, &mut consumer_map, None)
                .await
                .expect("every child's parent must already be mapped");
            if done {
                break;
            }
        }

        let target_txs = target
            .find_transactions(&FindTransactionsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(target_txs.len(), 30);
        let target_maps = target
            .find_tx_label_maps(&FindTxLabelMapsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(
            target_maps.len(),
            5,
            "maps referencing late-page parents must all replicate"
        );
    }

    // -----------------------------------------------------------------------
    // maxRoughSize bounds a chunk
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rough_size_budget_bounds_a_chunk() {
        let source = setup_storage().await.unwrap();
        let user_id = insert_test_user(&source, "02rough").await;
        let t1 = dt("2024-01-15 10:00:00");
        for i in 0..5 {
            source
                .insert_transaction(&mk_tx(user_id, t1, i), None)
                .await
                .unwrap();
        }

        let producer_map = SyncMap::new();
        let chunk = get_sync_chunk(
            &source,
            GetSyncChunkArgs {
                from_storage_identity_key: "storage-a".to_string(),
                to_storage_identity_key: "storage-b".to_string(),
                user_identity_key: "02rough".to_string(),
                sync_map: &producer_map,
                max_items: 1000,
                max_rough_size: 1,
                offsets: Default::default(),
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            chunk.transactions.as_ref().map(|v| v.len()),
            Some(1),
            "the first row exhausts a 1-byte rough budget; the chunk stops there"
        );
        assert!(
            chunk.outputs.is_none(),
            "entities after the budget stop must be absent"
        );
    }
}
