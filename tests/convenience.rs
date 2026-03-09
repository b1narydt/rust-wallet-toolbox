//! Tests for convenience methods on Wallet: balance, balance_and_utxos,
//! set_wallet_change_params, list_no_send_actions, list_failed_actions, admin_stats.
//!
//! sweep_to is not tested here because it requires full createAction/internalizeAction
//! flow that cannot be mocked at this level without significant mock infrastructure.

mod common;

#[tokio::test]
async fn test_balance_fresh_wallet_returns_zero() {
    let setup = common::create_test_wallet().await;
    let balance = setup.wallet.balance(None).await.unwrap();
    assert_eq!(balance, 0, "Fresh wallet should have zero balance");
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_balance_after_seeding_outputs() {
    let setup = common::create_test_wallet().await;

    // Seed 3 outputs with 1000 satoshis each
    common::seed_outputs(&setup.storage, &setup.identity_key, 3, 1000).await;

    let balance = setup.wallet.balance(None).await.unwrap();
    assert_eq!(
        balance, 3000,
        "Balance should be 3000 after seeding 3 x 1000"
    );
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_balance_and_utxos_returns_details() {
    let setup = common::create_test_wallet().await;

    // Seed 2 outputs with 500 satoshis each
    common::seed_outputs(&setup.storage, &setup.identity_key, 2, 500).await;

    let wb = setup.wallet.balance_and_utxos(None).await.unwrap();
    assert_eq!(wb.total, 1000, "Total should be 1000");
    assert_eq!(wb.utxos.len(), 2, "Should have 2 utxos");
    for utxo in &wb.utxos {
        assert_eq!(utxo.satoshis, 500, "Each utxo should be 500 satoshis");
    }
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_set_wallet_change_params() {
    let setup = common::create_test_wallet().await;

    // Should complete without error (creates/updates basket params)
    let result = setup.wallet.set_wallet_change_params(5, 2000).await;
    assert!(
        result.is_ok(),
        "set_wallet_change_params should succeed: {:?}",
        result.err()
    );
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_list_no_send_actions_empty() {
    let setup = common::create_test_wallet().await;

    let result = setup.wallet.list_no_send_actions(false).await.unwrap();
    assert_eq!(
        result.total_actions, 0,
        "Fresh wallet should have no nosend actions"
    );
    assert!(result.actions.is_empty(), "Actions list should be empty");
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_list_failed_actions_empty() {
    let setup = common::create_test_wallet().await;

    let result = setup.wallet.list_failed_actions(false).await.unwrap();
    assert_eq!(
        result.total_actions, 0,
        "Fresh wallet should have no failed actions"
    );
    assert!(result.actions.is_empty(), "Actions list should be empty");
    setup.wallet.destroy().await.unwrap();
}

#[tokio::test]
async fn test_admin_stats_returns_not_implemented() {
    let setup = common::create_test_wallet().await;

    // admin_stats has a default NotImplemented stub in StorageReaderWriter
    let result = setup.wallet.admin_stats().await;
    assert!(
        result.is_err(),
        "admin_stats should return NotImplemented error from default storage"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("admin_stats") || err.contains("NotImplemented"),
        "Error should mention admin_stats or NotImplemented, got: {}",
        err
    );
    setup.wallet.destroy().await.unwrap();
}
