//! Wire-format parity tests: verify Rust serialization matches TypeScript.
//!
//! These tests ensure that types sent over the wire produce JSON with the
//! exact same shape as the TypeScript wallet-toolbox, so the Rust
//! StorageClient can talk to a TS storage server (and vice-versa).

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::interfaces::{BasketInsertion, InternalizeOutput, Payment};
use bsv_wallet_toolbox::storage::action_types::StorageInternalizeActionArgs;

/// Build a dummy compressed public key for tests.
fn test_public_key() -> PublicKey {
    PublicKey::from_string(&("02".to_owned() + &"ab".repeat(32))).unwrap()
}

#[test]
fn test_internalize_action_wire_format_matches_ts() {
    let args = StorageInternalizeActionArgs {
        tx: vec![0, 1, 2],
        description: "receive payment".to_string(),
        labels: vec![],
        seek_permission: true,
        outputs: vec![
            InternalizeOutput::WalletPayment {
                output_index: 0,
                payment: Payment {
                    derivation_prefix: b"invoice-12345".to_vec(),
                    derivation_suffix: b"utxo-0".to_vec(),
                    sender_identity_key: test_public_key(),
                },
            },
            InternalizeOutput::BasketInsertion {
                output_index: 1,
                insertion: BasketInsertion {
                    basket: "payments".to_string(),
                    custom_instructions: Some(r#"{"root":"02135476"}"#.to_string()),
                    tags: vec!["test".to_string()],
                },
            },
        ],
    };
    let json = serde_json::to_value(&args).unwrap();

    // Verify seekPermission present
    assert!(
        json.get("seekPermission").is_some(),
        "must have seekPermission field"
    );
    assert_eq!(json["seekPermission"], true);

    // Verify camelCase field names
    assert!(
        json.get("tx").is_some(),
        "must have tx field"
    );
    assert!(
        json.get("description").is_some(),
        "must have description field"
    );

    // Verify tagged enum format for wallet payment
    let wp = &json["outputs"][0];
    assert_eq!(wp["protocol"], "wallet payment");
    assert!(
        wp.get("paymentRemittance").is_some(),
        "wallet payment must have paymentRemittance"
    );
    assert!(
        wp.get("outputIndex").is_some(),
        "wallet payment must have outputIndex"
    );
    assert_eq!(wp["outputIndex"], 0);

    // Must NOT have flat fields at the top level
    assert!(
        wp.get("derivationPrefix").is_none(),
        "flat derivationPrefix must not appear at top level"
    );
    assert!(
        wp.get("derivationSuffix").is_none(),
        "flat derivationSuffix must not appear at top level"
    );
    assert!(
        wp.get("basket").is_none(),
        "basket must not appear on wallet payment"
    );

    // Verify paymentRemittance contents
    let pr = &wp["paymentRemittance"];
    assert!(
        pr.get("derivationPrefix").is_some(),
        "paymentRemittance must have derivationPrefix"
    );
    assert!(
        pr.get("derivationSuffix").is_some(),
        "paymentRemittance must have derivationSuffix"
    );
    assert!(
        pr.get("senderIdentityKey").is_some(),
        "paymentRemittance must have senderIdentityKey"
    );

    // Verify basket insertion
    let bi = &json["outputs"][1];
    assert_eq!(bi["protocol"], "basket insertion");
    assert!(
        bi.get("insertionRemittance").is_some(),
        "basket insertion must have insertionRemittance"
    );
    assert!(
        bi.get("outputIndex").is_some(),
        "basket insertion must have outputIndex"
    );
    assert_eq!(bi["outputIndex"], 1);

    // Must NOT have flat fields
    assert!(
        bi.get("derivationPrefix").is_none(),
        "derivationPrefix must not appear on basket insertion"
    );

    // Verify insertionRemittance contents
    let ir = &bi["insertionRemittance"];
    assert_eq!(ir["basket"], "payments");
    assert_eq!(ir["customInstructions"], r#"{"root":"02135476"}"#);
    assert_eq!(ir["tags"][0], "test");
}

#[test]
fn test_internalize_action_seek_permission_defaults_true() {
    // Deserialize without seekPermission — should default to true
    let json = r#"{
        "tx": [0, 1, 2],
        "description": "test",
        "outputs": []
    }"#;
    let args: StorageInternalizeActionArgs = serde_json::from_str(json).unwrap();
    assert!(args.seek_permission, "seekPermission must default to true");
}

#[test]
fn test_internalize_action_seek_permission_roundtrip() {
    let args = StorageInternalizeActionArgs {
        tx: vec![0],
        description: "test".to_string(),
        labels: vec![],
        seek_permission: false,
        outputs: vec![],
    };
    let json = serde_json::to_value(&args).unwrap();
    assert_eq!(json["seekPermission"], false);

    let rt: StorageInternalizeActionArgs = serde_json::from_value(json).unwrap();
    assert!(!rt.seek_permission);
}
