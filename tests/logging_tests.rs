//! Tests for logging initialization and status enum serialization.

use bsv_wallet_toolbox::logging::init_logging;
use bsv_wallet_toolbox::status::{OutputStatus, ProvenTxReqStatus, SyncStatus, TransactionStatus};

#[test]
fn init_logging_does_not_panic() {
    // init_logging uses try_init() internally, so calling it
    // should never panic even if called multiple times.
    init_logging();
}

// -- TransactionStatus serde tests --

#[test]
fn transaction_status_serializes_to_camel_case() {
    let status = TransactionStatus::Completed;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"completed\"");

    let status = TransactionStatus::Unprocessed;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"unprocessed\"");

    let status = TransactionStatus::Nosend;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"nosend\"");

    let status = TransactionStatus::Nonfinal;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"nonfinal\"");
}

#[test]
fn transaction_status_roundtrips_through_serde_json() {
    let statuses = vec![
        TransactionStatus::Completed,
        TransactionStatus::Failed,
        TransactionStatus::Unprocessed,
        TransactionStatus::Sending,
        TransactionStatus::Unproven,
        TransactionStatus::Unsigned,
        TransactionStatus::Nosend,
        TransactionStatus::Nonfinal,
        TransactionStatus::Unfail,
    ];
    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let parsed: TransactionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, status, "Failed roundtrip for {:?}", status);
    }
}

// -- ProvenTxReqStatus serde tests --

#[test]
fn proven_tx_req_status_serializes_correctly() {
    assert_eq!(
        serde_json::to_string(&ProvenTxReqStatus::DoubleSpend).unwrap(),
        "\"doubleSpend\""
    );
    assert_eq!(
        serde_json::to_string(&ProvenTxReqStatus::Completed).unwrap(),
        "\"completed\""
    );
    assert_eq!(
        serde_json::to_string(&ProvenTxReqStatus::Unknown).unwrap(),
        "\"unknown\""
    );
}

#[test]
fn proven_tx_req_status_roundtrips() {
    let statuses = vec![
        ProvenTxReqStatus::Sending,
        ProvenTxReqStatus::Unsent,
        ProvenTxReqStatus::Nosend,
        ProvenTxReqStatus::Unknown,
        ProvenTxReqStatus::Nonfinal,
        ProvenTxReqStatus::Unprocessed,
        ProvenTxReqStatus::Unmined,
        ProvenTxReqStatus::Callback,
        ProvenTxReqStatus::Unconfirmed,
        ProvenTxReqStatus::Completed,
        ProvenTxReqStatus::Invalid,
        ProvenTxReqStatus::DoubleSpend,
        ProvenTxReqStatus::Unfail,
    ];
    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let parsed: ProvenTxReqStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, status);
    }
}

// -- SyncStatus serde tests --

#[test]
fn sync_status_serializes_correctly() {
    assert_eq!(
        serde_json::to_string(&SyncStatus::Success).unwrap(),
        "\"success\""
    );
    assert_eq!(
        serde_json::to_string(&SyncStatus::Unknown).unwrap(),
        "\"unknown\""
    );
}

// -- OutputStatus serde tests --

#[test]
fn output_status_roundtrips() {
    let statuses = vec![OutputStatus::Unspent, OutputStatus::Spent];
    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let parsed: OutputStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, status);
    }
}
