//! Unit tests for ARC BEEF downgrade, chain tracker caching, and header construction.

use bsv::transaction::beef::{Beef, BEEF_V1, BEEF_V2};
use bsv::transaction::beef_tx::BeefTx;
use bsv::transaction::Transaction;
use bsv_wallet_toolbox::services::types::ArcConfig;

// ---------------------------------------------------------------------------
// BEEF V2 Downgrade Tests
// ---------------------------------------------------------------------------

/// Build a valid V2 BEEF with one raw tx (no txid-only entries) using the bsv-sdk.
fn build_v2_beef_all_raw() -> Vec<u8> {
    let mut beef = Beef::new(BEEF_V2);
    // Create a minimal transaction (coinbase-like)
    let tx = Transaction::from_hex(
        "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000"
    ).expect("Valid coinbase tx");
    let beef_tx = BeefTx::from_tx(tx, None).expect("Valid beef tx");
    beef.txs.push(beef_tx);

    let mut buf = Vec::new();
    beef.to_binary(&mut buf).expect("Serialization should work");
    buf
}

/// Build a valid V2 BEEF with a txid-only entry using the bsv-sdk.
fn build_v2_beef_with_txid_only() -> Vec<u8> {
    let mut beef = Beef::new(BEEF_V2);

    // Add a txid-only entry
    let txid_only = BeefTx {
        tx: None,
        txid: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        bump_index: None,
        input_txids: Vec::new(),
    };
    beef.txs.push(txid_only);

    // Add a raw tx entry
    let tx = Transaction::from_hex(
        "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000"
    ).expect("Valid coinbase tx");
    let beef_tx = BeefTx::from_tx(tx, None).expect("Valid beef tx");
    beef.txs.push(beef_tx);

    let mut buf = Vec::new();
    beef.to_binary(&mut buf).expect("Serialization should work");
    buf
}

/// Build a valid V1 BEEF using the bsv-sdk.
fn build_v1_beef() -> Vec<u8> {
    let mut beef = Beef::new(BEEF_V1);
    let tx = Transaction::from_hex(
        "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000"
    ).expect("Valid coinbase tx");
    let beef_tx = BeefTx::from_tx(tx, None).expect("Valid beef tx");
    beef.txs.push(beef_tx);

    let mut buf = Vec::new();
    beef.to_binary(&mut buf).expect("Serialization should work");
    buf
}

#[test]
fn test_beef_v2_downgrade_all_raw() {
    // V2 BEEF with all raw txs should be downgraded to V1
    let v2_beef = build_v2_beef_all_raw();
    assert_eq!(
        u32::from_le_bytes([v2_beef[0], v2_beef[1], v2_beef[2], v2_beef[3]]),
        BEEF_V2,
        "Input should be V2"
    );

    let result = bsv_wallet_toolbox::services::providers::arc::maybe_downgrade_beef_v2(&v2_beef);

    // Result should be V1
    let result_version = u32::from_le_bytes([result[0], result[1], result[2], result[3]]);
    assert_eq!(
        result_version, BEEF_V1,
        "Result should be V1 after downgrade"
    );
}

#[test]
fn test_beef_v2_no_downgrade_with_txid_only() {
    // V2 BEEF with txid-only entries should NOT be downgraded
    let v2_beef = build_v2_beef_with_txid_only();
    let result = bsv_wallet_toolbox::services::providers::arc::maybe_downgrade_beef_v2(&v2_beef);

    // Result should still be V2
    let result_version = u32::from_le_bytes([result[0], result[1], result[2], result[3]]);
    assert_eq!(
        result_version, BEEF_V2,
        "Result should remain V2 when txid-only present"
    );
}

#[test]
fn test_beef_v1_unchanged() {
    // V1 BEEF should be returned unchanged
    let v1_beef = build_v1_beef();
    let result = bsv_wallet_toolbox::services::providers::arc::maybe_downgrade_beef_v2(&v1_beef);

    assert_eq!(result, v1_beef, "V1 BEEF should be returned unchanged");
}

// ---------------------------------------------------------------------------
// ARC Header Construction Tests
// ---------------------------------------------------------------------------

#[test]
fn test_arc_headers_with_full_config() {
    let mut custom_headers = std::collections::HashMap::new();
    custom_headers.insert("X-SkipScriptValidation".to_string(), "true".to_string());

    let config = ArcConfig {
        api_key: Some("test-key-123".to_string()),
        deployment_id: "test-deploy-1".to_string(),
        callback_url: Some("https://example.com/callback".to_string()),
        callback_token: Some("cb-token-456".to_string()),
        http_client: None,
        headers: Some(custom_headers),
    };

    let headers = bsv_wallet_toolbox::services::providers::arc::build_arc_headers(&config);

    assert_eq!(
        headers.get("Authorization").unwrap().to_str().unwrap(),
        "Bearer test-key-123"
    );
    assert_eq!(
        headers.get("XDeployment-ID").unwrap().to_str().unwrap(),
        "test-deploy-1"
    );
    assert_eq!(
        headers.get("X-CallbackUrl").unwrap().to_str().unwrap(),
        "https://example.com/callback"
    );
    assert_eq!(
        headers.get("X-CallbackToken").unwrap().to_str().unwrap(),
        "cb-token-456"
    );
    // Updated: TS SDK parity — Content-Type is application/json (not octet-stream).
    assert_eq!(
        headers.get("Content-Type").unwrap().to_str().unwrap(),
        "application/json"
    );
    // New: custom headers from ArcConfig.headers field.
    assert_eq!(
        headers
            .get("X-SkipScriptValidation")
            .unwrap()
            .to_str()
            .unwrap(),
        "true"
    );
}

#[test]
fn test_arc_headers_minimal_config() {
    let config = ArcConfig::default();

    let headers = bsv_wallet_toolbox::services::providers::arc::build_arc_headers(&config);

    // Should have Content-Type and XDeployment-ID, no auth or callback headers
    assert!(headers.get("Content-Type").is_some());
    assert!(headers.get("XDeployment-ID").is_some());
    assert!(headers.get("Authorization").is_none());
    assert!(headers.get("X-CallbackUrl").is_none());
    assert!(headers.get("X-CallbackToken").is_none());
    // Default config has no custom headers.
    assert!(headers.get("X-SkipScriptValidation").is_none());
}

#[test]
fn test_arc_headers_content_type_is_json() {
    // Regression test: Content-Type must be application/json to match TS SDK.
    // Previously application/octet-stream which TAAL ARC rejected.
    let config = ArcConfig::default();
    let headers = bsv_wallet_toolbox::services::providers::arc::build_arc_headers(&config);
    assert_eq!(
        headers.get("Content-Type").unwrap().to_str().unwrap(),
        "application/json",
    );
}

#[test]
fn test_arc_custom_headers_multiple() {
    // Verify multiple custom headers all flow through.
    let mut custom = std::collections::HashMap::new();
    custom.insert("X-SkipScriptValidation".to_string(), "true".to_string());
    custom.insert("X-SkipFeeValidation".to_string(), "true".to_string());
    custom.insert("X-SkipTxValidation".to_string(), "true".to_string());

    let config = ArcConfig {
        api_key: None,
        deployment_id: "test".to_string(),
        callback_url: None,
        callback_token: None,
        http_client: None,
        headers: Some(custom),
    };

    let headers = bsv_wallet_toolbox::services::providers::arc::build_arc_headers(&config);
    assert_eq!(
        headers
            .get("X-SkipScriptValidation")
            .unwrap()
            .to_str()
            .unwrap(),
        "true"
    );
    assert_eq!(
        headers
            .get("X-SkipFeeValidation")
            .unwrap()
            .to_str()
            .unwrap(),
        "true"
    );
    assert_eq!(
        headers.get("X-SkipTxValidation").unwrap().to_str().unwrap(),
        "true"
    );
}

#[test]
fn test_arc_post_body_is_json_with_rawtx_hex() {
    // Verify the request body matches TS SDK's postRawTx data format:
    // { "rawTx": "<hex>" }
    let beef_bytes: Vec<u8> = vec![0x01, 0x02, 0xab, 0xcd, 0xef];
    let body = bsv_wallet_toolbox::services::providers::arc::build_arc_post_body(&beef_bytes);

    // Must be a JSON object.
    assert!(body.is_object(), "body must be a JSON object");

    // Must have exactly one field: rawTx.
    let obj = body.as_object().unwrap();
    assert_eq!(obj.len(), 1, "body must contain exactly one field");
    assert!(obj.contains_key("rawTx"), "body must contain rawTx field");

    // rawTx value must be lowercase hex of the input bytes.
    let raw_tx = obj.get("rawTx").unwrap().as_str().unwrap();
    assert_eq!(raw_tx, "0102abcdef", "rawTx must be lowercase hex of input");
}

#[test]
fn test_arc_post_body_serializes_to_expected_string() {
    // Regression: serialized body must be exactly { "rawTx": "<hex>" }
    let beef_bytes: Vec<u8> = vec![0xde, 0xad, 0xbe, 0xef];
    let body = bsv_wallet_toolbox::services::providers::arc::build_arc_post_body(&beef_bytes);
    let serialized = serde_json::to_string(&body).unwrap();
    assert_eq!(serialized, r#"{"rawTx":"deadbeef"}"#);
}

// ---------------------------------------------------------------------------
// ChaintracksServiceClient URL Tests
// ---------------------------------------------------------------------------

#[test]
fn test_chaintracks_default_url_mainnet() {
    use bsv_wallet_toolbox::types::Chain;
    let client = bsv_wallet_toolbox::services::chaintracker::ChaintracksServiceClient::new(
        Chain::Main,
        None,
        reqwest::Client::new(),
    );
    assert!(
        client.service_url().contains("mainnet-chaintracks"),
        "Main chain should use mainnet URL, got: {}",
        client.service_url()
    );
}

#[test]
fn test_chaintracks_default_url_testnet() {
    use bsv_wallet_toolbox::types::Chain;
    let client = bsv_wallet_toolbox::services::chaintracker::ChaintracksServiceClient::new(
        Chain::Test,
        None,
        reqwest::Client::new(),
    );
    assert!(
        client.service_url().contains("testnet-chaintracks"),
        "Test chain should use testnet URL, got: {}",
        client.service_url()
    );
}

// ---------------------------------------------------------------------------
// ChaintracksChainTracker Cache Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chain_tracker_cache_hit() {
    use bsv::transaction::chain_tracker::ChainTracker;
    use bsv_wallet_toolbox::types::Chain;

    // Create a chain tracker pointing to a dummy URL (won't make real HTTP calls)
    let service_client = bsv_wallet_toolbox::services::chaintracker::ChaintracksServiceClient::new(
        Chain::Main,
        Some("http://localhost:1"),
        reqwest::Client::new(),
    );
    let tracker =
        bsv_wallet_toolbox::services::chaintracker::ChaintracksChainTracker::new(service_client);

    // Manually insert a cached merkle root
    let test_root = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    tracker.insert_cache(100000, test_root.to_string()).await;

    // Check that the cached value is returned without making HTTP calls
    let is_valid = tracker
        .is_valid_root_for_height(test_root, 100000)
        .await
        .expect("Cache lookup should succeed");
    assert!(is_valid, "Cached root should be valid");

    // Check with wrong root
    let is_valid_wrong = tracker
        .is_valid_root_for_height("wrong_root_value", 100000)
        .await
        .expect("Cache lookup should succeed");
    assert!(!is_valid_wrong, "Wrong root should be invalid");
}
