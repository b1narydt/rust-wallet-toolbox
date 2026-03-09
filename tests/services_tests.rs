//! Integration tests for Services construction, nLockTimeIsFinal, and hash_output_script.
//!
//! Tests construction correctness, pure logic functions, and configuration defaults.
//! Does NOT make real HTTP calls -- focuses on construction and local logic.

/// Decode hex string to bytes (test helper).
fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

use bsv_wallet_toolbox::services::services::Services;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types::{NLockTimeInput, ServicesConfig};
use bsv_wallet_toolbox::types::Chain;

// ---------------------------------------------------------------------------
// Construction tests
// ---------------------------------------------------------------------------

#[test]
fn test_services_from_chain_main_does_not_panic() {
    let services = Services::from_chain(Chain::Main);
    assert_eq!(services.config().chain, Chain::Main);
}

#[test]
fn test_services_from_chain_test_does_not_panic() {
    let services = Services::from_chain(Chain::Test);
    assert_eq!(services.config().chain, Chain::Test);
}

#[test]
fn test_services_config_from_main_has_correct_urls() {
    let config = ServicesConfig::from(Chain::Main);
    assert_eq!(config.chain, Chain::Main);
    assert_eq!(config.arc_url, "https://arc.taal.com");
    assert_eq!(
        config.arc_gorilla_pool_url.as_deref(),
        Some("https://arc.gorillapool.io")
    );
    assert_eq!(
        config.chaintracks_url.as_deref(),
        Some("https://mainnet-chaintracks.babbage.systems")
    );
}

#[test]
fn test_services_config_from_test_has_correct_urls() {
    let config = ServicesConfig::from(Chain::Test);
    assert_eq!(config.chain, Chain::Test);
    assert_eq!(config.arc_url, "https://arc-test.taal.com");
    assert!(
        config.arc_gorilla_pool_url.is_none(),
        "Test chain should not have GorillaPool"
    );
    assert_eq!(
        config.chaintracks_url.as_deref(),
        Some("https://testnet-chaintracks.babbage.systems")
    );
}

#[test]
fn test_services_config_default_timeouts() {
    let config = ServicesConfig::from(Chain::Main);
    assert_eq!(config.post_beef_soft_timeout_ms, 5000);
    assert_eq!(config.post_beef_soft_timeout_per_kb_ms, 50);
    assert_eq!(config.post_beef_soft_timeout_max_ms, 30000);
    assert_eq!(config.bsv_update_msecs, 15 * 60 * 1000);
    assert_eq!(config.fiat_update_msecs, 24 * 60 * 60 * 1000);
}

#[test]
fn test_services_config_adaptive_timeout_basic() {
    let config = ServicesConfig::from(Chain::Main);
    // 0 bytes -> base timeout
    let timeout = config.get_post_beef_soft_timeout_ms(0);
    assert_eq!(timeout, 5000);
}

#[test]
fn test_services_config_adaptive_timeout_large_beef() {
    let config = ServicesConfig::from(Chain::Main);
    // 100KB -> base(5000) + (100 * 50) = 10000, capped at 30000
    let timeout = config.get_post_beef_soft_timeout_ms(100 * 1024);
    assert_eq!(timeout, 10000);
}

#[test]
fn test_services_config_adaptive_timeout_capped() {
    let config = ServicesConfig::from(Chain::Main);
    // Very large -> capped at max
    let timeout = config.get_post_beef_soft_timeout_ms(1024 * 1024);
    assert_eq!(timeout, 30000);
}

// ---------------------------------------------------------------------------
// nLockTimeIsFinal tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_n_lock_time_is_final_zero() {
    let services = Services::from_chain(Chain::Main);
    let result = services
        .n_lock_time_is_final(NLockTimeInput::Raw(0))
        .await
        .unwrap();
    assert!(result, "locktime 0 should always be final");
}

#[tokio::test]
async fn test_n_lock_time_is_final_timestamp_past() {
    let services = Services::from_chain(Chain::Main);
    // A unix timestamp well in the past (Jan 1 2020)
    let result = services
        .n_lock_time_is_final(NLockTimeInput::Raw(1577836800))
        .await
        .unwrap();
    assert!(result, "past timestamp locktime should be final");
}

#[tokio::test]
async fn test_n_lock_time_is_final_timestamp_future() {
    let services = Services::from_chain(Chain::Main);
    // A unix timestamp far in the future (year 2040)
    let result = services
        .n_lock_time_is_final(NLockTimeInput::Raw(2208988800))
        .await
        .unwrap();
    assert!(!result, "future timestamp locktime should not be final");
}

// ---------------------------------------------------------------------------
// hash_output_script tests
// ---------------------------------------------------------------------------

#[test]
fn test_hash_output_script_known_value() {
    let services = Services::from_chain(Chain::Main);

    // A simple OP_DUP OP_HASH160 <20-byte-hash> OP_EQUALVERIFY OP_CHECKSIG script
    // We test that hash_output_script returns SHA256 reversed hex
    let script = hex_decode("76a91489abcdefabbaabbaabbaabbaabbaabbaabbaabba88ac");

    let hash = services.hash_output_script(&script);

    // The hash should be 64 hex characters (32 bytes)
    assert_eq!(hash.len(), 64, "Hash should be 64 hex characters");

    // Verify it is valid hex
    let _decoded = hex_decode(&hash);

    // Compute expected: SHA256 of script, reversed to BE
    let expected_sha = bsv::primitives::hash::sha256(&script);
    let mut expected_bytes = expected_sha.to_vec();
    expected_bytes.reverse();
    let expected_hex: String = expected_bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    assert_eq!(hash, expected_hex);
}

// ---------------------------------------------------------------------------
// get_services_call_history tests
// ---------------------------------------------------------------------------

#[test]
fn test_get_services_call_history_empty() {
    let services = Services::from_chain(Chain::Main);
    let history = services.get_services_call_history(false);

    // Should have entries for each service collection
    assert_eq!(history.services.len(), 6, "Should have 6 service collections");

    // Each should have service name but empty history
    let names: Vec<&str> = history
        .services
        .iter()
        .map(|s| s.service_name.as_str())
        .collect();
    assert!(names.contains(&"getMerklePath"));
    assert!(names.contains(&"getRawTx"));
    assert!(names.contains(&"postBeef"));
    assert!(names.contains(&"getUtxoStatus"));
    assert!(names.contains(&"getStatusForTxids"));
    assert!(names.contains(&"getScriptHashHistory"));
}

// ---------------------------------------------------------------------------
// From<Chain> impl test
// ---------------------------------------------------------------------------

#[test]
fn test_services_from_chain_impl() {
    let services: Services = Chain::Main.into();
    assert_eq!(services.config().chain, Chain::Main);
}
