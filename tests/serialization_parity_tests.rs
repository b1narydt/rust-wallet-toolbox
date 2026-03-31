//! Wire-format parity tests: verify Rust serialization matches TypeScript.
//!
//! These tests ensure that types sent over the wire produce JSON with the
//! exact same shape as the TypeScript wallet-toolbox, so the Rust
//! StorageClient can talk to a TS storage server (and vice-versa).

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::interfaces::{BasketInsertion, InternalizeOutput, Payment};
use bsv_wallet_toolbox::storage::action_types::*;
use bsv_wallet_toolbox::types::StorageProvidedBy;

#[test]
fn test_storage_create_action_args_camel_case() {
    let args = StorageCreateActionArgs {
        description: "test payment".to_string(),
        inputs: vec![],
        outputs: vec![],
        lock_time: 0,
        version: 1,
        labels: vec![],
        options: StorageCreateActionOptions::default(),
        input_beef: None,
        is_new_tx: true,
        is_sign_action: false,
        is_no_send: false,
        is_delayed: true,
        is_send_with: false,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    };
    let json = serde_json::to_value(&args).unwrap();
    assert!(
        json.get("lockTime").is_some(),
        "expected camelCase 'lockTime', got: {json}"
    );
    assert!(
        json.get("isNoSend").is_some(),
        "expected camelCase 'isNoSend', got: {json}"
    );
    assert!(
        json.get("isDelayed").is_some(),
        "expected camelCase 'isDelayed', got: {json}"
    );
    assert!(
        json.get("isSignAction").is_some(),
        "expected camelCase 'isSignAction', got: {json}"
    );
    // None fields should be omitted
    assert!(
        json.get("inputBeef").is_none(),
        "inputBeef should be omitted when None"
    );
    assert!(
        json.get("inputBEEF").is_none(),
        "inputBEEF should be omitted when None"
    );
    // Must NOT have snake_case
    assert!(
        json.get("lock_time").is_none(),
        "snake_case 'lock_time' must not appear"
    );
    assert!(
        json.get("is_no_send").is_none(),
        "snake_case 'is_no_send' must not appear"
    );

    // Add a second test instance with input_beef = Some
    let args_with_beef = StorageCreateActionArgs {
        description: "test".to_string(),
        inputs: vec![],
        outputs: vec![],
        lock_time: 0,
        version: 1,
        labels: vec![],
        options: StorageCreateActionOptions::default(),
        input_beef: Some(vec![0, 1, 2]),
        is_new_tx: true,
        is_sign_action: false,
        is_no_send: false,
        is_delayed: false,
        is_send_with: false,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    };
    let json2 = serde_json::to_value(&args_with_beef).unwrap();
    assert!(
        json2.get("inputBEEF").is_some(),
        "must serialize as 'inputBEEF' (uppercase), got: {json2}"
    );
    assert!(
        json2.get("inputBeef").is_none(),
        "'inputBeef' (camelCase) must not appear"
    );
    assert!(
        json2.get("input_beef").is_none(),
        "'input_beef' (snake_case) must not appear"
    );
}

#[test]
fn test_storage_create_transaction_sdk_input_type_field() {
    let input = StorageCreateTransactionSdkInput {
        vin: 0,
        source_txid: "a".repeat(64),
        source_vout: 0,
        source_satoshis: 1000,
        source_locking_script: "76a914".to_string(),
        source_transaction: None,
        unlocking_script_length: 107,
        provided_by: StorageProvidedBy::Storage,
        output_type: "P2PKH".to_string(),
        spending_description: None,
        derivation_prefix: None,
        derivation_suffix: None,
        sender_identity_key: None,
    };
    let json = serde_json::to_value(&input).unwrap();
    assert!(
        json.get("type").is_some(),
        "field must serialize as 'type', got: {json}"
    );
    assert!(
        json.get("outputType").is_none(),
        "'outputType' must not appear"
    );
    assert!(
        json.get("output_type").is_none(),
        "'output_type' must not appear"
    );
}

#[test]
fn test_storage_process_action_args_camel_case() {
    let args = StorageProcessActionArgs {
        is_new_tx: true,
        is_send_with: false,
        is_no_send: false,
        is_delayed: true,
        reference: Some("ref123".to_string()),
        txid: None,
        raw_tx: None,
        send_with: vec![],
    };
    let json = serde_json::to_value(&args).unwrap();
    assert!(json.get("isNewTx").is_some(), "expected 'isNewTx'");
    assert!(json.get("isSendWith").is_some(), "expected 'isSendWith'");
    assert!(
        json.get("reference").is_some(),
        "reference should be present when Some"
    );
    assert!(
        json.get("txid").is_none(),
        "txid should be omitted when None"
    );
    assert!(
        json.get("rawTx").is_none(),
        "rawTx should be omitted when None"
    );
}

#[test]
fn test_storage_internalize_action_result_camel_case() {
    let result = StorageInternalizeActionResult {
        accepted: true,
        is_merge: false,
        txid: "ab".repeat(32),
        satoshis: 5000,
        send_with_results: None,
        not_delayed_results: None,
    };
    let json = serde_json::to_value(&result).unwrap();
    assert!(
        json.get("isMerge").is_some(),
        "expected camelCase 'isMerge'"
    );
    assert!(
        json.get("sendWithResults").is_none(),
        "sendWithResults should be omitted when None"
    );
}

#[test]
fn test_sdk_input_type_field_deserializes() {
    // Verify that TS wire format with "type" key deserializes correctly
    let json_str = r#"{
        "vin": 0,
        "sourceTxid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sourceVout": 0,
        "sourceSatoshis": 1000,
        "sourceLockingScript": "76a914",
        "unlockingScriptLength": 107,
        "providedBy": "storage",
        "type": "P2PKH"
    }"#;
    let input: StorageCreateTransactionSdkInput = serde_json::from_str(json_str).unwrap();
    assert_eq!(input.output_type, "P2PKH");
    assert_eq!(input.source_txid, "a".repeat(64));
}

use bsv_wallet_toolbox::tables::Output;

#[test]
fn test_output_none_fields_omitted() {
    let output = Output {
        output_id: 1,
        user_id: 1,
        transaction_id: 1,
        basket_id: None,
        spendable: true,
        change: false,
        vout: 0,
        satoshis: 1000,
        provided_by: StorageProvidedBy::Storage,
        purpose: "change".to_string(),
        output_type: "P2PKH".to_string(),
        output_description: None,
        txid: None,
        sender_identity_key: None,
        derivation_prefix: None,
        derivation_suffix: None,
        custom_instructions: None,
        spent_by: None,
        sequence_number: None,
        spending_description: None,
        script_length: None,
        script_offset: None,
        locking_script: None,
        created_at: chrono::NaiveDateTime::default(),
        updated_at: chrono::NaiveDateTime::default(),
    };
    let json = serde_json::to_value(&output).unwrap();
    // None fields must be OMITTED, not present as null
    assert!(
        json.get("basketId").is_none(),
        "None basketId must be omitted, not null"
    );
    assert!(json.get("txid").is_none(), "None txid must be omitted");
    assert!(
        json.get("lockingScript").is_none(),
        "None lockingScript must be omitted"
    );
    assert!(
        json.get("spentBy").is_none(),
        "None spentBy must be omitted"
    );
    assert!(
        json.get("outputDescription").is_none(),
        "None outputDescription must be omitted"
    );
    // Required fields must still be present
    assert!(json.get("outputId").is_some());
    assert!(json.get("satoshis").is_some());
    assert!(json.get("spendable").is_some());
}

use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::tables::Transaction;

#[test]
fn test_transaction_none_fields_omitted() {
    let tx = Transaction {
        transaction_id: 1,
        user_id: 1,
        status: TransactionStatus::Completed,
        reference: "ref123".to_string(),
        is_outgoing: true,
        satoshis: 5000,
        description: "test".to_string(),
        version: None,
        lock_time: None,
        txid: None,
        input_beef: None,
        raw_tx: None,
        proven_tx_id: None,
        created_at: chrono::NaiveDateTime::default(),
        updated_at: chrono::NaiveDateTime::default(),
    };
    let json = serde_json::to_value(&tx).unwrap();
    assert!(
        json.get("version").is_none(),
        "None version must be omitted"
    );
    assert!(
        json.get("lockTime").is_none(),
        "None lockTime must be omitted"
    );
    assert!(json.get("txid").is_none(), "None txid must be omitted");
    assert!(
        json.get("inputBEEF").is_none(),
        "None inputBEEF must be omitted"
    );
    assert!(json.get("rawTx").is_none(), "None rawTx must be omitted");
    assert!(
        json.get("provenTxId").is_none(),
        "None provenTxId must be omitted"
    );
    // Required fields present
    assert!(json.get("transactionId").is_some());
    assert!(json.get("status").is_some());
}

#[test]
fn test_create_action_args_wire_format_matches_ts() {
    let args = StorageCreateActionArgs {
        description: "test payment".to_string(),
        inputs: vec![StorageCreateActionInput {
            outpoint: StorageOutPoint {
                txid: "a".repeat(64),
                vout: 0,
            },
            input_description: "spend utxo".to_string(),
            unlocking_script_length: 107,
            sequence_number: 0xffffffff,
        }],
        outputs: vec![StorageCreateActionOutput {
            locking_script: "76a914aabb88ac".to_string(),
            satoshis: 1000,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        }],
        lock_time: 0,
        version: 1,
        labels: vec![],
        options: StorageCreateActionOptions::default(),
        input_beef: Some(vec![0, 1, 2]),
        is_new_tx: true,
        is_sign_action: false,
        is_no_send: false,
        is_delayed: true,
        is_send_with: false,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    };
    let json = serde_json::to_value(&args).unwrap();

    // Verify nested outpoint object
    let input = &json["inputs"][0];
    assert!(
        input.get("outpoint").is_some(),
        "must have nested outpoint object"
    );
    assert_eq!(input["outpoint"]["txid"].as_str().unwrap().len(), 64);
    assert_eq!(input["outpoint"]["vout"].as_u64().unwrap(), 0);
    assert!(
        input.get("outpointTxid").is_none(),
        "flat outpointTxid must NOT appear"
    );

    // Verify options present
    assert!(json.get("options").is_some(), "must have options object");
    let opts = &json["options"];
    assert!(opts.get("signAndProcess").is_some());
    assert!(opts.get("acceptDelayedBroadcast").is_some());
    assert!(opts.get("randomizeOutputs").is_some());

    // Verify boolean flags
    assert!(json.get("isNewTx").is_some());
    assert!(json.get("isSendWith").is_some());
    assert!(json.get("isDelayed").is_some());
    assert!(json.get("isRemixChange").is_some());
    assert!(json.get("includeAllSourceTransactions").is_some());

    // Verify inputBEEF (uppercase BEEF)
    assert!(
        json.get("inputBEEF").is_some(),
        "must use inputBEEF not inputBeef"
    );

    // Verify camelCase
    assert!(json.get("lockTime").is_some());
    assert!(json.get("isSignAction").is_some());
    assert!(json.get("lock_time").is_none());

    // Negative assertion: camelCase "inputBeef" must NOT appear (we use "inputBEEF")
    assert!(
        json.get("inputBeef").is_none(),
        "'inputBeef' (camelCase) must not appear"
    );
}

// ---------------------------------------------------------------------------
// C4: Internalize action wire format tests
// ---------------------------------------------------------------------------

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
    assert!(json.get("tx").is_some(), "must have tx field");
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

// ---------------------------------------------------------------------------
// BRC-100 cross-language test vectors from TS wallet-toolbox
// ---------------------------------------------------------------------------

#[test]
fn test_status_enum_serialization_matches_ts_brc100() {
    use bsv_wallet_toolbox::status::{ProvenTxReqStatus, SyncStatus, TransactionStatus};

    let cases: Vec<(TransactionStatus, &str)> = vec![
        (TransactionStatus::Completed, "completed"),
        (TransactionStatus::Failed, "failed"),
        (TransactionStatus::Unprocessed, "unprocessed"),
        (TransactionStatus::Sending, "sending"),
        (TransactionStatus::Unproven, "unproven"),
        (TransactionStatus::Unsigned, "unsigned"),
        (TransactionStatus::Nosend, "nosend"),
        (TransactionStatus::Nonfinal, "nonfinal"),
        (TransactionStatus::Unfail, "unfail"),
    ];
    for (status, expected) in &cases {
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            *expected,
            "TransactionStatus::{:?} must serialize as '{}'",
            status,
            expected
        );
    }

    // ProvenTxReqStatus — note camelCase "doubleSpend"
    assert_eq!(
        serde_json::to_value(ProvenTxReqStatus::DoubleSpend).unwrap(),
        "doubleSpend"
    );

    let sync_cases: Vec<(SyncStatus, &str)> = vec![
        (SyncStatus::Success, "success"),
        (SyncStatus::Error, "error"),
        (SyncStatus::Identified, "identified"),
        (SyncStatus::Updated, "updated"),
        (SyncStatus::Unknown, "unknown"),
    ];
    for (status, expected) in &sync_cases {
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            *expected,
            "SyncStatus::{:?} must serialize as '{}'",
            status,
            expected
        );
    }
}

#[test]
fn test_storage_provided_by_enum_matches_ts() {
    use bsv_wallet_toolbox::types::StorageProvidedBy;

    assert_eq!(serde_json::to_value(StorageProvidedBy::You).unwrap(), "you");
    assert_eq!(
        serde_json::to_value(StorageProvidedBy::Storage).unwrap(),
        "storage"
    );
    assert_eq!(
        serde_json::to_value(StorageProvidedBy::YouAndStorage).unwrap(),
        "you-and-storage",
        "Must use hyphenated 'you-and-storage', not camelCase"
    );
}

#[test]
fn test_create_action_options_boolean_defaults_match_ts() {
    let opts = StorageCreateActionOptions::default();
    let json = serde_json::to_value(&opts).unwrap();

    assert_eq!(json["signAndProcess"], true);
    assert_eq!(json["acceptDelayedBroadcast"], true);
    assert_eq!(json["randomizeOutputs"], true);
    assert_eq!(json["returnTxidOnly"], false);
    assert_eq!(json["noSend"], false);
}
