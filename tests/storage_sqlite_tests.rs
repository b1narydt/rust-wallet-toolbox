//! SQLite CRUD integration tests for StorageSqlx<Sqlite>.
//!
//! Each test creates a fresh in-memory SQLite database, runs migrations,
//! and verifies CRUD operations. Tests are gated with `#[cfg(feature = "sqlite")]`.

#[cfg(feature = "sqlite")]
mod storage_sqlite {
    use chrono::NaiveDateTime;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::{ProvenTxReqStatus, TransactionStatus};
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

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

    // -----------------------------------------------------------------------
    // Test 1: Insert a User, find by identity_key, verify all fields match
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_insert_and_find_user() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02abc123def456".to_string(),
            active_storage: "default".to_string(),
        };

        let user_id = storage.insert_user(&user, None).await.unwrap();
        assert!(user_id > 0, "insert_user should return a positive ID");

        // Find by identity key
        let args = FindUsersArgs {
            partial: UserPartial {
                identity_key: Some("02abc123def456".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_users(&args, None).await.unwrap();
        assert_eq!(results.len(), 1);

        let found = &results[0];
        assert_eq!(found.user_id, user_id);
        assert_eq!(found.identity_key, "02abc123def456");
        assert_eq!(found.active_storage, "default");
    }

    // -----------------------------------------------------------------------
    // Test 2: Insert an OutputBasket, count, update, verify updated values
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_insert_count_update_output_basket() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        // First insert a user (foreign key requirement)
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "user1".to_string(),
            active_storage: String::new(),
        };
        let user_id = storage.insert_user(&user, None).await.unwrap();

        let basket = OutputBasket {
            created_at: now,
            updated_at: now,
            basket_id: 0,
            user_id,
            name: "test_basket".to_string(),
            number_of_desired_utxos: 6,
            minimum_desired_utxo_value: 10000,
            is_deleted: false,
        };

        let basket_id = storage.insert_output_basket(&basket, None).await.unwrap();
        assert!(basket_id > 0);

        // Count
        let count = storage
            .count_output_baskets(&FindOutputBasketsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(count, 1);

        // Update the name
        let update = OutputBasketPartial {
            name: Some("updated_basket".to_string()),
            ..Default::default()
        };
        let rows = storage
            .update_output_basket(basket_id, &update, None)
            .await
            .unwrap();
        assert_eq!(rows, 1);

        // Find and verify updated value
        let args = FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                basket_id: Some(basket_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_output_baskets(&args, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "updated_basket");
    }

    // -----------------------------------------------------------------------
    // Test 3: Insert a Transaction with all fields, find by txid
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_insert_and_find_transaction() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "user_tx_test".to_string(),
            active_storage: String::new(),
        };
        let user_id = storage.insert_user(&user, None).await.unwrap();

        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Unprocessed,
            reference: "ref-001".to_string(),
            is_outgoing: true,
            satoshis: 50000,
            description: "Test transaction".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("abc123def456".to_string()),
            input_beef: Some(vec![1, 2, 3]),
            raw_tx: Some(vec![4, 5, 6]),
        };

        let tx_id = storage.insert_transaction(&tx, None).await.unwrap();
        assert!(tx_id > 0);

        // Find by txid
        let args = FindTransactionsArgs {
            partial: TransactionPartial {
                txid: Some("abc123def456".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_transactions(&args, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].transaction_id, tx_id);
        assert_eq!(results[0].satoshis, 50000);
        assert_eq!(results[0].status, TransactionStatus::Unprocessed);
        assert_eq!(results[0].description, "Test transaction");
    }

    // -----------------------------------------------------------------------
    // Test 4: Insert multiple Outputs, test pagination
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_output_pagination() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let user_id = storage
            .insert_user(
                &User {
                    created_at: now,
                    updated_at: now,
                    user_id: 0,
                    identity_key: "paged_user".to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap();

        let tx_id = storage
            .insert_transaction(
                &Transaction {
                    created_at: now,
                    updated_at: now,
                    transaction_id: 0,
                    user_id,
                    proven_tx_id: None,
                    status: TransactionStatus::Completed,
                    reference: "ref-paged".to_string(),
                    is_outgoing: false,
                    satoshis: 100000,
                    description: "Paged test tx".to_string(),
                    version: None,
                    lock_time: None,
                    txid: None,
                    input_beef: None,
                    raw_tx: None,
                },
                None,
            )
            .await
            .unwrap();

        // Insert 5 outputs
        for i in 0..5 {
            let output = Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id,
                transaction_id: tx_id,
                basket_id: None,
                spendable: true,
                change: false,
                output_description: None,
                vout: i,
                satoshis: (i as i64 + 1) * 1000,
                provided_by: StorageProvidedBy::Storage,
                purpose: "test".to_string(),
                output_type: "P2PKH".to_string(),
                txid: None,
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
            storage.insert_output(&output, None).await.unwrap();
        }

        // Page 1: limit=2, offset=0
        let args = FindOutputsArgs {
            paged: Some(Paged {
                limit: 2,
                offset: 0,
            }),
            ..Default::default()
        };
        let page1 = storage.find_outputs(&args, None).await.unwrap();
        assert_eq!(page1.len(), 2);

        // Page 2: limit=2, offset=2
        let args = FindOutputsArgs {
            paged: Some(Paged {
                limit: 2,
                offset: 2,
            }),
            ..Default::default()
        };
        let page2 = storage.find_outputs(&args, None).await.unwrap();
        assert_eq!(page2.len(), 2);

        // Page 3: limit=2, offset=4
        let args = FindOutputsArgs {
            paged: Some(Paged {
                limit: 2,
                offset: 4,
            }),
            ..Default::default()
        };
        let page3 = storage.find_outputs(&args, None).await.unwrap();
        assert_eq!(page3.len(), 1);

        // Total count
        let total = storage
            .count_outputs(&FindOutputsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(total, 5);
    }

    // -----------------------------------------------------------------------
    // Test 5: Transaction rollback -- data should NOT be persisted
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_transaction_rollback() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let trx = storage.begin_transaction().await.unwrap();
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "rollback_user".to_string(),
            active_storage: String::new(),
        };
        let _user_id = storage.insert_user(&user, Some(&trx)).await.unwrap();

        // Rollback
        storage.rollback_transaction(trx).await.unwrap();

        // Verify user is NOT persisted
        let count = storage
            .count_users(&FindUsersArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(count, 0, "Rolled-back user should not exist");
    }

    // -----------------------------------------------------------------------
    // Test 6: Transaction commit -- data SHOULD be persisted
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_transaction_commit() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let trx = storage.begin_transaction().await.unwrap();
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "commit_user".to_string(),
            active_storage: String::new(),
        };
        let _user_id = storage.insert_user(&user, Some(&trx)).await.unwrap();

        // Commit
        storage.commit_transaction(trx).await.unwrap();

        // Verify user IS persisted
        let count = storage
            .count_users(&FindUsersArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(count, 1, "Committed user should exist");
    }

    // -----------------------------------------------------------------------
    // Test 7: Insert ProvenTx and ProvenTxReq, find by ID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_proven_tx_and_req_roundtrip() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let ptx = ProvenTx {
            created_at: now,
            updated_at: now,
            proven_tx_id: 0,
            txid: "deadbeef1234".to_string(),
            height: 800000,
            index: 42,
            merkle_path: vec![10, 20, 30],
            raw_tx: vec![40, 50, 60],
            block_hash: "blockhash123".to_string(),
            merkle_root: "merkleroot456".to_string(),
        };

        let ptx_id = storage.insert_proven_tx(&ptx, None).await.unwrap();
        assert!(ptx_id > 0);

        // Find by ID
        let args = FindProvenTxsArgs {
            partial: ProvenTxPartial {
                proven_tx_id: Some(ptx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_proven_txs(&args, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].txid, "deadbeef1234");
        assert_eq!(results[0].height, 800000);

        // Insert a ProvenTxReq
        let req = ProvenTxReq {
            created_at: now,
            updated_at: now,
            proven_tx_req_id: 0,
            proven_tx_id: Some(ptx_id),
            status: ProvenTxReqStatus::Completed,
            attempts: 3,
            notified: true,
            txid: "deadbeef1234".to_string(),
            batch: Some("batch-1".to_string()),
            history: "{}".to_string(),
            notify: "{}".to_string(),
            raw_tx: vec![1, 2, 3],
            input_beef: None,
        };

        let req_id = storage.insert_proven_tx_req(&req, None).await.unwrap();
        assert!(req_id > 0);

        let args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                proven_tx_req_id: Some(req_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_proven_tx_reqs(&args, None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ProvenTxReqStatus::Completed);
        assert_eq!(results[0].attempts, 3);
    }

    // -----------------------------------------------------------------------
    // Test 8: find_or_insert_user -- first call inserts, second returns existing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_find_or_insert_user() {
        let storage = setup_test_storage().await.unwrap();

        let (user1, was_new1) = storage
            .find_or_insert_user("unique_identity_key_1", None)
            .await
            .unwrap();
        assert!(was_new1, "First call should insert");
        assert!(user1.user_id > 0);

        let (user2, was_new2) = storage
            .find_or_insert_user("unique_identity_key_1", None)
            .await
            .unwrap();
        assert!(!was_new2, "Second call should find existing");
        assert_eq!(user2.user_id, user1.user_id);
    }

    // -----------------------------------------------------------------------
    // Test 9: Insert Certificate and CertificateFields, verify FK relationship
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_certificate_and_fields() {
        let storage = setup_test_storage().await.unwrap();
        let now = test_datetime();

        let user_id = storage
            .insert_user(
                &User {
                    created_at: now,
                    updated_at: now,
                    user_id: 0,
                    identity_key: "cert_user".to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap();

        let cert = Certificate {
            created_at: now,
            updated_at: now,
            certificate_id: 0,
            user_id,
            cert_type: "identity".to_string(),
            serial_number: "SN-12345".to_string(),
            certifier: "certifier_pubkey".to_string(),
            subject: "subject_pubkey".to_string(),
            verifier: Some("verifier_pubkey".to_string()),
            revocation_outpoint: "outpoint:0".to_string(),
            signature: "sig_hex_abc".to_string(),
            is_deleted: false,
        };

        let cert_id = storage.insert_certificate(&cert, None).await.unwrap();
        assert!(cert_id > 0);

        // Insert fields
        let field1 = CertificateField {
            created_at: now,
            updated_at: now,
            user_id,
            certificate_id: cert_id,
            field_name: "email".to_string(),
            field_value: "encrypted_email".to_string(),
            master_key: "master_key_1".to_string(),
        };
        storage
            .insert_certificate_field(&field1, None)
            .await
            .unwrap();

        let field2 = CertificateField {
            created_at: now,
            updated_at: now,
            user_id,
            certificate_id: cert_id,
            field_name: "name".to_string(),
            field_value: "encrypted_name".to_string(),
            master_key: "master_key_2".to_string(),
        };
        storage
            .insert_certificate_field(&field2, None)
            .await
            .unwrap();

        // Find certificate by ID
        let cert_args = FindCertificatesArgs {
            partial: CertificatePartial {
                certificate_id: Some(cert_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let certs = storage.find_certificates(&cert_args, None).await.unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].cert_type, "identity");

        // Find fields by certificate_id
        let field_args = FindCertificateFieldsArgs {
            partial: CertificateFieldPartial {
                certificate_id: Some(cert_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let fields = storage
            .find_certificate_fields(&field_args, None)
            .await
            .unwrap();
        assert_eq!(fields.len(), 2);

        // Count fields
        let field_count = storage
            .count_certificate_fields(&field_args, None)
            .await
            .unwrap();
        assert_eq!(field_count, 2);
    }
}
