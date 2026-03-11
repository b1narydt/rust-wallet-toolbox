//! Serde round-trip tests for all 16 table structs.
//!
//! Verifies that each struct serializes to JSON with camelCase field names
//! and deserializes back to an equal struct.

use chrono::NaiveDateTime;
use serde_json;

use bsv_wallet_toolbox::status::{ProvenTxReqStatus, SyncStatus, TransactionStatus};
use bsv_wallet_toolbox::tables::*;
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

fn sample_datetime() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2025-01-15 10:30:00", "%Y-%m-%d %H:%M:%S").unwrap()
}

fn sample_datetime2() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2025-01-15 10:31:00", "%Y-%m-%d %H:%M:%S").unwrap()
}

// -- User --

fn sample_user() -> User {
    User {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        user_id: 1,
        identity_key: "02abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab"
            .to_string(),
        active_storage: "03storagekey".to_string(),
    }
}

#[test]
fn user_serializes_camel_case() {
    let json = serde_json::to_value(&sample_user()).unwrap();
    assert!(
        json.get("userId").is_some(),
        "Expected camelCase key 'userId'"
    );
    assert!(
        json.get("identityKey").is_some(),
        "Expected camelCase key 'identityKey'"
    );
    assert!(
        json.get("activeStorage").is_some(),
        "Expected camelCase key 'activeStorage'"
    );
    assert!(
        json.get("user_id").is_none(),
        "snake_case key 'user_id' should not be present"
    );
}

#[test]
fn user_roundtrip() {
    let original = sample_user();
    let json = serde_json::to_string(&original).unwrap();
    let restored: User = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- ProvenTx --

fn sample_proven_tx() -> ProvenTx {
    ProvenTx {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        proven_tx_id: 42,
        txid: "abcd1234".to_string(),
        height: 800000,
        index: 5,
        merkle_path: vec![0x01, 0x02, 0x03],
        raw_tx: vec![0x04, 0x05],
        block_hash: "blockhash123".to_string(),
        merkle_root: "merkleroot456".to_string(),
    }
}

#[test]
fn proven_tx_serializes_camel_case() {
    let json = serde_json::to_value(&sample_proven_tx()).unwrap();
    assert!(json.get("provenTxId").is_some());
    assert!(json.get("blockHash").is_some());
    assert!(json.get("merkleRoot").is_some());
    assert!(json.get("merklePath").is_some());
    assert!(json.get("rawTx").is_some());
}

#[test]
fn proven_tx_roundtrip() {
    let original = sample_proven_tx();
    let json = serde_json::to_string(&original).unwrap();
    let restored: ProvenTx = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- ProvenTxReq --

fn sample_proven_tx_req() -> ProvenTxReq {
    ProvenTxReq {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        proven_tx_req_id: 10,
        proven_tx_id: Some(42),
        status: ProvenTxReqStatus::Unsent,
        attempts: 3,
        notified: false,
        txid: "reqtxid123".to_string(),
        batch: Some("batch-001".to_string()),
        history: "{}".to_string(),
        notify: "{}".to_string(),
        raw_tx: vec![0x10, 0x20],
        input_beef: None,
    }
}

#[test]
fn proven_tx_req_serializes_camel_case() {
    let json = serde_json::to_value(&sample_proven_tx_req()).unwrap();
    assert!(json.get("provenTxReqId").is_some());
    assert!(json.get("provenTxId").is_some());
    assert!(json.get("inputBEEF").is_some());
    assert!(json.get("rawTx").is_some());
}

#[test]
fn proven_tx_req_roundtrip() {
    let original = sample_proven_tx_req();
    let json = serde_json::to_string(&original).unwrap();
    let restored: ProvenTxReq = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Transaction --

fn sample_transaction() -> Transaction {
    Transaction {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        transaction_id: 100,
        user_id: 1,
        proven_tx_id: Some(42),
        status: TransactionStatus::Completed,
        reference: "ref123".to_string(),
        is_outgoing: true,
        satoshis: 50000,
        description: "Test payment".to_string(),
        version: Some(1),
        lock_time: Some(0),
        txid: Some("txid123".to_string()),
        input_beef: None,
        raw_tx: Some(vec![0x01, 0x02, 0x03]),
    }
}

#[test]
fn transaction_serializes_camel_case() {
    let json = serde_json::to_value(&sample_transaction()).unwrap();
    assert!(json.get("transactionId").is_some());
    assert!(json.get("userId").is_some());
    assert!(json.get("provenTxId").is_some());
    assert!(json.get("isOutgoing").is_some());
    assert!(json.get("inputBEEF").is_some());
    assert!(json.get("rawTx").is_some());
    assert!(json.get("lockTime").is_some());
}

#[test]
fn transaction_status_serializes_camel_case() {
    let json = serde_json::to_value(&sample_transaction()).unwrap();
    assert_eq!(json["status"], "completed");
}

#[test]
fn transaction_roundtrip() {
    let original = sample_transaction();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Transaction = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- OutputBasket --

fn sample_output_basket() -> OutputBasket {
    OutputBasket {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        basket_id: 1,
        user_id: 1,
        name: "default".to_string(),
        number_of_desired_utxos: 6,
        minimum_desired_utxo_value: 10000,
        is_deleted: false,
    }
}

#[test]
fn output_basket_serializes_camel_case() {
    let json = serde_json::to_value(&sample_output_basket()).unwrap();
    assert!(json.get("basketId").is_some());
    assert!(json.get("numberOfDesiredUTXOs").is_some());
    assert!(json.get("minimumDesiredUTXOValue").is_some());
    assert!(json.get("isDeleted").is_some());
}

#[test]
fn output_basket_roundtrip() {
    let original = sample_output_basket();
    let json = serde_json::to_string(&original).unwrap();
    let restored: OutputBasket = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Output --

fn sample_output() -> Output {
    Output {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        output_id: 1,
        user_id: 1,
        transaction_id: 100,
        basket_id: Some(1),
        spendable: true,
        change: false,
        output_description: Some("test output".to_string()),
        vout: 0,
        satoshis: 25000,
        provided_by: StorageProvidedBy::Storage,
        purpose: "change".to_string(),
        output_type: "P2PKH".to_string(),
        txid: Some("txid123".to_string()),
        sender_identity_key: None,
        derivation_prefix: Some("prefix".to_string()),
        derivation_suffix: Some("suffix".to_string()),
        custom_instructions: None,
        spent_by: None,
        sequence_number: Some(0),
        spending_description: None,
        script_length: Some(25),
        script_offset: Some(0),
        locking_script: Some(vec![0x76, 0xa9]),
    }
}

#[test]
fn output_serializes_camel_case() {
    let json = serde_json::to_value(&sample_output()).unwrap();
    assert!(json.get("outputId").is_some());
    assert!(json.get("transactionId").is_some());
    assert!(json.get("basketId").is_some());
    assert!(json.get("outputDescription").is_some());
    assert!(json.get("senderIdentityKey").is_some());
    assert!(json.get("derivationPrefix").is_some());
    assert!(json.get("lockingScript").is_some());
    assert!(json.get("providedBy").is_some());
    // The 'type' field should serialize as "type" not "outputType"
    assert!(json.get("type").is_some(), "Expected 'type' key in JSON");
    assert!(json.get("scriptLength").is_some());
    assert!(json.get("scriptOffset").is_some());
}

#[test]
fn output_optional_none_serialized() {
    let json = serde_json::to_value(&sample_output()).unwrap();
    // None values should serialize as null
    assert!(json.get("senderIdentityKey").unwrap().is_null());
    assert!(json.get("customInstructions").unwrap().is_null());
    assert!(json.get("spentBy").unwrap().is_null());
}

#[test]
fn output_roundtrip() {
    let original = sample_output();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Output = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- OutputTag --

fn sample_output_tag() -> OutputTag {
    OutputTag {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        output_tag_id: 1,
        user_id: 1,
        tag: "important".to_string(),
        is_deleted: false,
    }
}

#[test]
fn output_tag_roundtrip() {
    let original = sample_output_tag();
    let json = serde_json::to_string(&original).unwrap();
    let restored: OutputTag = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- OutputTagMap --

fn sample_output_tag_map() -> OutputTagMap {
    OutputTagMap {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        output_tag_id: 1,
        output_id: 1,
        is_deleted: false,
    }
}

#[test]
fn output_tag_map_roundtrip() {
    let original = sample_output_tag_map();
    let json = serde_json::to_string(&original).unwrap();
    let restored: OutputTagMap = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- TxLabel --

fn sample_tx_label() -> TxLabel {
    TxLabel {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        tx_label_id: 1,
        user_id: 1,
        label: "payment".to_string(),
        is_deleted: false,
    }
}

#[test]
fn tx_label_roundtrip() {
    let original = sample_tx_label();
    let json = serde_json::to_string(&original).unwrap();
    let restored: TxLabel = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- TxLabelMap --

fn sample_tx_label_map() -> TxLabelMap {
    TxLabelMap {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        tx_label_id: 1,
        transaction_id: 100,
        is_deleted: false,
    }
}

#[test]
fn tx_label_map_roundtrip() {
    let original = sample_tx_label_map();
    let json = serde_json::to_string(&original).unwrap();
    let restored: TxLabelMap = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Certificate --

fn sample_certificate() -> Certificate {
    Certificate {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        certificate_id: 1,
        user_id: 1,
        cert_type: "identity".to_string(),
        serial_number: "SN-001".to_string(),
        certifier: "02certifier".to_string(),
        subject: "02subject".to_string(),
        verifier: None,
        revocation_outpoint: "txid.0".to_string(),
        signature: "sig123".to_string(),
        is_deleted: false,
    }
}

#[test]
fn certificate_serializes_type_field() {
    let json = serde_json::to_value(&sample_certificate()).unwrap();
    // Should serialize as "type" not "certType"
    assert!(json.get("type").is_some(), "Expected 'type' key in JSON");
    assert!(
        json.get("certType").is_none(),
        "Should not have 'certType' key"
    );
    assert!(json.get("serialNumber").is_some());
    assert!(json.get("revocationOutpoint").is_some());
}

#[test]
fn certificate_roundtrip() {
    let original = sample_certificate();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Certificate = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- CertificateField --

fn sample_certificate_field() -> CertificateField {
    CertificateField {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        user_id: 1,
        certificate_id: 1,
        field_name: "name".to_string(),
        field_value: "Alice".to_string(),
        master_key: "masterkey123".to_string(),
    }
}

#[test]
fn certificate_field_roundtrip() {
    let original = sample_certificate_field();
    let json = serde_json::to_string(&original).unwrap();
    let restored: CertificateField = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Commission --

fn sample_commission() -> Commission {
    Commission {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        commission_id: 1,
        user_id: 1,
        transaction_id: 100,
        satoshis: 500,
        key_offset: "keyoffset123".to_string(),
        is_redeemed: false,
        locking_script: vec![0x76, 0xa9, 0x14],
    }
}

#[test]
fn commission_serializes_camel_case() {
    let json = serde_json::to_value(&sample_commission()).unwrap();
    assert!(json.get("commissionId").is_some());
    assert!(json.get("keyOffset").is_some());
    assert!(json.get("isRedeemed").is_some());
    assert!(json.get("lockingScript").is_some());
}

#[test]
fn commission_roundtrip() {
    let original = sample_commission();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Commission = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- MonitorEvent --

fn sample_monitor_event() -> MonitorEvent {
    MonitorEvent {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        id: 1,
        event: "block_processed".to_string(),
        details: Some("Block 800000 processed".to_string()),
    }
}

#[test]
fn monitor_event_roundtrip() {
    let original = sample_monitor_event();
    let json = serde_json::to_string(&original).unwrap();
    let restored: MonitorEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Settings --

fn sample_settings() -> Settings {
    Settings {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        storage_identity_key: "03storageidentity".to_string(),
        storage_name: "test-storage".to_string(),
        chain: Chain::Main,
        dbtype: "SQLite".to_string(),
        max_output_script: 100,
        wallet_settings_json: None,
    }
}

#[test]
fn settings_serializes_camel_case() {
    let json = serde_json::to_value(&sample_settings()).unwrap();
    assert!(json.get("storageIdentityKey").is_some());
    assert!(json.get("storageName").is_some());
    assert!(json.get("maxOutputScript").is_some());
    assert_eq!(json["chain"], "main");
}

#[test]
fn settings_roundtrip() {
    let original = sample_settings();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- SyncState --

fn sample_sync_state() -> SyncState {
    SyncState {
        created_at: sample_datetime(),
        updated_at: sample_datetime2(),
        sync_state_id: 1,
        user_id: 1,
        storage_identity_key: "03storageidentity".to_string(),
        storage_name: "test-storage".to_string(),
        status: SyncStatus::Unknown,
        init: false,
        ref_num: "ref-001".to_string(),
        sync_map: "{}".to_string(),
        when: None,
        satoshis: None,
        error_local: None,
        error_other: None,
    }
}

#[test]
fn sync_state_serializes_camel_case() {
    let json = serde_json::to_value(&sample_sync_state()).unwrap();
    assert!(json.get("syncStateId").is_some());
    assert!(json.get("storageIdentityKey").is_some());
    assert!(json.get("storageName").is_some());
    assert!(json.get("refNum").is_some());
    assert!(json.get("syncMap").is_some());
    assert!(json.get("errorLocal").is_some());
    assert!(json.get("errorOther").is_some());
    assert_eq!(json["status"], "unknown");
}

#[test]
fn sync_state_roundtrip() {
    let original = sample_sync_state();
    let json = serde_json::to_string(&original).unwrap();
    let restored: SyncState = serde_json::from_str(&json).unwrap();
    assert_eq!(original, restored);
}

// -- Comprehensive: All 16 structs round-trip --

#[test]
fn all_16_structs_roundtrip() {
    // User
    let u = sample_user();
    let j = serde_json::to_string(&u).unwrap();
    assert_eq!(u, serde_json::from_str::<User>(&j).unwrap());

    // ProvenTx
    let pt = sample_proven_tx();
    let j = serde_json::to_string(&pt).unwrap();
    assert_eq!(pt, serde_json::from_str::<ProvenTx>(&j).unwrap());

    // ProvenTxReq
    let ptr = sample_proven_tx_req();
    let j = serde_json::to_string(&ptr).unwrap();
    assert_eq!(ptr, serde_json::from_str::<ProvenTxReq>(&j).unwrap());

    // Transaction
    let t = sample_transaction();
    let j = serde_json::to_string(&t).unwrap();
    assert_eq!(t, serde_json::from_str::<Transaction>(&j).unwrap());

    // OutputBasket
    let ob = sample_output_basket();
    let j = serde_json::to_string(&ob).unwrap();
    assert_eq!(ob, serde_json::from_str::<OutputBasket>(&j).unwrap());

    // Output
    let o = sample_output();
    let j = serde_json::to_string(&o).unwrap();
    assert_eq!(o, serde_json::from_str::<Output>(&j).unwrap());

    // OutputTag
    let ot = sample_output_tag();
    let j = serde_json::to_string(&ot).unwrap();
    assert_eq!(ot, serde_json::from_str::<OutputTag>(&j).unwrap());

    // OutputTagMap
    let otm = sample_output_tag_map();
    let j = serde_json::to_string(&otm).unwrap();
    assert_eq!(otm, serde_json::from_str::<OutputTagMap>(&j).unwrap());

    // TxLabel
    let tl = sample_tx_label();
    let j = serde_json::to_string(&tl).unwrap();
    assert_eq!(tl, serde_json::from_str::<TxLabel>(&j).unwrap());

    // TxLabelMap
    let tlm = sample_tx_label_map();
    let j = serde_json::to_string(&tlm).unwrap();
    assert_eq!(tlm, serde_json::from_str::<TxLabelMap>(&j).unwrap());

    // Certificate
    let c = sample_certificate();
    let j = serde_json::to_string(&c).unwrap();
    assert_eq!(c, serde_json::from_str::<Certificate>(&j).unwrap());

    // CertificateField
    let cf = sample_certificate_field();
    let j = serde_json::to_string(&cf).unwrap();
    assert_eq!(cf, serde_json::from_str::<CertificateField>(&j).unwrap());

    // Commission
    let cm = sample_commission();
    let j = serde_json::to_string(&cm).unwrap();
    assert_eq!(cm, serde_json::from_str::<Commission>(&j).unwrap());

    // MonitorEvent
    let me = sample_monitor_event();
    let j = serde_json::to_string(&me).unwrap();
    assert_eq!(me, serde_json::from_str::<MonitorEvent>(&j).unwrap());

    // Settings
    let s = sample_settings();
    let j = serde_json::to_string(&s).unwrap();
    assert_eq!(s, serde_json::from_str::<Settings>(&j).unwrap());

    // SyncState
    let ss = sample_sync_state();
    let j = serde_json::to_string(&ss).unwrap();
    assert_eq!(ss, serde_json::from_str::<SyncState>(&j).unwrap());
}

// -- Status enum serialization in table context --

#[test]
fn transaction_status_values_serialize_correctly() {
    let mut tx = sample_transaction();

    tx.status = TransactionStatus::Completed;
    assert_eq!(serde_json::to_value(&tx).unwrap()["status"], "completed");

    tx.status = TransactionStatus::Failed;
    assert_eq!(serde_json::to_value(&tx).unwrap()["status"], "failed");

    tx.status = TransactionStatus::Unprocessed;
    assert_eq!(serde_json::to_value(&tx).unwrap()["status"], "unprocessed");

    tx.status = TransactionStatus::Nosend;
    assert_eq!(serde_json::to_value(&tx).unwrap()["status"], "nosend");

    tx.status = TransactionStatus::Nonfinal;
    assert_eq!(serde_json::to_value(&tx).unwrap()["status"], "nonfinal");
}

#[test]
fn proven_tx_req_status_values_serialize_correctly() {
    let mut req = sample_proven_tx_req();

    req.status = ProvenTxReqStatus::Completed;
    assert_eq!(serde_json::to_value(&req).unwrap()["status"], "completed");

    req.status = ProvenTxReqStatus::DoubleSpend;
    assert_eq!(serde_json::to_value(&req).unwrap()["status"], "doubleSpend");

    req.status = ProvenTxReqStatus::Unknown;
    assert_eq!(serde_json::to_value(&req).unwrap()["status"], "unknown");
}

#[test]
fn sync_status_values_serialize_correctly() {
    let mut ss = sample_sync_state();

    ss.status = SyncStatus::Success;
    assert_eq!(serde_json::to_value(&ss).unwrap()["status"], "success");

    ss.status = SyncStatus::Error;
    assert_eq!(serde_json::to_value(&ss).unwrap()["status"], "error");

    ss.status = SyncStatus::Identified;
    assert_eq!(serde_json::to_value(&ss).unwrap()["status"], "identified");
}

// -- Vec<u8> serialization behavior --

#[test]
fn vec_u8_optional_none_serializes_as_null() {
    let tx = Transaction {
        input_beef: None,
        raw_tx: None,
        ..sample_transaction()
    };
    let json = serde_json::to_value(&tx).unwrap();
    assert!(json["inputBEEF"].is_null());
    assert!(json["rawTx"].is_null());
}

#[test]
fn vec_u8_some_serializes_as_array() {
    let tx = Transaction {
        raw_tx: Some(vec![1, 2, 3]),
        ..sample_transaction()
    };
    let json = serde_json::to_value(&tx).unwrap();
    let raw_tx = json["rawTx"].as_array().unwrap();
    assert_eq!(raw_tx.len(), 3);
    assert_eq!(raw_tx[0], 1);
}
