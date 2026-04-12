//! Tests for storage trait object safety, dual-pool setup, and transaction lifecycle.

#[cfg(feature = "sqlite")]
mod sqlite_tests {
    use std::sync::Arc;

    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::{StorageConfig, StorageProvider, WalletStorageProvider};

    /// Test 1: Verify `Arc<dyn StorageProvider>` compiles (object safety).
    /// This function signature proves the trait is object-safe.
    fn _accepts_provider(_p: Arc<dyn StorageProvider>) {}

    #[test]
    fn object_safety_compiles() {
        // If this test compiles, `Arc<dyn StorageProvider>` is object-safe.
        // The `_accepts_sp`/`_accepts_dsp` function signatures above are the
        // actual compile-time assertion; this function only needs to exist.
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
        assert!(
            storage.read_pool.is_some(),
            "read_pool should be Some for SQLite"
        );
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
        let inner =
            SqliteStorage::extract_sqlite_trx(&trx).expect("should extract SQLite trx inner");

        // The inner should contain Some(transaction)
        let guard = inner.lock().await;
        assert!(guard.is_some(), "Transaction should be present after begin");
        drop(guard);

        // Commit (take the transaction out and commit it)
        let inner = trx
            .downcast::<bsv_wallet_toolbox::storage::sqlx_impl::SqliteTrxInner>()
            .expect("should downcast to SqliteTrxInner");
        let mut guard = inner.lock().await;
        let tx = guard
            .take()
            .expect("Transaction should be present for commit");
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

    // -----------------------------------------------------------------------
    // WalletStorageProvider trait tests (Phase 2)
    // -----------------------------------------------------------------------

    /// Test: Arc<dyn WalletStorageProvider> compiles (object safety).
    fn _accepts_wsp(_p: Arc<dyn WalletStorageProvider>) {}

    #[test]
    fn wallet_storage_provider_object_safe() {
        // If this test compiles, `Arc<dyn WalletStorageProvider>` is
        // object-safe. The `_accepts_wsp` signature above carries the
        // actual compile-time assertion.
    }

    /// Test: a function accepting &dyn WalletStorageProvider compiles when passed &SqliteStorage.
    fn _accepts_wsp_ref(_p: &dyn WalletStorageProvider) {}

    #[tokio::test]
    async fn sqlite_storage_satisfies_wallet_provider() {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, bsv_wallet_toolbox::types::Chain::Test)
            .await
            .expect("SqliteStorage should create");
        // If this compiles, the blanket impl works.
        _accepts_wsp_ref(&storage);
    }

    /// Test: is_storage_provider() returns true via blanket impl.
    #[tokio::test]
    async fn is_storage_provider_returns_true() {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, bsv_wallet_toolbox::types::Chain::Test)
            .await
            .expect("SqliteStorage should create");
        StorageProvider::make_available(&storage)
            .await
            .expect("make_available should succeed");
        assert!(
            storage.is_storage_provider(),
            "blanket impl should return true"
        );
    }
}
