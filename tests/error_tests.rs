//! Tests for WalletError enum and WalletErrorObject serialization.
//!
//! Verifies that all WERR variants display correctly, code() returns the right
//! string codes, and to_wallet_error_object() produces valid JSON matching the
//! TypeScript WalletError.toJson() format.

// The production crate already allows `clippy::result_large_err` at the
// crate root because `WalletError::ReviewActions` intentionally carries rich
// payload data. Test-side `WalletResult<_>` functions inherit the same
// constraint, so we allow the lint here too.
#![allow(clippy::result_large_err)]

use bsv_wallet_toolbox::error::{WalletError, WalletErrorObject, WalletResult};

// -- Display output tests --

#[test]
fn internal_error_displays_with_werr_prefix() {
    let err = WalletError::Internal("something broke".to_string());
    let display = format!("{}", err);
    assert!(
        display.starts_with("WERR_INTERNAL:"),
        "Expected WERR_INTERNAL prefix, got: {}",
        display
    );
    assert!(display.contains("something broke"));
}

#[test]
fn invalid_parameter_displays_with_werr_prefix_and_details() {
    let err = WalletError::InvalidParameter {
        parameter: "amount".to_string(),
        must_be: "a positive integer".to_string(),
    };
    let display = format!("{}", err);
    assert!(
        display.starts_with("WERR_INVALID_PARAMETER:"),
        "Expected WERR_INVALID_PARAMETER prefix, got: {}",
        display
    );
    assert!(display.contains("amount"));
    assert!(display.contains("a positive integer"));
}

#[test]
fn missing_parameter_displays_with_werr_prefix() {
    let err = WalletError::MissingParameter("foo".to_string());
    let display = format!("{}", err);
    assert!(
        display.starts_with("WERR_MISSING_PARAMETER:"),
        "Expected WERR_MISSING_PARAMETER prefix, got: {}",
        display
    );
    assert!(display.contains("foo"));
}

#[test]
fn not_implemented_displays_with_werr_prefix() {
    let err = WalletError::NotImplemented("feature X".to_string());
    let display = format!("{}", err);
    assert!(display.starts_with("WERR_NOT_IMPLEMENTED:"));
}

#[test]
fn bad_request_displays_with_werr_prefix() {
    let err = WalletError::BadRequest("bad input".to_string());
    assert!(format!("{}", err).starts_with("WERR_BAD_REQUEST:"));
}

#[test]
fn unauthorized_displays_with_werr_prefix() {
    let err = WalletError::Unauthorized("no token".to_string());
    assert!(format!("{}", err).starts_with("WERR_UNAUTHORIZED:"));
}

#[test]
fn not_active_displays_with_werr_prefix() {
    let err = WalletError::NotActive("wallet inactive".to_string());
    assert!(format!("{}", err).starts_with("WERR_NOT_ACTIVE:"));
}

#[test]
fn invalid_operation_displays_with_werr_prefix() {
    let err = WalletError::InvalidOperation("cannot do that".to_string());
    assert!(format!("{}", err).starts_with("WERR_INVALID_OPERATION:"));
}

#[test]
fn insufficient_funds_displays_with_werr_prefix() {
    let err = WalletError::InsufficientFunds {
        message: "not enough".to_string(),
        total_satoshis_needed: 1000,
        more_satoshis_needed: 500,
    };
    let display = format!("{}", err);
    assert!(display.starts_with("WERR_INSUFFICIENT_FUNDS:"));
    assert!(display.contains("not enough"));
}

#[test]
fn broadcast_unavailable_displays_with_werr_prefix() {
    let err = WalletError::BroadcastUnavailable;
    assert!(format!("{}", err).starts_with("WERR_BROADCAST_UNAVAILABLE:"));
}

#[test]
fn network_chain_displays_with_werr_prefix() {
    let err = WalletError::NetworkChain("wrong chain".to_string());
    assert!(format!("{}", err).starts_with("WERR_NETWORK_CHAIN:"));
}

#[test]
fn invalid_public_key_displays_with_werr_prefix() {
    let err = WalletError::InvalidPublicKey {
        message: "bad key format".to_string(),
        key: "abc123".to_string(),
    };
    assert!(format!("{}", err).starts_with("WERR_INVALID_PUBLIC_KEY:"));
}

// -- code() method tests --

#[test]
fn code_returns_correct_string_for_each_variant() {
    assert_eq!(WalletError::Internal("x".into()).code(), "WERR_INTERNAL");
    assert_eq!(
        WalletError::InvalidParameter {
            parameter: "p".into(),
            must_be: "m".into()
        }
        .code(),
        "WERR_INVALID_PARAMETER"
    );
    assert_eq!(
        WalletError::NotImplemented("x".into()).code(),
        "WERR_NOT_IMPLEMENTED"
    );
    assert_eq!(
        WalletError::BadRequest("x".into()).code(),
        "WERR_BAD_REQUEST"
    );
    assert_eq!(
        WalletError::Unauthorized("x".into()).code(),
        "WERR_UNAUTHORIZED"
    );
    assert_eq!(WalletError::NotActive("x".into()).code(), "WERR_NOT_ACTIVE");
    assert_eq!(
        WalletError::InvalidOperation("x".into()).code(),
        "WERR_INVALID_OPERATION"
    );
    assert_eq!(
        WalletError::MissingParameter("x".into()).code(),
        "WERR_MISSING_PARAMETER"
    );
    assert_eq!(
        WalletError::InsufficientFunds {
            message: "x".into(),
            total_satoshis_needed: 0,
            more_satoshis_needed: 0
        }
        .code(),
        "WERR_INSUFFICIENT_FUNDS"
    );
    assert_eq!(
        WalletError::BroadcastUnavailable.code(),
        "WERR_BROADCAST_UNAVAILABLE"
    );
    assert_eq!(
        WalletError::NetworkChain("x".into()).code(),
        "WERR_NETWORK_CHAIN"
    );
    assert_eq!(
        WalletError::InvalidPublicKey {
            message: "x".into(),
            key: "k".into()
        }
        .code(),
        "WERR_INVALID_PUBLIC_KEY"
    );
}

// -- to_wallet_error_object() tests --

#[test]
fn internal_error_object_has_correct_fields() {
    let err = WalletError::Internal("test error".to_string());
    let obj = err.to_wallet_error_object();
    assert!(obj.is_error);
    assert_eq!(obj.name, "WERR_INTERNAL");
    assert!(obj.message.contains("test error"));
    assert!(obj.parameter.is_none());
    assert!(obj.total_satoshis_needed.is_none());
    assert!(obj.more_satoshis_needed.is_none());
}

#[test]
fn invalid_parameter_error_object_includes_parameter() {
    let err = WalletError::InvalidParameter {
        parameter: "txid".to_string(),
        must_be: "a valid hex string".to_string(),
    };
    let obj = err.to_wallet_error_object();
    assert!(obj.is_error);
    assert_eq!(obj.name, "WERR_INVALID_PARAMETER");
    assert_eq!(obj.parameter, Some("txid".to_string()));
}

#[test]
fn missing_parameter_error_object_includes_parameter() {
    let err = WalletError::MissingParameter("outputId".to_string());
    let obj = err.to_wallet_error_object();
    assert_eq!(obj.parameter, Some("outputId".to_string()));
}

#[test]
fn insufficient_funds_error_object_includes_satoshi_amounts() {
    let err = WalletError::InsufficientFunds {
        message: "need more coins".to_string(),
        total_satoshis_needed: 50000,
        more_satoshis_needed: 25000,
    };
    let obj = err.to_wallet_error_object();
    assert!(obj.is_error);
    assert_eq!(obj.name, "WERR_INSUFFICIENT_FUNDS");
    assert_eq!(obj.total_satoshis_needed, Some(50000));
    assert_eq!(obj.more_satoshis_needed, Some(25000));
}

// -- JSON serialization tests --

#[test]
fn wallet_error_object_serializes_to_camel_case_json() {
    let err = WalletError::Internal("test".to_string());
    let obj = err.to_wallet_error_object();
    let json = serde_json::to_string(&obj).unwrap();
    assert!(json.contains("\"isError\":true"));
    assert!(json.contains("\"name\":\"WERR_INTERNAL\""));
    assert!(json.contains("\"message\":"));
}

#[test]
fn optional_fields_are_omitted_when_none() {
    let err = WalletError::Internal("test".to_string());
    let obj = err.to_wallet_error_object();
    let json = serde_json::to_string(&obj).unwrap();
    // None fields should not appear in the JSON at all
    assert!(
        !json.contains("\"parameter\""),
        "parameter should be omitted when None, got: {}",
        json
    );
    assert!(
        !json.contains("\"totalSatoshisNeeded\""),
        "totalSatoshisNeeded should be omitted when None"
    );
    assert!(
        !json.contains("\"moreSatoshisNeeded\""),
        "moreSatoshisNeeded should be omitted when None"
    );
    assert!(
        !json.contains("\"code\""),
        "code should be omitted when None"
    );
}

#[test]
fn wallet_error_object_serde_roundtrip() {
    let obj = WalletErrorObject {
        is_error: true,
        name: "WERR_INTERNAL".to_string(),
        message: "test message".to_string(),
        code: Some(42),
        parameter: Some("txid".to_string()),
        total_satoshis_needed: Some(100),
        more_satoshis_needed: Some(50),
        review_action_results: None,
        send_with_results: None,
        txid: None,
        tx: None,
        no_send_change: None,
        block_hash: None,
        block_height: None,
        merkle_root: None,
        key: None,
    };
    let json = serde_json::to_string(&obj).unwrap();
    let parsed: WalletErrorObject = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.is_error, obj.is_error);
    assert_eq!(parsed.name, obj.name);
    assert_eq!(parsed.message, obj.message);
    assert_eq!(parsed.code, obj.code);
    assert_eq!(parsed.parameter, obj.parameter);
    assert_eq!(parsed.total_satoshis_needed, obj.total_satoshis_needed);
    assert_eq!(parsed.more_satoshis_needed, obj.more_satoshis_needed);
}

// -- #[from] conversion tests --

#[test]
fn from_io_error_converts_to_wallet_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let wallet_err: WalletError = io_err.into();
    assert_eq!(wallet_err.code(), "WERR_INTERNAL");
    assert!(format!("{}", wallet_err).contains("file not found"));
}

#[test]
fn from_serde_json_error_converts_to_wallet_error() {
    let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
    let wallet_err: WalletError = json_err.into();
    assert_eq!(wallet_err.code(), "WERR_INTERNAL");
}

// -- WalletResult type alias test --

#[test]
fn wallet_result_works_as_return_type() {
    fn test_fn() -> WalletResult<i32> {
        Ok(42)
    }
    assert_eq!(test_fn().unwrap(), 42);
}

#[test]
fn wallet_result_propagates_errors() {
    fn test_fn() -> WalletResult<()> {
        Err(WalletError::Internal("fail".to_string()))
    }
    assert!(test_fn().is_err());
}
