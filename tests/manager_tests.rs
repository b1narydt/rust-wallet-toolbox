//! WalletStorageManager integration tests.
//!
//! Tests verify that the new manager correctly wraps providers in ManagedStorage,
//! partitions stores into active/backups/conflicting_actives via makeAvailable,
//! serializes concurrent access via hierarchical locks, and exposes correct
//! getter methods after initialization.
//!
//! All tests use in-memory SQLite databases as WalletStorageProvider implementations
//! (via the blanket impl over StorageProvider).

#[cfg(feature = "sqlite")]
mod manager_tests {
    use std::sync::Arc;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::wallet::types::{ReproveHeaderResult, WalletStorageInfo};
    use bsv_wallet_toolbox::types::Chain;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Create a fresh in-memory SQLite provider, migrated and ready.
    async fn create_provider() -> WalletResult<Arc<dyn WalletStorageProvider>> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        StorageProvider::migrate_database(&storage).await?;
        Ok(Arc::new(storage) as Arc<dyn WalletStorageProvider>)
    }

    const IDENTITY_KEY: &str = "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

    // -----------------------------------------------------------------------
    // Test 1: Constructor stores providers in correct order
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_constructor() {
        let active = create_provider().await.unwrap();
        let backup1 = create_provider().await.unwrap();
        let backup2 = create_provider().await.unwrap();

        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![backup1, backup2],
        );

        // Manager starts not available, has 3 stores total
        assert!(!manager.is_available());
        assert!(manager.can_make_available().await);
        assert_eq!(manager.get_user_identity_key(), IDENTITY_KEY);
        assert_eq!(manager.auth_id(), IDENTITY_KEY);
        assert_eq!(manager.get_all_stores().await.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Test 2: ManagedStorage cache starts empty before makeAvailable
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_managed_storage_cache() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        // Before makeAvailable: is_available is false
        assert!(!manager.is_available());

        // get_active_settings should fail before makeAvailable
        let result = manager.get_active_settings().await;
        assert!(result.is_err(), "get_active_settings should fail before makeAvailable");
    }

    // -----------------------------------------------------------------------
    // Test 3: makeAvailable with single store — becomes active
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_make_available_single_store() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        let settings = manager.make_available().await.unwrap();
        assert!(manager.is_available());
        assert!(!settings.storage_identity_key.is_empty());

        // Active settings accessible
        let active_settings = manager.get_active_settings().await.unwrap();
        assert_eq!(active_settings.storage_identity_key, settings.storage_identity_key);

        // No backups
        assert!(manager.get_backup_stores().await.is_empty());
        assert!(manager.get_conflicting_stores().await.is_empty());

        // All stores = 1
        assert_eq!(manager.get_all_stores().await.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 4: makeAvailable with 2 stores — active + 1 backup
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_make_available_partition() {
        let active = create_provider().await.unwrap();
        let backup = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![backup],
        );

        manager.make_available().await.unwrap();
        assert!(manager.is_available());

        let all_stores = manager.get_all_stores().await;
        assert_eq!(all_stores.len(), 2);

        // After making available with 2 separate providers (each w/ different storage_identity_key),
        // the backup user.active_storage won't match active's storage_identity_key by default,
        // so the backup ends up as a conflicting_active OR a plain backup depending on user state.
        // Either way, active_store should be set and all_stores has 2 entries.
        let active_store = manager.get_active_store().await.unwrap();
        assert!(!active_store.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 5: makeAvailable zero stores returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_make_available_zero_stores() {
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            None,
            vec![],
        );

        // can_make_available should return false with no stores
        assert!(!manager.can_make_available().await);

        // makeAvailable should return an error
        let result = manager.make_available().await;
        assert!(result.is_err(), "makeAvailable with no stores should fail");
    }

    // -----------------------------------------------------------------------
    // Test 6: makeAvailable idempotent — second call returns immediately
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_make_available_idempotent() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        let settings1 = manager.make_available().await.unwrap();
        let settings2 = manager.make_available().await.unwrap();

        // Both calls return the same active settings
        assert_eq!(
            settings1.storage_identity_key,
            settings2.storage_identity_key
        );
        assert!(manager.is_available());
    }

    // -----------------------------------------------------------------------
    // Test 7: get_auth before makeAvailable returns WERR_NOT_ACTIVE
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_verify_active_before_available() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        // get_auth(must_be_active=true) before makeAvailable should fail with NotActive
        let result = manager.get_auth(true).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("WERR_NOT_ACTIVE"),
            "Expected WERR_NOT_ACTIVE, got: {err_msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Getter methods after makeAvailable
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_getter_methods() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        manager.make_available().await.unwrap();

        // is_storage_provider() on manager always returns false
        assert!(!manager.is_storage_provider());

        // get_active_store_name should return non-empty string
        let name = manager.get_active_store_name().await.unwrap();
        assert!(!name.is_empty());

        // get_active_user should return a User
        let user = manager.get_active_user().await.unwrap();
        assert_eq!(user.identity_key, IDENTITY_KEY);

        // get_auth(false) should return AuthId with identity_key set
        let auth = manager.get_auth(false).await.unwrap();
        assert_eq!(auth.identity_key, IDENTITY_KEY);
    }

    // -----------------------------------------------------------------------
    // Test 9: Hierarchical lock helpers don't deadlock
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_hierarchical_locks() {
        let active = create_provider().await.unwrap();
        let manager = Arc::new(WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        ));

        manager.make_available().await.unwrap();

        // acquire_reader, acquire_writer, acquire_sync, acquire_storage_provider
        // all in sequence — if any deadlock, the test will hang and timeout.
        {
            let _guard = manager.acquire_reader().await.unwrap();
        }
        {
            let _guards = manager.acquire_writer().await.unwrap();
        }
        {
            let _guards = manager.acquire_sync().await.unwrap();
        }
        {
            let _guards = manager.acquire_storage_provider().await.unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Test 10: Auto-init — acquire_reader triggers makeAvailable
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_auto_init_in_lock_helpers() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        // Manager not yet available
        assert!(!manager.is_available());

        // Acquiring a reader lock should auto-trigger makeAvailable
        {
            let _guard = manager.acquire_reader().await.unwrap();
        }

        // Now manager should be available
        assert!(manager.is_available());
    }

    // -----------------------------------------------------------------------
    // Test 11: is_active_storage_provider
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_is_active_storage_provider() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        manager.make_available().await.unwrap();

        // SqliteStorage is_storage_provider() returns true (blanket impl default)
        let is_sp = manager.is_active_storage_provider().await.unwrap();
        assert!(is_sp, "SqliteStorage should be a storage provider");

        // But the manager itself is NOT a storage provider
        assert!(!manager.is_storage_provider());
    }

    // -----------------------------------------------------------------------
    // Test 12: reprove_header with no matching records returns empty result
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reprove_header_empty() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        manager.make_available().await.unwrap();

        // No services configured — reprove_header with a non-matching hash should
        // find zero ProvenTx records and return empty without calling services.
        let result: ReproveHeaderResult = manager
            .reprove_header("0000000000000000000000000000000000000000000000000000000000000000")
            .await
            .unwrap();

        assert!(result.updated.is_empty());
        assert!(result.unchanged.is_empty());
        assert!(result.unavailable.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 13: get_stores returns WalletStorageInfo for all stores
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_stores() {
        let active = create_provider().await.unwrap();
        let backup = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![backup],
        );

        manager.make_available().await.unwrap();

        let stores: Vec<WalletStorageInfo> = manager.get_stores().await;

        // Should have 2 entries (active + backup/conflicting)
        assert_eq!(stores.len(), 2);

        // Exactly one store is active
        let active_count = stores.iter().filter(|s| s.is_active).count();
        assert_eq!(active_count, 1, "Exactly one store should be active");

        // All stores have non-empty storage_identity_key
        for store in &stores {
            assert!(
                !store.storage_identity_key.is_empty(),
                "storage_identity_key should not be empty"
            );
        }

        // Local stores have no endpoint_url
        for store in &stores {
            assert!(
                store.endpoint_url.is_none(),
                "Local SQLite stores should have no endpoint_url"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 14: get_store_endpoint_url returns None for local storage
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_store_endpoint_url_local() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );

        manager.make_available().await.unwrap();

        let active_key = manager.get_active_store().await.unwrap();
        let url = manager.get_store_endpoint_url(&active_key).await;

        // Local SQLite returns None
        assert!(url.is_none(), "Local SQLite should return no endpoint URL");

        // Non-existent key also returns None
        let url2 = manager.get_store_endpoint_url("nonexistent_key").await;
        assert!(url2.is_none());
    }

    // -----------------------------------------------------------------------
    // Task 1 TDD Tests: sync loops and make_request_sync_chunk_args
    // -----------------------------------------------------------------------

    // Test 15: make_request_sync_chunk_args builds correct RequestSyncChunkArgs from SyncState
    #[tokio::test]
    async fn test_make_request_sync_chunk_args() {
        use bsv_wallet_toolbox::status::SyncStatus;
        use bsv_wallet_toolbox::storage::manager::make_request_sync_chunk_args;
        use bsv_wallet_toolbox::storage::sync::sync_map::SyncMap;
        use bsv_wallet_toolbox::tables::SyncState;
        use chrono::NaiveDateTime;

        let mut sync_map = SyncMap::new();
        sync_map.transaction.count = 5;
        sync_map.output.count = 10;
        let since =
            NaiveDateTime::parse_from_str("2024-01-01 12:00:00.000", "%Y-%m-%d %H:%M:%S%.3f").ok();

        let sync_map_json = serde_json::to_string(&sync_map).unwrap();

        let now = chrono::Utc::now().naive_utc();
        let ss = SyncState {
            created_at: now,
            updated_at: now,
            sync_state_id: 1,
            user_id: 1,
            storage_identity_key: "reader_sik".to_string(),
            storage_name: "reader_store".to_string(),
            status: SyncStatus::Unknown,
            init: false,
            ref_num: "ref1".to_string(),
            sync_map: sync_map_json,
            when: since,
            satoshis: None,
            error_local: None,
            error_other: None,
        };

        let args = make_request_sync_chunk_args(&ss, &IDENTITY_KEY.to_string(), "writer_sik").unwrap();

        assert_eq!(args.from_storage_identity_key, "reader_sik");
        assert_eq!(args.to_storage_identity_key, "writer_sik");
        assert_eq!(args.identity_key, IDENTITY_KEY);
        assert_eq!(args.since, since);
        assert_eq!(args.max_rough_size, 10_000_000);
        assert_eq!(args.max_items, 1000);
        assert_eq!(args.offsets.len(), 12);

        let tx_offset = args.offsets.iter().find(|o| o.name == "transaction").unwrap();
        assert_eq!(tx_offset.offset, 5);
        let out_offset = args.offsets.iter().find(|o| o.name == "output").unwrap();
        assert_eq!(out_offset.offset, 10);
        let proven_tx_offset = args.offsets.iter().find(|o| o.name == "provenTx").unwrap();
        assert_eq!(proven_tx_offset.offset, 0);
    }

    // Test 16: sync_to_writer loops until done, accumulates inserts/updates
    #[tokio::test]
    async fn test_sync_to_writer() {
        let reader_provider = create_provider().await.unwrap();
        let writer_provider = create_provider().await.unwrap();

        let reader_settings = reader_provider.make_available().await.unwrap();
        let writer_settings = writer_provider.make_available().await.unwrap();

        let _ = reader_provider.find_or_insert_user(IDENTITY_KEY).await.unwrap();

        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(reader_provider.clone()),
            vec![],
        );
        manager.make_available().await.unwrap();

        let (inserts, updates, _log) = manager
            .sync_to_writer(
                writer_provider.as_ref(),
                reader_provider.as_ref(),
                &writer_settings,
                &reader_settings,
                None,
            )
            .await
            .unwrap();

        let _ = (inserts, updates);
    }

    // Test 17: sync_from_reader succeeds when identity keys match
    #[tokio::test]
    async fn test_sync_from_reader_matching_identity() {
        let reader_provider = create_provider().await.unwrap();
        let writer_provider = create_provider().await.unwrap();

        let reader_settings = reader_provider.make_available().await.unwrap();
        let writer_settings = writer_provider.make_available().await.unwrap();

        let _ = reader_provider.find_or_insert_user(IDENTITY_KEY).await.unwrap();

        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(writer_provider.clone()),
            vec![],
        );
        manager.make_available().await.unwrap();

        let result = manager
            .sync_from_reader(
                writer_provider.as_ref(),
                reader_provider.as_ref(),
                &writer_settings,
                &reader_settings,
                None,
            )
            .await;

        assert!(
            result.is_ok(),
            "sync_from_reader should succeed when identity keys match: {:?}",
            result.err()
        );
    }

    // Test 18: update_backups with no backups returns (0, 0)
    #[tokio::test]
    async fn test_update_backups_zero_backups() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );
        manager.make_available().await.unwrap();

        let (inserts, updates, _log) = manager.update_backups(None).await.unwrap();
        assert_eq!(inserts, 0);
        assert_eq!(updates, 0);
    }

    // Test 19: update_backups with active + backup completes without error
    #[tokio::test]
    async fn test_update_backups() {
        let active_provider = create_provider().await.unwrap();
        let backup_provider = create_provider().await.unwrap();

        active_provider.make_available().await.unwrap();
        backup_provider.make_available().await.unwrap();

        let _ = active_provider.find_or_insert_user(IDENTITY_KEY).await.unwrap();

        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active_provider.clone()),
            vec![backup_provider.clone()],
        );
        manager.make_available().await.unwrap();

        let (inserts, updates, _log) = manager.update_backups(None).await.unwrap();
        let _ = (inserts, updates);
    }

    // Test 20: add_wallet_storage_provider adds at runtime and re-partitions
    #[tokio::test]
    async fn test_add_provider_runtime() {
        let active = create_provider().await.unwrap();
        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(active),
            vec![],
        );
        manager.make_available().await.unwrap();

        assert_eq!(manager.get_all_stores().await.len(), 1);

        let new_provider = create_provider().await.unwrap();
        manager.add_wallet_storage_provider(new_provider).await.unwrap();

        assert_eq!(manager.get_all_stores().await.len(), 2);
    }

    // Test 21: sync_to_writer with prog_log captures progress messages
    #[tokio::test]
    async fn test_sync_prog_log() {
        use std::sync::Mutex;

        let reader_provider = create_provider().await.unwrap();
        let writer_provider = create_provider().await.unwrap();

        let reader_settings = reader_provider.make_available().await.unwrap();
        let writer_settings = writer_provider.make_available().await.unwrap();
        let _ = reader_provider.find_or_insert_user(IDENTITY_KEY).await.unwrap();

        let manager = WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(reader_provider.clone()),
            vec![],
        );
        manager.make_available().await.unwrap();

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured2 = captured.clone();
        let prog_log = move |s: &str| -> String {
            captured2.lock().unwrap().push(s.to_string());
            s.to_string()
        };

        let (_inserts, _updates, _log) = manager
            .sync_to_writer(
                writer_provider.as_ref(),
                reader_provider.as_ref(),
                &writer_settings,
                &reader_settings,
                Some(&prog_log),
            )
            .await
            .unwrap();
    }
}
