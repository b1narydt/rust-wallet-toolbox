//! Tests for storage trait object safety, dual-pool setup, and transaction lifecycle.

#[cfg(feature = "sqlite")]
mod sqlite_tests {
    use std::sync::Arc;

    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::{StorageConfig, StorageProvider};

    /// Test 1: Verify `Arc<dyn StorageProvider>` compiles (object safety).
    /// This function signature proves the trait is object-safe.
    fn _accepts_provider(_p: Arc<dyn StorageProvider>) {}

    #[test]
    fn object_safety_compiles() {
        // If this test compiles, Arc<dyn StorageProvider> is object-safe.
        // We just need the function signature above to exist.
        assert!(true);
    }

    /// Test 3: SqliteStorage::new_sqlite creates successfully with in-memory SQLite.
    #[tokio::test]
    async fn sqlite_storage_creates_in_memory() {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, bsv_wallet_toolbox::types::Chain::Test)
            .await
            .expect("SqliteStorage should create with in-memory URL");

        // Verify both pools are available
        // read_pool() returns a reference to the read pool, which should
        // be the dedicated reader (not the write pool) for SQLite.
        let _rp = storage.read_pool();
        assert!(storage.read_pool.is_some(), "read_pool should be Some for SQLite");
    }

    /// Test 4: Dual-pool SQLite -- begin_transaction returns a TrxToken,
    /// commit_transaction succeeds.
    #[tokio::test]
    async fn sqlite_begin_commit_transaction() {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, bsv_wallet_toolbox::types::Chain::Test)
            .await
            .expect("SqliteStorage should create");

        // Begin a transaction
        let trx = storage
            .begin_sqlite_transaction()
            .await
            .expect("begin_transaction should succeed");

        // Extract and verify it is a valid SQLite transaction
        let inner = SqliteStorage::extract_sqlite_trx(&trx)
            .expect("should extract SQLite trx inner");

        // The inner should contain Some(transaction)
        let guard = inner.lock().await;
        assert!(guard.is_some(), "Transaction should be present after begin");
        drop(guard);

        // Commit (take the transaction out and commit it)
        let inner = trx.downcast::<bsv_wallet_toolbox::storage::sqlx_impl::SqliteTrxInner>()
            .expect("should downcast to SqliteTrxInner");
        let mut guard = inner.lock().await;
        let tx = guard.take().expect("Transaction should be present for commit");
        tx.commit().await.expect("commit should succeed");
    }

    /// Test 5: FindUsersArgs::default() has all None fields.
    #[test]
    fn find_users_args_default_all_none() {
        let args = bsv_wallet_toolbox::storage::FindUsersArgs::default();
        assert!(args.partial.user_id.is_none());
        assert!(args.partial.identity_key.is_none());
        assert!(args.partial.active_storage.is_none());
        assert!(args.since.is_none());
        assert!(args.paged.is_none());
    }
}
