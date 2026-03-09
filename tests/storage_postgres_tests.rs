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
    use chrono::NaiveDateTime;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::PgStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::Chain;

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
}
