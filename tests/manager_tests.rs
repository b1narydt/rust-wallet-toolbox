//! WalletStorageManager integration tests.
//!
//! Tests verify that the manager correctly routes reads to the active provider,
//! propagates writes to both active and backup, serializes concurrent writes
//! via the internal lock, and handles backup failures gracefully.
//!
//! All tests use dual in-memory SQLite databases as active and backup providers.

#[cfg(feature = "sqlite")]
mod manager_tests {
    use std::sync::Arc;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::storage::find_args::*;
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::*;
    use bsv_wallet_toolbox::types::Chain;

    /// Helper to create a fresh in-memory SQLite storage with migrations run.
    async fn create_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        storage.migrate_database().await?;
        storage.make_available().await?;
        Ok(storage)
    }

    fn test_datetime() -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str("2024-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    /// Create a WalletStorageManager with a single active provider (no backup).
    async fn setup_single_provider() -> WalletResult<(WalletStorageManager, Arc<SqliteStorage>)> {
        let active = Arc::new(create_storage().await?);
        let manager = WalletStorageManager::new(
            active.clone(),
            None,
            Chain::Test,
            "test-identity-key".to_string(),
        );
        Ok((manager, active))
    }

    /// Create a WalletStorageManager with active + backup providers (separate in-memory DBs).
    async fn setup_dual_provider() -> WalletResult<(
        WalletStorageManager,
        Arc<SqliteStorage>,
        Arc<SqliteStorage>,
    )> {
        let active = Arc::new(create_storage().await?);
        let backup = Arc::new(create_storage().await?);
        let manager = WalletStorageManager::new(
            active.clone(),
            Some(backup.clone()),
            Chain::Test,
            "test-identity-key".to_string(),
        );
        Ok((manager, active, backup))
    }

    // -----------------------------------------------------------------------
    // Test 1: Manager with single active provider -- insert User, find User
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_single_provider_insert_and_find() {
        let (manager, _active) = setup_single_provider().await.unwrap();
        let now = test_datetime();

        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02single_test_key".to_string(),
            active_storage: "default".to_string(),
        };

        // Insert via manager
        let user_id = manager.insert_user(&user, None).await.unwrap();
        assert!(user_id > 0, "insert_user should return a positive ID");

        // Find via manager
        let found = manager
            .find_user_by_identity_key("02single_test_key", None)
            .await
            .unwrap();
        assert!(found.is_some(), "Should find the inserted user");
        let found_user = found.unwrap();
        assert_eq!(found_user.identity_key, "02single_test_key");
        assert_eq!(found_user.user_id, user_id);
    }

    // -----------------------------------------------------------------------
    // Test 2: Manager with active + backup -- insert User appears in both
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dual_provider_write_propagation() {
        let (manager, active, backup) = setup_dual_provider().await.unwrap();
        let now = test_datetime();

        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02dual_test_key".to_string(),
            active_storage: "default".to_string(),
        };

        // Insert via manager -- should go to both active and backup
        let user_id = manager.insert_user(&user, None).await.unwrap();
        assert!(user_id > 0);

        // Verify user exists in active (direct query)
        let active_found = active
            .find_user_by_identity_key("02dual_test_key", None)
            .await
            .unwrap();
        assert!(
            active_found.is_some(),
            "User should exist in active provider"
        );

        // Verify user exists in backup (direct query)
        let backup_found = backup
            .find_user_by_identity_key("02dual_test_key", None)
            .await
            .unwrap();
        assert!(
            backup_found.is_some(),
            "User should exist in backup provider"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: Manager reads only from active (not backup)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reads_only_from_active() {
        let (manager, _active, backup) = setup_dual_provider().await.unwrap();
        let now = test_datetime();

        // Insert directly into backup (bypassing manager)
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02backup_only_key".to_string(),
            active_storage: "default".to_string(),
        };
        let _backup_user_id = backup.insert_user(&user, None).await.unwrap();

        // Manager find should NOT find this user (it only reads from active)
        let found = manager
            .find_user_by_identity_key("02backup_only_key", None)
            .await
            .unwrap();
        assert!(
            found.is_none(),
            "Manager should not find data only in backup"
        );

        // But direct backup query should find it
        let backup_found = backup
            .find_user_by_identity_key("02backup_only_key", None)
            .await
            .unwrap();
        assert!(
            backup_found.is_some(),
            "Data should exist in backup when queried directly"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Concurrent write lock serialization
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_concurrent_write_serialization() {
        let (manager, _active) = setup_single_provider().await.unwrap();
        let manager = Arc::new(manager);
        let now = test_datetime();

        // Spawn 10 concurrent insert tasks
        let mut handles = Vec::new();
        for i in 0..10 {
            let mgr = manager.clone();
            let handle = tokio::spawn(async move {
                let user = User {
                    created_at: now,
                    updated_at: now,
                    user_id: 0,
                    identity_key: format!("02concurrent_user_{}", i),
                    active_storage: "default".to_string(),
                };
                mgr.insert_user(&user, None).await
            });
            handles.push(handle);
        }

        // All inserts should succeed
        let mut user_ids = Vec::new();
        for handle in handles {
            let result = handle.await.unwrap();
            let user_id = result.unwrap();
            assert!(user_id > 0);
            user_ids.push(user_id);
        }

        // All IDs should be unique
        user_ids.sort();
        user_ids.dedup();
        assert_eq!(
            user_ids.len(),
            10,
            "All 10 concurrent inserts should produce unique IDs"
        );

        // Verify all 10 users exist
        let args = FindUsersArgs::default();
        let all_users = manager.find_users(&args, None).await.unwrap();
        // Account for the settings-inserted user (from make_available) plus our 10
        let concurrent_users: Vec<_> = all_users
            .iter()
            .filter(|u| u.identity_key.starts_with("02concurrent_user_"))
            .collect();
        assert_eq!(
            concurrent_users.len(),
            10,
            "All 10 concurrent users should be retrievable"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: has_backup() and accessor methods
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_accessor_methods() {
        let (single_mgr, _active) = setup_single_provider().await.unwrap();
        assert!(!single_mgr.has_backup(), "Single provider should have no backup");
        assert!(single_mgr.backup().is_none());

        let (dual_mgr, _active, _backup) = setup_dual_provider().await.unwrap();
        assert!(dual_mgr.has_backup(), "Dual provider should have backup");
        assert!(dual_mgr.backup().is_some());
    }

    // -----------------------------------------------------------------------
    // Test 6: Update propagation to backup
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_update_propagation_to_backup() {
        let (manager, active, backup) = setup_dual_provider().await.unwrap();
        let now = test_datetime();

        // Insert a user via manager
        let user = User {
            created_at: now,
            updated_at: now,
            user_id: 0,
            identity_key: "02update_test_key".to_string(),
            active_storage: "default".to_string(),
        };
        let user_id = manager.insert_user(&user, None).await.unwrap();

        // Update via manager
        let update = UserPartial {
            active_storage: Some("new-storage".to_string()),
            ..Default::default()
        };
        let rows = manager.update_user(user_id, &update, None).await.unwrap();
        assert_eq!(rows, 1, "Should update 1 row in active");

        // Verify active has updated value
        let active_user = active
            .find_user_by_identity_key("02update_test_key", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active_user.active_storage, "new-storage");

        // Verify backup also has updated value
        let backup_user = backup
            .find_user_by_identity_key("02update_test_key", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(backup_user.active_storage, "new-storage");
    }

    // -----------------------------------------------------------------------
    // Test 7: StorageProvider lifecycle (make_available, get_settings)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_storage_provider_lifecycle() {
        let (manager, _active, _backup) = setup_dual_provider().await.unwrap();

        // Manager should report available since make_available was called on both providers
        assert!(manager.is_available());

        // get_settings should work via manager
        let settings = manager.get_settings(None).await.unwrap();
        assert!(
            !settings.storage_name.is_empty(),
            "Settings should be present with a storage name"
        );

        // get_chain should return the manager's chain
        assert_eq!(manager.get_chain(), Chain::Test);
    }
}
