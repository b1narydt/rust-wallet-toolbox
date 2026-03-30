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
        is_no_send: false,
        is_delayed: true,
        is_sign_action: false,
        input_beef: None,
    };
    let json = serde_json::to_value(&args).unwrap();
    assert!(json.get("lockTime").is_some(), "expected camelCase 'lockTime', got: {json}");
    assert!(json.get("isNoSend").is_some(), "expected camelCase 'isNoSend', got: {json}");
    assert!(json.get("isDelayed").is_some(), "expected camelCase 'isDelayed', got: {json}");
    assert!(json.get("isSignAction").is_some(), "expected camelCase 'isSignAction', got: {json}");
    // None fields should be omitted
    assert!(json.get("inputBeef").is_none(), "inputBeef should be omitted when None");
    assert!(json.get("inputBEEF").is_none(), "inputBEEF should be omitted when None");
    // Must NOT have snake_case
    assert!(json.get("lock_time").is_none(), "snake_case 'lock_time' must not appear");
    assert!(json.get("is_no_send").is_none(), "snake_case 'is_no_send' must not appear");

    // Add a second test instance with input_beef = Some
    let args_with_beef = StorageCreateActionArgs {
        description: "test".to_string(),
        inputs: vec![],
        outputs: vec![],
        lock_time: 0,
        version: 1,
        labels: vec![],
        is_no_send: false,
        is_delayed: false,
        is_sign_action: false,
        input_beef: Some(vec![0, 1, 2]),
    };
    let json2 = serde_json::to_value(&args_with_beef).unwrap();
    assert!(json2.get("inputBEEF").is_some(), "must serialize as 'inputBEEF' (uppercase), got: {json2}");
    assert!(json2.get("inputBeef").is_none(), "'inputBeef' (camelCase) must not appear");
    assert!(json2.get("input_beef").is_none(), "'input_beef' (snake_case) must not appear");
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
    assert!(json.get("type").is_some(), "field must serialize as 'type', got: {json}");
    assert!(json.get("outputType").is_none(), "'outputType' must not appear");
    assert!(json.get("output_type").is_none(), "'output_type' must not appear");
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
    assert!(json.get("reference").is_some(), "reference should be present when Some");
    assert!(json.get("txid").is_none(), "txid should be omitted when None");
    assert!(json.get("rawTx").is_none(), "rawTx should be omitted when None");
}

#[test]
fn test_storage_internalize_output_camel_case() {
    let output = StorageInternalizeOutput {
        output_index: 0,
        protocol: "wallet payment".to_string(),
        basket: None,
        custom_instructions: None,
        tags: vec![],
        derivation_prefix: Some("prefix".to_string()),
        derivation_suffix: Some("suffix".to_string()),
        sender_identity_key: Some("02ab".to_string()),
    };
    let json = serde_json::to_value(&output).unwrap();
    assert!(json.get("outputIndex").is_some(), "expected camelCase 'outputIndex'");
    assert!(json.get("derivationPrefix").is_some(), "expected camelCase 'derivationPrefix'");
    assert!(json.get("basket").is_none(), "basket should be omitted when None");
    assert!(json.get("customInstructions").is_none(), "customInstructions should be omitted when None");
}

#[test]
fn test_storage_internalize_action_result_camel_case() {
    let result = StorageInternalizeActionResult {
        accepted: true,
        is_merge: false,
        txid: "ab".repeat(32),
        satoshis: 5000,
        send_with_results: None,
    };
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("isMerge").is_some(), "expected camelCase 'isMerge'");
    assert!(json.get("sendWithResults").is_none(), "sendWithResults should be omitted when None");
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
    assert!(json.get("basketId").is_none(), "None basketId must be omitted, not null");
    assert!(json.get("txid").is_none(), "None txid must be omitted");
    assert!(json.get("lockingScript").is_none(), "None lockingScript must be omitted");
    assert!(json.get("spentBy").is_none(), "None spentBy must be omitted");
    assert!(json.get("outputDescription").is_none(), "None outputDescription must be omitted");
    // Required fields must still be present
    assert!(json.get("outputId").is_some());
    assert!(json.get("satoshis").is_some());
    assert!(json.get("spendable").is_some());
}

use bsv_wallet_toolbox::tables::Transaction;
use bsv_wallet_toolbox::status::TransactionStatus;

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
    assert!(json.get("version").is_none(), "None version must be omitted");
    assert!(json.get("lockTime").is_none(), "None lockTime must be omitted");
    assert!(json.get("txid").is_none(), "None txid must be omitted");
    assert!(json.get("inputBEEF").is_none(), "None inputBEEF must be omitted");
    assert!(json.get("rawTx").is_none(), "None rawTx must be omitted");
    assert!(json.get("provenTxId").is_none(), "None provenTxId must be omitted");
    // Required fields present
    assert!(json.get("transactionId").is_some());
    assert!(json.get("status").is_some());
}
