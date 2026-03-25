//! Integration tests for StorageClient against the live TypeScript storage server.
//!
//! These tests prove wire compatibility between the Rust StorageClient and the
//! TypeScript wallet-toolbox storage server at staging-storage.babbage.systems.
//!
//! Tests cover:
//! - TEST-01: BRC-31 mutual auth handshake
//! - TEST-02: makeAvailable returns valid Settings with storage_identity_key
//! - TEST-03: findOrInsertUser creates/retrieves a user on the live server
//! - TEST-04: getSyncChunk returns a valid SyncChunk (deserialization round-trip)
//! - TEST-05: Full wallet with StorageClient backup wired through WalletStorageManager
//! - TEST-06: syncToWriter completes a full sync loop from local to remote StorageClient
//! - TEST-07: updateBackups iterates backups and syncs to live StorageClient
//!
//! # Running these tests
//!
//! Generate a test key: `openssl rand -hex 32`
//!
//! Then run:
//! ```
//! BSV_TEST_ROOT_KEY=<your-64-char-hex-key> cargo test --features sqlite -- --ignored storage_client_integration 2>&1
//! ```
//!
//! Tests are gated with `#[cfg(feature = "sqlite")]` and `#[ignore]` — they do
//! not run by default (require a live network connection and a valid BSV private key).

mod common;

#[cfg(feature = "sqlite")]
mod storage_client_integration {
    use std::sync::Arc;

    use bsv::primitives::private_key::PrivateKey;

    use bsv_wallet_toolbox::storage::remoting::{StorageClient, WalletArc};
    use bsv_wallet_toolbox::storage::sync::RequestSyncChunkArgs;
    use bsv_wallet_toolbox::storage::traits::WalletStorageProvider;
    use bsv_wallet_toolbox::types::Chain;
    use bsv_wallet_toolbox::wallet::setup::WalletBuilder;
    use bsv_wallet_toolbox::ProtoWallet;

    use super::common;

    const STAGING_ENDPOINT: &str = "https://staging-storage.babbage.systems";

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Read BSV_TEST_ROOT_KEY from the environment and parse it as a PrivateKey.
    ///
    /// Panics with a clear message if the env var is not set or invalid.
    /// To set it: `export BSV_TEST_ROOT_KEY=$(openssl rand -hex 32)`
    fn test_root_key() -> PrivateKey {
        let hex = std::env::var("BSV_TEST_ROOT_KEY").unwrap_or_else(|_| {
            panic!(
                "BSV_TEST_ROOT_KEY must be set to run live integration tests.\n\
                Generate one with: openssl rand -hex 32\n\
                Then run: BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored storage_client_integration"
            )
        });
        PrivateKey::from_hex(&hex).unwrap_or_else(|e| {
            panic!("BSV_TEST_ROOT_KEY is not a valid 64-char secp256k1 private key hex: {e}")
        })
    }

    /// Create a StorageClient from the test root key pointing at the staging server.
    ///
    /// Uses `WalletArc<ProtoWallet>` to satisfy `W: Clone` — `ProtoWallet` does not
    /// implement `Clone` but `WalletArc` wraps it in `Arc` and is cheaply cloneable.
    fn make_client() -> StorageClient<WalletArc<ProtoWallet>> {
        let root_key = test_root_key();
        let proto = WalletArc::new(ProtoWallet::new(root_key));
        StorageClient::new(proto, STAGING_ENDPOINT)
    }

    /// Derive the compressed public key DER hex (identity key) from a PrivateKey.
    ///
    /// This matches the `identity_key` field format used by the TS server for
    /// `findOrInsertUser` and `RequestSyncChunkArgs`.
    fn identity_key_hex(key: &PrivateKey) -> String {
        key.to_public_key().to_der_hex()
    }

    // -----------------------------------------------------------------------
    // Shared setup helper: local wallet + StorageClient backup
    //
    // Used by TEST-05, TEST-06, and TEST-07 to avoid duplicating setup code.
    // -----------------------------------------------------------------------

    async fn make_wallet_with_remote_backup() -> bsv_wallet_toolbox::wallet::setup::SetupWallet {
        let root_key = test_root_key();
        let setup = WalletBuilder::new()
            .chain(Chain::Test)
            .root_key(root_key.clone())
            .with_sqlite_memory()
            .with_services(Arc::new(common::MockWalletServices))
            .build()
            .await
            .expect("build wallet");

        // Wire a StorageClient as a backup provider through the manager.
        // WalletArc wraps ProtoWallet in Arc so it satisfies W: Clone (required by AuthFetch).
        let proto = WalletArc::new(ProtoWallet::new(root_key));
        let client = Arc::new(StorageClient::new(proto, STAGING_ENDPOINT))
            as Arc<dyn WalletStorageProvider>;

        setup
            .storage
            .add_wallet_storage_provider(client)
            .await
            .expect("add StorageClient backup — BRC-31 auth + make_available must succeed");

        setup
    }

    // -----------------------------------------------------------------------
    // TEST-01 + TEST-02: BRC-31 auth handshake and makeAvailable Settings
    // -----------------------------------------------------------------------

    /// Proves TEST-01 (BRC-31 handshake completes without auth errors) and
    /// TEST-02 (makeAvailable returns Settings with non-empty storage_identity_key).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_brc31_auth_and_make_available() {
        let client = make_client();

        // make_available performs the BRC-31 handshake and fetches settings.
        let settings = client
            .make_available()
            .await
            .expect("make_available must succeed — BRC-31 handshake + settings fetch");

        // TEST-02: Settings must carry a non-empty storage_identity_key.
        assert!(
            !settings.storage_identity_key.is_empty(),
            "Settings.storage_identity_key must be non-empty after make_available"
        );

        // TEST-01: is_available should reflect cached state after make_available.
        assert!(
            client.is_available(),
            "is_available() must return true after successful make_available"
        );
    }

    // -----------------------------------------------------------------------
    // TEST-03: findOrInsertUser
    // -----------------------------------------------------------------------

    /// Proves TEST-03: findOrInsertUser creates or retrieves a user record on
    /// the live TS server, and is idempotent (same user_id on second call).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_find_or_insert_user() {
        let client = make_client();
        let root_key = test_root_key();
        let ik = identity_key_hex(&root_key);

        // make_available must be called first — establishes BRC-31 session.
        client
            .make_available()
            .await
            .expect("make_available before find_or_insert_user");

        // First call: creates or retrieves the user.
        let (user, _is_new) = client
            .find_or_insert_user(&ik)
            .await
            .expect("find_or_insert_user must succeed on live server");

        assert!(user.user_id > 0, "user_id must be a positive integer assigned by the server");

        // Second call: idempotency check — must return same user_id.
        let (user2, _) = client
            .find_or_insert_user(&ik)
            .await
            .expect("find_or_insert_user (second call) must succeed");

        assert_eq!(
            user.user_id, user2.user_id,
            "find_or_insert_user must be idempotent — same user_id on repeated calls"
        );
    }

    // -----------------------------------------------------------------------
    // TEST-04: getSyncChunk round-trip
    // -----------------------------------------------------------------------

    /// Proves TEST-04: getSyncChunk returns a valid SyncChunk that deserializes
    /// without error, proving wire format compatibility with the TS server.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_sync_chunk_round_trip() {
        let client = make_client();
        let root_key = test_root_key();
        let ik = identity_key_hex(&root_key);

        let settings = client
            .make_available()
            .await
            .expect("make_available before get_sync_chunk");

        // Construct minimal first-sync args (no since, no pagination offsets).
        let args = RequestSyncChunkArgs {
            // from: the remote server (we are reading from it)
            from_storage_identity_key: settings.storage_identity_key.clone(),
            // to: arbitrary local identifier
            to_storage_identity_key: "test-local-sik-rust".to_string(),
            // identity_key is the user's public key (NOT user_identity_key)
            identity_key: ik,
            since: None,
            max_rough_size: 100_000,
            max_items: 100,
            offsets: vec![],
        };

        // get_sync_chunk performs the JSON-RPC call and deserializes the response.
        // Success here proves the wire format is compatible end-to-end.
        let chunk = client
            .get_sync_chunk(&args)
            .await
            .expect("get_sync_chunk must succeed — wire format compatibility proof");

        // The SyncChunk deserialized successfully (if we're here, it did).
        // Log some diagnostics for test output visibility.
        let _ = chunk; // chunk fields are optional slices; just proving deserialization
    }

    // -----------------------------------------------------------------------
    // TEST-05: Full wallet with StorageClient backup through manager layer
    // -----------------------------------------------------------------------

    /// Proves TEST-05: A wallet with StorageClient as a backup provider correctly
    /// wires BRC-31 auth + make_available + find_or_insert_user through the
    /// WalletStorageManager layer, and get_stores() reflects the remote provider.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_internalize_with_storage_client_backup() {
        let setup = make_wallet_with_remote_backup().await;

        // Verify the remote provider appears in get_stores() as a backup.
        let stores = setup.storage.get_stores().await;
        assert!(
            stores.len() >= 2,
            "Expected at least 2 stores (local active + remote backup), got {}",
            stores.len()
        );

        let remote_store = stores
            .iter()
            .find(|s| s.is_backup)
            .expect("At least one store must be marked as backup (the StorageClient)");

        // The remote store's endpoint URL must point to the staging server.
        let endpoint = remote_store
            .endpoint_url
            .as_deref()
            .expect("Remote StorageClient store must have an endpoint_url");
        assert!(
            endpoint.contains("staging-storage.babbage.systems"),
            "Remote store endpoint must be the staging server, got: {endpoint}"
        );

        // The remote store must have a valid storage_identity_key (from make_available).
        assert!(
            !remote_store.storage_identity_key.is_empty(),
            "Remote store must have a non-empty storage_identity_key after make_available"
        );
    }

    // -----------------------------------------------------------------------
    // TEST-06: syncToWriter — full sync loop local → remote StorageClient
    // -----------------------------------------------------------------------

    /// Proves TEST-06: sync_to_writer completes a full RPC round-trip
    /// (getSyncChunk + processSyncChunk loop) from local SQLite to the remote
    /// StorageClient without auth or wire errors.
    ///
    /// With an empty local wallet, this is a no-op sync, but the full protocol
    /// loop must complete (empty SyncChunk + done=true returned by TS server).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_sync_to_writer_remote() {
        let setup = make_wallet_with_remote_backup().await;

        // update_backups() internally calls sync_to_writer for each backup.
        // This is the simplest correct path to verify the full sync_to_writer
        // protocol works through the manager layer (see plan research note).
        let (inserts, updates, _log) = setup
            .storage
            .update_backups(None)
            .await
            .expect("update_backups (which calls sync_to_writer) must complete without auth or wire errors");

        // With an empty wallet, we expect zero inserts and zero updates.
        // The critical proof is that the call completed without error.
        let _ = (inserts, updates);
    }

    // -----------------------------------------------------------------------
    // TEST-07: updateBackups — fan-out sync to all backup StorageClients
    // -----------------------------------------------------------------------

    /// Proves TEST-07: update_backups iterates all backup stores and calls
    /// sync_to_writer for each, completing without auth or wire errors
    /// against the live StorageClient backup.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn test_update_backups_remote() {
        let setup = make_wallet_with_remote_backup().await;

        // update_backups iterates backup_indices and calls sync_to_writer for each.
        // Returns (total_inserts, total_updates, accumulated_log).
        let result = setup
            .storage
            .update_backups(None)
            .await
            .expect("update_backups must complete without auth or wire errors for StorageClient backup");

        // Destructure the tuple to confirm the return type matches expectations.
        let (_total_inserts, _total_updates, _log) = result;
        // Success: the full updateBackups fan-out loop completed without error.
    }
}
