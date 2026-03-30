use bsv_wallet_toolbox::tables::Output;
use bsv_wallet_toolbox::types::StorageProvidedBy;

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
