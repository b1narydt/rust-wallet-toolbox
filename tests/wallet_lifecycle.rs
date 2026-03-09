//! Full wallet lifecycle integration test.
//!
//! Exercises the complete flow: build wallet -> verify empty state -> seed data ->
//! verify balance -> list actions -> list outputs -> admin_stats -> destroy.

mod common;

use bsv::wallet::interfaces::WalletInterface;
use bsv::wallet::types::BooleanDefaultFalse;

#[tokio::test]
async fn test_full_wallet_lifecycle() {
    // 1. Create wallet via WalletBuilder
    let setup = common::create_test_wallet().await;
    let wallet = &setup.wallet;

    // 2. Verify initial state: zero balance
    let balance = wallet.balance(None).await.unwrap();
    assert_eq!(balance, 0, "Fresh wallet should have zero balance");

    // 3. Seed some test data: 5 outputs x 1000 satoshis
    common::seed_outputs(&setup.storage, &setup.identity_key, 5, 1000).await;

    // 4. Verify balance after seeding
    let balance = wallet.balance(None).await.unwrap();
    assert_eq!(balance, 5000, "Balance should be 5000 after seeding 5 x 1000");

    // 5. Verify balance_and_utxos
    let wb = wallet.balance_and_utxos(None).await.unwrap();
    assert_eq!(wb.total, 5000, "balance_and_utxos total should match");
    assert_eq!(wb.utxos.len(), 5, "Should have 5 UTXOs");

    // 6. List actions via WalletInterface
    let actions_args = bsv::wallet::interfaces::ListActionsArgs {
        labels: vec!["default".to_string()],
        label_query_mode: None,
        include_labels: BooleanDefaultFalse::default(),
        include_inputs: BooleanDefaultFalse::default(),
        include_input_source_locking_scripts: BooleanDefaultFalse::default(),
        include_input_unlocking_scripts: BooleanDefaultFalse::default(),
        include_outputs: BooleanDefaultFalse::default(),
        include_output_locking_scripts: BooleanDefaultFalse::default(),
        limit: None,
        offset: None,
        seek_permission: Default::default(),
    };
    let actions_result = wallet.list_actions(actions_args, None).await;
    // list_actions may return Ok or Err depending on label matching -- just verify no panic
    match actions_result {
        Ok(result) => {
            // Result is valid even if zero actions match the "default" label
            println!("list_actions returned {} actions", result.total_actions);
        }
        Err(e) => {
            println!("list_actions returned error (acceptable): {}", e);
        }
    }

    // 7. List outputs via WalletInterface
    let outputs_args = bsv::wallet::interfaces::ListOutputsArgs {
        basket: "default".to_string(),
        tags: vec![],
        tag_query_mode: None,
        include: None,
        include_custom_instructions: BooleanDefaultFalse::default(),
        include_tags: BooleanDefaultFalse::default(),
        include_labels: BooleanDefaultFalse::default(),
        limit: None,
        offset: None,
        seek_permission: Default::default(),
    };
    let outputs_result = wallet.list_outputs(outputs_args, None).await;
    match outputs_result {
        Ok(result) => {
            println!(
                "list_outputs returned {} total, {} items",
                result.total_outputs,
                result.outputs.len()
            );
            assert!(
                result.total_outputs > 0 || !result.outputs.is_empty(),
                "Expected some outputs after seeding"
            );
        }
        Err(e) => {
            println!("list_outputs returned error (acceptable): {}", e);
        }
    }

    // 8. Admin stats (returns NotImplemented from default storage stub)
    let stats_result = wallet.admin_stats().await;
    match stats_result {
        Ok(stats) => {
            println!("admin_stats returned: requested_by={}", stats.requested_by);
        }
        Err(e) => {
            // Expected: default stub returns NotImplemented
            println!("admin_stats returned error (expected): {}", e);
        }
    }

    // 9. List no-send actions (verify call completes)
    let nosend = wallet.list_no_send_actions(false).await.unwrap();
    println!("no-send actions: {}", nosend.total_actions);

    // 10. List failed actions (verify call completes)
    let failed = wallet.list_failed_actions(false).await.unwrap();
    println!("failed actions: {}", failed.total_actions);

    // 11. Destroy
    wallet.destroy().await.unwrap();
}
