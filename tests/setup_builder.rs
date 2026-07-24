//! Tests for WalletBuilder construction, validation, and identity key derivation.

mod common;

use bsv::primitives::private_key::PrivateKey;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::wallet::setup::WalletBuilder;

#[tokio::test]
async fn test_builder_minimal_config() {
    // Build with chain + root_key + sqlite_memory + mock services
    let setup = common::create_test_wallet().await;

    // Assert wallet is constructed and identity_key is non-empty
    assert!(
        !setup.identity_key.is_empty(),
        "identity_key should be non-empty"
    );
    // Identity key should be a compressed public key hex (66 chars)
    assert_eq!(
        setup.identity_key.len(),
        66,
        "identity_key should be 66 hex chars (compressed pubkey)"
    );

    // Assert chain matches
    assert_eq!(setup.chain, Chain::Test, "chain should be Test");

    // Destroy wallet
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_builder_missing_chain_returns_error() {
    let result = WalletBuilder::new().build().await;
    match result {
        Err(e) => {
            let err = e.to_string();
            assert!(err.contains("chain"), "Expected chain error, got: {err}");
        }
        Ok(_) => panic!("Expected error for missing chain"),
    }
}

#[tokio::test]
async fn test_builder_missing_root_key_returns_error() {
    let result = WalletBuilder::new().chain(Chain::Test).build().await;
    match result {
        Err(e) => {
            let err = e.to_string();
            assert!(
                err.contains("root_key"),
                "Expected root_key error, got: {err}"
            );
        }
        Ok(_) => panic!("Expected error for missing root_key"),
    }
}

#[tokio::test]
async fn test_builder_missing_storage_returns_error() {
    let root_key = PrivateKey::from_hex("aa").unwrap();
    let result = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key)
        .build()
        .await;
    match result {
        Err(e) => {
            let err = e.to_string();
            assert!(
                err.contains("storage"),
                "Expected storage error, got: {err}"
            );
        }
        Ok(_) => panic!("Expected error for missing storage"),
    }
}

#[tokio::test]
async fn test_builder_with_monitor() {
    // DEFAULT (0.3.4): a wallet built with services gets a monitor, STARTED --
    // the toolbox stores createAction txs as `unsent` ProvenTxReqs and only the
    // monitor's TaskSendWaiting ever broadcasts them, so "out of the box" must
    // include a running monitor (rust-mpc#147 phantom-spend regression).
    let root_key = common::random_root_key();
    let setup_default = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key)
        .with_sqlite_memory()
        .with_services(std::sync::Arc::new(common::MockWalletServices))
        .build()
        .await
        .expect("build default");
    let monitor = setup_default
        .monitor
        .as_ref()
        .expect("default build must include a monitor when services exist");
    assert!(
        monitor.is_running(),
        "the default monitor must be STARTED by build(), not left for the caller to remember"
    );

    // Explicit opt-out for callers that own broadcasting themselves.
    let root_key = common::random_root_key();
    let setup_without = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key)
        .with_sqlite_memory()
        .with_services(std::sync::Arc::new(common::MockWalletServices))
        .without_monitor()
        .build()
        .await
        .expect("build without monitor");
    assert!(
        setup_without.monitor.is_none(),
        "without_monitor() must opt out"
    );

    setup_default.wallet.destroy().await.unwrap();
    setup_without.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_builder_identity_key_derives_from_root_key() {
    // Build two wallets with the same root key
    let root_key = PrivateKey::from_hex("aa").unwrap();
    let setup1 = common::create_test_wallet_with_key(root_key.clone()).await;

    let root_key2 = PrivateKey::from_hex("aa").unwrap();
    let setup2 = common::create_test_wallet_with_key(root_key2).await;

    // Identity keys should match for the same root key
    assert_eq!(
        setup1.identity_key, setup2.identity_key,
        "Same root key should produce same identity key"
    );

    // Build third wallet with a different root key
    let different_key = PrivateKey::from_hex("bb").unwrap();
    let setup3 = common::create_test_wallet_with_key(different_key).await;

    // Identity key should differ
    assert_ne!(
        setup1.identity_key, setup3.identity_key,
        "Different root keys should produce different identity keys"
    );

    setup1.wallet.destroy().await.unwrap();
    setup2.wallet.destroy().await.unwrap();
    setup3.wallet.destroy().await.unwrap();
}
