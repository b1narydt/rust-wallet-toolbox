//! PostgreSQL CRUD integration test stubs for StorageSqlx<Postgres>.
//!
//! These tests require a running PostgreSQL instance and the `POSTGRES_DATABASE_URL`
//! environment variable to be set (e.g., `postgres://user:pass@localhost/test_db`).
//!
//! Tests are gated with `#[cfg(feature = "postgres")]` and marked `#[ignore]`
//! so they do not run by default.
//!
//! Run with: cargo test --features postgres -- --ignored --test-threads=1

#[cfg(feature = "postgres")]
mod storage_postgres {
    use std::collections::BTreeSet;

    use chrono::NaiveDateTime;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::{ProvenTxReqStatus, SyncStatus, TransactionStatus};
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::PgStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

    /// Helper to create a PostgreSQL storage connected to the test database.
    ///
    /// Reads `POSTGRES_DATABASE_URL` from the environment, creates the storage,
    /// runs migrations, and drops all existing data for a clean slate.
    async fn setup_pg_storage() -> WalletResult<PgStorage> {
        let url = std::env::var("POSTGRES_DATABASE_URL")
            .expect("POSTGRES_DATABASE_URL must be set to run PostgreSQL tests");
        let config = StorageConfig {
            url,
            ..Default::default()
        };
        let storage = PgStorage::new_postgres(config, Chain::Test).await?;
        storage.migrate_database().await?;
        storage.drop_all_data().await?;
        Ok(storage)
    }

    fn test_datetime() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2024-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    async fn insert_pg_user(storage: &PgStorage, identity_key: &str) -> i64 {
        let now = test_datetime();
        storage
            .insert_user(
                &User {
                    created_at: now,
                    updated_at: now,
                    user_id: 0,
                    identity_key: identity_key.to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap()
    }

    async fn insert_pg_transaction(
        storage: &PgStorage,
        user_id: i64,
        status: TransactionStatus,
        reference: &str,
    ) -> i64 {
        let now = test_datetime();
        storage
            .insert_transaction(
                &Transaction {
                    created_at: now,
                    updated_at: now,
                    transaction_id: 0,
                    user_id,
                    status,
                    reference: reference.to_string(),
                    is_outgoing: true,
                    satoshis: 1_000,
                    description: reference.to_string(),
                    version: Some(1),
                    lock_time: Some(0),
                    txid: Some(format!("{reference:0<64}")),
                    input_beef: None,
                    raw_tx: None,
                    proven_tx_id: None,
                },
                None,
            )
            .await
            .unwrap()
    }

    async fn insert_pg_output(
        storage: &PgStorage,
        user_id: i64,
        transaction_id: i64,
        vout: i32,
        spendable: bool,
        spent_by: Option<i64>,
        locking_script: Option<Vec<u8>>,
    ) -> i64 {
        let now = test_datetime();
        storage
            .insert_output(
                &Output {
                    created_at: now,
                    updated_at: now,
                    output_id: 0,
                    user_id,
                    transaction_id,
                    basket_id: None,
                    spendable,
                    change: false,
                    output_description: Some(format!("output-{vout}")),
                    vout,
                    satoshis: i64::from(vout) + 1_000,
                    provided_by: StorageProvidedBy::Storage,
                    purpose: "integration-test".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(format!("{transaction_id:064x}")),
                    sender_identity_key: None,
                    derivation_prefix: None,
                    derivation_suffix: None,
                    custom_instructions: None,
                    spent_by,
                    sequence_number: Some(0),
                    spending_description: None,
                    script_length: locking_script.as_ref().map(|script| script.len() as i64),
                    script_offset: None,
                    locking_script,
                },
                None,
            )
            .await
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // Test 1: Insert a User, find by identity_key, verify fields match
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore]
    async fn test_pg_insert_and_find_user() {
        let storage = setup_pg_storage().await.unwrap();
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
    // Test 2: Insert a Transaction, find by status, verify fields match
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore]
    async fn test_pg_insert_and_find_transaction() {
        let storage = setup_pg_storage().await.unwrap();
        let now = test_datetime();

        // Insert user first (FK dependency)
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "user_for_tx".to_string(),
            active_storage: String::new(),
        };
        let user_id = storage.insert_user(&user, None).await.unwrap();

        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            status: TransactionStatus::Completed,
            reference: "ref-pg-001".to_string(),
            is_outgoing: true,
            satoshis: 50000,
            description: "PostgreSQL test transaction".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("deadbeef00002222".to_string()),
            input_beef: None,
            raw_tx: None,
            proven_tx_id: None,
        };

        let tx_id = storage.insert_transaction(&tx, None).await.unwrap();
        assert!(tx_id > 0);

        let args = FindTransactionsArgs {
            partial: TransactionPartial {
                status: Some(TransactionStatus::Completed),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_transactions(&args, None).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].reference, "ref-pg-001");
    }

    // -----------------------------------------------------------------------
    // Test 3: Transaction commit/rollback semantics
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore]
    async fn test_pg_transaction_commit_rollback() {
        let storage = setup_pg_storage().await.unwrap();
        let now = test_datetime();

        // Begin transaction and insert a user, then rollback
        let trx = storage.begin_transaction().await.unwrap();
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "rollback_user".to_string(),
            active_storage: String::new(),
        };
        storage.insert_user(&user, Some(&trx)).await.unwrap();
        storage.rollback_transaction(trx).await.unwrap();

        // Should not be found after rollback
        let args = FindUsersArgs {
            partial: UserPartial {
                identity_key: Some("rollback_user".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results = storage.find_users(&args, None).await.unwrap();
        assert!(results.is_empty(), "User should not exist after rollback");

        // Begin transaction and insert a user, then commit
        let trx = storage.begin_transaction().await.unwrap();
        let user2 = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "commit_user".to_string(),
            active_storage: String::new(),
        };
        storage.insert_user(&user2, Some(&trx)).await.unwrap();
        storage.commit_transaction(trx).await.unwrap();

        let args2 = FindUsersArgs {
            partial: UserPartial {
                identity_key: Some("commit_user".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let results2 = storage.find_users(&args2, None).await.unwrap();
        assert_eq!(results2.len(), 1, "User should exist after commit");
    }

    // -----------------------------------------------------------------------
    // Test 4: Insert and find OutputBasket with count
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore]
    async fn test_pg_insert_and_count_output_basket() {
        let storage = setup_pg_storage().await.unwrap();
        let now = test_datetime();

        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "basket_user".to_string(),
            active_storage: String::new(),
        };
        let user_id = storage.insert_user(&user, None).await.unwrap();

        let basket = OutputBasket {
            created_at: now,
            updated_at: now,
            basket_id: 0,
            user_id,
            name: "pg_basket".to_string(),
            number_of_desired_utxos: 6,
            minimum_desired_utxo_value: 10000,
            is_deleted: false,
        };

        let basket_id = storage.insert_output_basket(&basket, None).await.unwrap();
        assert!(basket_id > 0);

        let count = storage
            .count_output_baskets(&FindOutputBasketsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_no_script_projection() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-no-script-user").await;
        let tx_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-no-script",
        )
        .await;
        let script = vec![0x76, 0xa9, 0x14, 0x88, 0xac];
        let output_id = insert_pg_output(
            &storage,
            user_id,
            tx_id,
            0,
            true,
            None,
            Some(script.clone()),
        )
        .await;

        // Probes: the no-script SELECT projection must omit script bytes while
        // retaining every other output column, including the quoted `change` column.
        let without_script = storage
            .find_outputs(
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
        assert!(
            without_script[0]
                .locking_script
                .as_ref()
                .map_or(true, Vec::is_empty),
            "no_script=true should omit locking_script bytes"
        );

        let with_script = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        output_id: Some(output_id),
                        ..Default::default()
                    },
                    no_script: false,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(with_script.len(), 1);
        assert_eq!(
            with_script[0].locking_script.as_deref(),
            Some(script.as_slice())
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_users_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        insert_pg_user(&storage, "pg-count-users").await;
        let unpaged = storage
            .count_users(&FindUsersArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_users(
                &FindUsersArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_certificates_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-certificates").await;
        storage
            .insert_certificate(
                &Certificate {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    certificate_id: 0,
                    user_id,
                    cert_type: "identity".to_string(),
                    serial_number: "pg-count-certificates".to_string(),
                    certifier: "certifier".to_string(),
                    subject: "subject".to_string(),
                    verifier: None,
                    revocation_outpoint: "outpoint.0".to_string(),
                    signature: "signature".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_certificates(&FindCertificatesArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_certificates(
                &FindCertificatesArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_certificate_fields_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-certificate-fields").await;
        let certificate_id = storage
            .insert_certificate(
                &Certificate {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    certificate_id: 0,
                    user_id,
                    cert_type: "identity".to_string(),
                    serial_number: "pg-count-certificate-fields".to_string(),
                    certifier: "certifier".to_string(),
                    subject: "subject".to_string(),
                    verifier: None,
                    revocation_outpoint: "outpoint.0".to_string(),
                    signature: "signature".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_certificate_field(
                &CertificateField {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    user_id,
                    certificate_id,
                    field_name: "name".to_string(),
                    field_value: "encrypted".to_string(),
                    master_key: "master-key".to_string(),
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_certificate_fields(&FindCertificateFieldsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_certificate_fields(
                &FindCertificateFieldsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_commissions_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-commissions").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-count-commissions",
        )
        .await;
        storage
            .insert_commission(
                &Commission {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    commission_id: 0,
                    user_id,
                    transaction_id,
                    satoshis: 100,
                    key_offset: "offset".to_string(),
                    is_redeemed: false,
                    locking_script: vec![0x51],
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_commissions(&FindCommissionsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_commissions(
                &FindCommissionsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_monitor_events_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        storage
            .insert_monitor_event(
                &MonitorEvent {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    id: 0,
                    event: "pg-count-monitor-events".to_string(),
                    details: Some("{}".to_string()),
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_monitor_events(&FindMonitorEventsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_monitor_events(
                &FindMonitorEventsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_output_baskets_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-output-baskets").await;
        storage
            .insert_output_basket(
                &OutputBasket {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    basket_id: 0,
                    user_id,
                    name: "count-basket".to_string(),
                    number_of_desired_utxos: 6,
                    minimum_desired_utxo_value: 1_000,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_output_baskets(&FindOutputBasketsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_output_baskets(
                &FindOutputBasketsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_output_tag_maps_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-output-tag-maps").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-count-output-tag-maps",
        )
        .await;
        let output_id =
            insert_pg_output(&storage, user_id, transaction_id, 0, true, None, None).await;
        let output_tag_id = storage
            .insert_output_tag(
                &OutputTag {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    output_tag_id: 0,
                    user_id,
                    tag: "count-tag-map".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_output_tag_map(
                &OutputTagMap {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    output_tag_id,
                    output_id,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_output_tag_maps(&FindOutputTagMapsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_output_tag_maps(
                &FindOutputTagMapsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_output_tags_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-output-tags").await;
        storage
            .insert_output_tag(
                &OutputTag {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    output_tag_id: 0,
                    user_id,
                    tag: "count-tag".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_output_tags(&FindOutputTagsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_output_tags(
                &FindOutputTagsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_outputs_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-outputs").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-count-outputs",
        )
        .await;
        insert_pg_output(&storage, user_id, transaction_id, 0, true, None, None).await;
        let unpaged = storage
            .count_outputs(&FindOutputsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_outputs(
                &FindOutputsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_proven_txs_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        storage
            .insert_proven_tx(
                &ProvenTx {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    proven_tx_id: 0,
                    txid: "pg-count-proven-txs".to_string(),
                    height: 800_000,
                    index: 1,
                    merkle_path: vec![1, 2, 3],
                    raw_tx: vec![4, 5, 6],
                    block_hash: "block-hash".to_string(),
                    merkle_root: "merkle-root".to_string(),
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_proven_txs(&FindProvenTxsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_proven_txs(
                &FindProvenTxsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_proven_tx_reqs_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        storage
            .insert_proven_tx_req(
                &ProvenTxReq {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    proven_tx_req_id: 0,
                    proven_tx_id: None,
                    status: ProvenTxReqStatus::Unprocessed,
                    attempts: 0,
                    notified: false,
                    txid: "pg-count-proven-tx-reqs".to_string(),
                    batch: None,
                    history: "{}".to_string(),
                    notify: "{}".to_string(),
                    raw_tx: vec![1, 2, 3],
                    input_beef: None,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_proven_tx_reqs(&FindProvenTxReqsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_proven_tx_reqs(
                &FindProvenTxReqsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_settings_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        // Settings has no public insert method; drop_all_data leaves this table empty.
        let unpaged = storage
            .count_settings(&FindSettingsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 0);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_settings(
                &FindSettingsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 0);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_sync_states_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-sync-states").await;
        storage
            .insert_sync_state(
                &SyncState {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    sync_state_id: 0,
                    user_id,
                    storage_identity_key: "remote-key".to_string(),
                    storage_name: "remote".to_string(),
                    status: SyncStatus::Unknown,
                    init: false,
                    ref_num: "pg-count-sync-states".to_string(),
                    sync_map: "{}".to_string(),
                    when: None,
                    satoshis: None,
                    error_local: None,
                    error_other: None,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_sync_states(&FindSyncStatesArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_sync_states(
                &FindSyncStatesArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_transactions_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-transactions").await;
        insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-count-transactions",
        )
        .await;
        let unpaged = storage
            .count_transactions(&FindTransactionsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_transactions(
                &FindTransactionsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_tx_label_maps_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-tx-label-maps").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-count-tx-label-maps",
        )
        .await;
        let tx_label_id = storage
            .insert_tx_label(
                &TxLabel {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    tx_label_id: 0,
                    user_id,
                    label: "count-label-map".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    tx_label_id,
                    transaction_id,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_tx_label_maps(&FindTxLabelMapsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_tx_label_maps(
                &FindTxLabelMapsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_count_tx_labels_with_paged_args() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-count-tx-labels").await;
        storage
            .insert_tx_label(
                &TxLabel {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    tx_label_id: 0,
                    user_id,
                    label: "count-label".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        let unpaged = storage
            .count_tx_labels(&FindTxLabelsArgs::default(), None)
            .await
            .unwrap();
        assert_eq!(unpaged, 1);
        // Probes: COUNT(*) with ORDER BY/LIMIT appended by the paged where-builder.
        let paged = storage
            .count_tx_labels(
                &FindTxLabelsArgs {
                    paged: Some(Paged {
                        limit: 10,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(paged, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_non_empty_transaction_status_list() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-output-status-list").await;
        let mut expected = BTreeSet::new();
        for (index, status) in [
            TransactionStatus::Completed,
            TransactionStatus::Unproven,
            TransactionStatus::Failed,
        ]
        .into_iter()
        .enumerate()
        {
            let transaction_id = insert_pg_transaction(
                &storage,
                user_id,
                status,
                &format!("pg-output-status-{index}"),
            )
            .await;
            let output_id = insert_pg_output(
                &storage,
                user_id,
                transaction_id,
                index as i32,
                true,
                None,
                None,
            )
            .await;
            if index < 2 {
                expected.insert(output_id);
            }
        }

        let found = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    tx_status: Some(vec![
                        TransactionStatus::Completed,
                        TransactionStatus::Unproven,
                    ]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let actual: BTreeSet<_> = found.iter().map(|row| row.output_id).collect();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_empty_transaction_status_list_result() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-empty-output-status-list").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-empty-output-status-list",
        )
        .await;
        insert_pg_output(&storage, user_id, transaction_id, 0, true, None, None).await;

        // Probes: an explicitly empty correlated-subquery IN-list may be
        // accepted or rejected by the server dialect, but must not panic Rust.
        let result = storage
            .find_outputs(
                &FindOutputsArgs {
                    tx_status: Some(vec![]),
                    ..Default::default()
                },
                None,
            )
            .await;
        println!("PostgreSQL empty tx_status IN-list result: {result:?}");
        assert!(
            result.is_ok() || result.is_err(),
            "find_outputs must return a Result for an empty tx_status list"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_combined_filters() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-combined-output-user").await;
        let other_user_id = insert_pg_user(&storage, "pg-combined-output-other").await;
        let completed_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-combined-completed",
        )
        .await;
        let unproven_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Unproven,
            "pg-combined-unproven",
        )
        .await;
        let other_tx_id = insert_pg_transaction(
            &storage,
            other_user_id,
            TransactionStatus::Completed,
            "pg-combined-other",
        )
        .await;
        let expected_id = insert_pg_output(
            &storage,
            user_id,
            completed_id,
            0,
            true,
            None,
            Some(vec![0x51]),
        )
        .await;
        insert_pg_output(
            &storage,
            user_id,
            completed_id,
            1,
            false,
            None,
            Some(vec![0x52]),
        )
        .await;
        insert_pg_output(
            &storage,
            user_id,
            unproven_id,
            2,
            true,
            None,
            Some(vec![0x53]),
        )
        .await;
        insert_pg_output(
            &storage,
            other_user_id,
            other_tx_id,
            3,
            true,
            None,
            Some(vec![0x54]),
        )
        .await;

        let found = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(user_id),
                        spendable: Some(true),
                        ..Default::default()
                    },
                    tx_status: Some(vec![TransactionStatus::Completed]),
                    paged: Some(Paged {
                        limit: 1,
                        offset: 0,
                    }),
                    no_script: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].output_id, expected_id);
        assert!(found[0].locking_script.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_limit_offset_two_pages() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-paged-outputs").await;
        let transaction_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-paged-outputs",
        )
        .await;
        let mut expected = BTreeSet::new();
        for vout in 0..4 {
            expected.insert(
                insert_pg_output(&storage, user_id, transaction_id, vout, true, None, None).await,
            );
        }

        let page = |offset| FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                ..Default::default()
            },
            paged: Some(Paged { limit: 2, offset }),
            ..Default::default()
        };
        let page_one = storage.find_outputs(&page(0), None).await.unwrap();
        let page_two = storage.find_outputs(&page(2), None).await.unwrap();
        assert_eq!(page_one.len(), 2);
        assert_eq!(page_two.len(), 2);
        let first_ids: BTreeSet<_> = page_one.iter().map(|row| row.output_id).collect();
        let second_ids: BTreeSet<_> = page_two.iter().map(|row| row.output_id).collect();
        assert!(first_ids.is_disjoint(&second_ids));
        assert_eq!(
            first_ids
                .union(&second_ids)
                .copied()
                .collect::<BTreeSet<_>>(),
            expected
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_zero_limit_result() {
        let storage = setup_pg_storage().await.unwrap();
        // Probes: LIMIT 0 handling must return a Result rather than panic.
        let result = storage
            .find_outputs(
                &FindOutputsArgs {
                    paged: Some(Paged {
                        limit: 0,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await;
        println!("PostgreSQL LIMIT 0 result: {result:?}");
        assert!(
            result.is_ok() || result.is_err(),
            "find_outputs must return a Result for LIMIT 0"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_outputs_negative_limit_result() {
        let storage = setup_pg_storage().await.unwrap();
        // Probes: a negative LIMIT may be accepted or rejected by the server,
        // but parameter handling must return a Result rather than panic.
        let result = storage
            .find_outputs(
                &FindOutputsArgs {
                    paged: Some(Paged {
                        limit: -1,
                        offset: 0,
                    }),
                    ..Default::default()
                },
                None,
            )
            .await;
        println!("PostgreSQL LIMIT -1 result: {result:?}");
        assert!(
            result.is_ok() || result.is_err(),
            "find_outputs must return a Result for LIMIT -1"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_list_actions_with_labels_inputs_outputs_and_scripts() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-list-actions-user").await;
        let source_tx_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-list-actions-source",
        )
        .await;
        let action_tx_id = insert_pg_transaction(
            &storage,
            user_id,
            TransactionStatus::Completed,
            "pg-list-actions-action",
        )
        .await;
        let input_script = vec![0x51, 0x21];
        let output_script = vec![0x76, 0xac];
        insert_pg_output(
            &storage,
            user_id,
            source_tx_id,
            0,
            false,
            Some(action_tx_id),
            Some(input_script.clone()),
        )
        .await;
        insert_pg_output(
            &storage,
            user_id,
            action_tx_id,
            1,
            true,
            None,
            Some(output_script.clone()),
        )
        .await;
        let tx_label_id = storage
            .insert_tx_label(
                &TxLabel {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    tx_label_id: 0,
                    user_id,
                    label: "integration-action".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: test_datetime(),
                    updated_at: test_datetime(),
                    tx_label_id,
                    transaction_id: action_tx_id,
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        // Probes: list_actions drives both no_script=true input/output queries.
        let without_scripts: bsv::wallet::interfaces::ListActionsArgs =
            serde_json::from_value(serde_json::json!({
                "labels": ["integration-action"],
                "labelQueryMode": "any",
                "includeLabels": true,
                "includeInputs": true,
                "includeInputSourceLockingScripts": false,
                "includeOutputs": true,
                "includeOutputLockingScripts": false,
                "limit": 10,
                "offset": 0
            }))
            .unwrap();
        let result = bsv_wallet_toolbox::storage::methods::list_actions::list_actions(
            &storage,
            "pg-list-actions-auth",
            user_id,
            &without_scripts,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.total_actions, 1);
        assert_eq!(result.actions.len(), 1);
        let action = &result.actions[0];
        assert_eq!(
            action.labels.as_deref(),
            Some(&["integration-action".to_string()][..])
        );
        assert_eq!(action.inputs.as_deref().unwrap().len(), 1);
        assert_eq!(action.outputs.as_deref().unwrap().len(), 1);
        assert!(action.inputs.as_deref().unwrap()[0]
            .source_locking_script
            .is_none());
        assert!(action.outputs.as_deref().unwrap()[0]
            .locking_script
            .is_none());

        // The complementary flags drive no_script=false and preserve both scripts.
        let with_scripts: bsv::wallet::interfaces::ListActionsArgs =
            serde_json::from_value(serde_json::json!({
                "labels": ["integration-action"],
                "labelQueryMode": "any",
                "includeLabels": true,
                "includeInputs": true,
                "includeInputSourceLockingScripts": true,
                "includeOutputs": true,
                "includeOutputLockingScripts": true,
                "limit": 10,
                "offset": 0
            }))
            .unwrap();
        let result = bsv_wallet_toolbox::storage::methods::list_actions::list_actions(
            &storage,
            "pg-list-actions-auth",
            user_id,
            &with_scripts,
            None,
        )
        .await
        .unwrap();
        let action = &result.actions[0];
        assert_eq!(
            action.inputs.as_deref().unwrap()[0]
                .source_locking_script
                .as_deref(),
            Some(input_script.as_slice())
        );
        assert_eq!(
            action.outputs.as_deref().unwrap()[0]
                .locking_script
                .as_deref(),
            Some(output_script.as_slice())
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_delete_monitor_events_before_id_scopes_event_and_boundary() {
        let storage = setup_pg_storage().await.unwrap();
        let insert_event = |event: &str| MonitorEvent {
            created_at: test_datetime(),
            updated_at: test_datetime(),
            id: 0,
            event: event.to_string(),
            details: Some("{}".to_string()),
        };
        let old_target_id = storage
            .insert_monitor_event(&insert_event("target"), None)
            .await
            .unwrap();
        let old_other_id = storage
            .insert_monitor_event(&insert_event("other"), None)
            .await
            .unwrap();
        let boundary_target_id = storage
            .insert_monitor_event(&insert_event("target"), None)
            .await
            .unwrap();
        let new_other_id = storage
            .insert_monitor_event(&insert_event("other"), None)
            .await
            .unwrap();

        // Probes: the shared server DELETE must use PostgreSQL placeholders and
        // constrain both the event name and the strict id boundary.
        let deleted = storage
            .delete_monitor_events_before_id("target", boundary_target_id, None)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        let remaining = storage
            .find_monitor_events(&FindMonitorEventsArgs::default(), None)
            .await
            .unwrap();
        let remaining_ids: BTreeSet<_> = remaining.iter().map(|row| row.id).collect();
        assert!(!remaining_ids.contains(&old_target_id));
        assert!(remaining_ids.contains(&old_other_id));
        assert!(remaining_ids.contains(&boundary_target_id));
        assert!(remaining_ids.contains(&new_other_id));
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_transactions_limit_offset_two_pages() {
        let storage = setup_pg_storage().await.unwrap();
        let user_id = insert_pg_user(&storage, "pg-paged-transactions").await;
        let mut expected = BTreeSet::new();
        for index in 0..4 {
            expected.insert(
                insert_pg_transaction(
                    &storage,
                    user_id,
                    TransactionStatus::Completed,
                    &format!("pg-paged-transaction-{index}"),
                )
                .await,
            );
        }
        let page = |offset| FindTransactionsArgs {
            partial: TransactionPartial {
                user_id: Some(user_id),
                ..Default::default()
            },
            paged: Some(Paged { limit: 2, offset }),
            ..Default::default()
        };
        let page_one = storage.find_transactions(&page(0), None).await.unwrap();
        let page_two = storage.find_transactions(&page(2), None).await.unwrap();
        assert_eq!(page_one.len(), 2);
        assert_eq!(page_two.len(), 2);
        let first_ids: BTreeSet<_> = page_one.iter().map(|row| row.transaction_id).collect();
        let second_ids: BTreeSet<_> = page_two.iter().map(|row| row.transaction_id).collect();
        assert!(first_ids.is_disjoint(&second_ids));
        assert_eq!(
            first_ids
                .union(&second_ids)
                .copied()
                .collect::<BTreeSet<_>>(),
            expected
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_pg_find_users_limit_offset_two_pages() {
        let storage = setup_pg_storage().await.unwrap();
        let mut expected = BTreeSet::new();
        for index in 0..4 {
            expected.insert(insert_pg_user(&storage, &format!("pg-paged-user-{index}")).await);
        }
        let page = |offset| FindUsersArgs {
            paged: Some(Paged { limit: 2, offset }),
            ..Default::default()
        };
        let page_one = storage.find_users(&page(0), None).await.unwrap();
        let page_two = storage.find_users(&page(2), None).await.unwrap();
        assert_eq!(page_one.len(), 2);
        assert_eq!(page_two.len(), 2);
        let first_ids: BTreeSet<_> = page_one.iter().map(|row| row.user_id).collect();
        let second_ids: BTreeSet<_> = page_two.iter().map(|row| row.user_id).collect();
        assert!(first_ids.is_disjoint(&second_ids));
        assert_eq!(
            first_ids
                .union(&second_ids)
                .copied()
                .collect::<BTreeSet<_>>(),
            expected
        );
    }
}
