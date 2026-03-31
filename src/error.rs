//! Error types for the BSV Wallet Toolbox.
//!
//! Provides a unified `WalletError` enum with variants matching all WERR codes
//! from the TypeScript wallet-toolbox. Each variant includes contextual data and
//! displays with a WERR prefix for wire-format consistency.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use bsv::wallet::interfaces::{ReviewActionResult, SendWithResult};
use bsv::wallet::types::OutpointString;

/// Convenience type alias for Results using `WalletError`.
pub type WalletResult<T> = Result<T, WalletError>;

/// Unified error type for all wallet operations.
///
/// Each variant maps to a WERR error code from the TypeScript wallet-toolbox.
/// Display output always includes the WERR prefix (e.g., "WERR_INTERNAL: description").
#[derive(Debug, Error)]
pub enum WalletError {
    /// An internal error with a descriptive message.
    #[error("WERR_INTERNAL: {0}")]
    Internal(String),

    /// A parameter has an invalid value.
    #[error("WERR_INVALID_PARAMETER: The {parameter} parameter must be {must_be}")]
    InvalidParameter {
        /// The name of the invalid parameter.
        parameter: String,
        /// Description of the valid value constraint.
        must_be: String,
    },

    /// The requested operation is not yet implemented.
    #[error("WERR_NOT_IMPLEMENTED: {0}")]
    NotImplemented(String),

    /// The request was malformed or invalid.
    #[error("WERR_BAD_REQUEST: {0}")]
    BadRequest(String),

    /// The caller is not authenticated or authorized.
    #[error("WERR_UNAUTHORIZED: {0}")]
    Unauthorized(String),

    /// The wallet or resource is not in an active state.
    #[error("WERR_NOT_ACTIVE: {0}")]
    NotActive(String),

    /// The operation is not valid in the current state.
    #[error("WERR_INVALID_OPERATION: {0}")]
    InvalidOperation(String),

    /// A required parameter was not provided.
    #[error("WERR_MISSING_PARAMETER: The required {0} parameter is missing.")]
    MissingParameter(String),

    /// The wallet does not have enough funds for the operation.
    #[error("WERR_INSUFFICIENT_FUNDS: {message}")]
    InsufficientFunds {
        /// Human-readable description of the shortfall.
        message: String,
        /// Total satoshis required for the operation.
        total_satoshis_needed: i64,
        /// Additional satoshis needed beyond available balance.
        more_satoshis_needed: i64,
    },

    /// No broadcast service is currently available.
    #[error("WERR_BROADCAST_UNAVAILABLE: Unable to broadcast transaction at this time.")]
    BroadcastUnavailable,

    /// A network or chain-related error occurred.
    #[error("WERR_NETWORK_CHAIN: {0}")]
    NetworkChain(String),

    /// A public key value is invalid.
    #[error("WERR_INVALID_PUBLIC_KEY: {message}")]
    InvalidPublicKey {
        /// Description of why the key is invalid.
        message: String,
        /// The invalid key value.
        key: String,
    },

    /// Undelayed broadcast results require review (code=5).
    #[error("WERR_REVIEW_ACTIONS: {message}")]
    ReviewActions {
        /// Human-readable message describing the review requirement.
        message: String,
        /// Results from the undelayed broadcast review.
        review_action_results: Vec<ReviewActionResult>,
        /// Results from batch sending.
        send_with_results: Vec<SendWithResult>,
        /// The transaction ID, if available.
        txid: Option<String>,
        /// The raw transaction bytes, if available.
        tx: Option<Vec<u8>>,
        /// Outpoints of change outputs for noSend transactions.
        no_send_change: Vec<OutpointString>,
    },

    /// Invalid merkle root detected during chain validation (code=8).
    #[error("WERR_INVALID_MERKLE_ROOT: {message}")]
    InvalidMerkleRoot {
        /// Human-readable message describing the invalid merkle root.
        message: String,
        /// The hash of the block with the invalid merkle root.
        block_hash: String,
        /// The height of the block with the invalid merkle root.
        block_height: u32,
        /// The invalid merkle root value.
        merkle_root: String,
        /// The transaction ID involved, if available.
        txid: Option<String>,
    },

    /// A database error from sqlx.
    #[error("WERR_INTERNAL: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// An I/O error.
    #[error("WERR_INTERNAL: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization/deserialization error.
    #[error("WERR_INTERNAL: {0}")]
    SerdeJson(#[from] serde_json::Error),
}

impl WalletError {
    /// Returns the WERR string code for wire serialization.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Internal(_) | Self::Sqlx(_) | Self::Io(_) | Self::SerdeJson(_) => "WERR_INTERNAL",
            Self::InvalidParameter { .. } => "WERR_INVALID_PARAMETER",
            Self::NotImplemented(_) => "WERR_NOT_IMPLEMENTED",
            Self::BadRequest(_) => "WERR_BAD_REQUEST",
            Self::Unauthorized(_) => "WERR_UNAUTHORIZED",
            Self::NotActive(_) => "WERR_NOT_ACTIVE",
            Self::InvalidOperation(_) => "WERR_INVALID_OPERATION",
            Self::MissingParameter(_) => "WERR_MISSING_PARAMETER",
            Self::InsufficientFunds { .. } => "WERR_INSUFFICIENT_FUNDS",
            Self::BroadcastUnavailable => "WERR_BROADCAST_UNAVAILABLE",
            Self::NetworkChain(_) => "WERR_NETWORK_CHAIN",
            Self::InvalidPublicKey { .. } => "WERR_INVALID_PUBLIC_KEY",
            Self::ReviewActions { .. } => "WERR_REVIEW_ACTIONS",
            Self::InvalidMerkleRoot { .. } => "WERR_INVALID_MERKLE_ROOT",
        }
    }

    /// Converts this error into a `WalletErrorObject` suitable for JSON wire format.
    ///
    /// This matches the TypeScript `WalletError.toJson()` output format.
    pub fn to_wallet_error_object(&self) -> WalletErrorObject {
        let mut obj = WalletErrorObject {
            is_error: true,
            name: self.code().to_string(),
            message: self.to_string(),
            code: None,
            parameter: None,
            total_satoshis_needed: None,
            more_satoshis_needed: None,
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

        match self {
            Self::InvalidParameter { parameter, .. } => {
                obj.parameter = Some(parameter.clone());
            }
            Self::MissingParameter(p) => {
                obj.parameter = Some(p.clone());
            }
            Self::InsufficientFunds {
                total_satoshis_needed,
                more_satoshis_needed,
                ..
            } => {
                obj.total_satoshis_needed = Some(*total_satoshis_needed);
                obj.more_satoshis_needed = Some(*more_satoshis_needed);
            }
            Self::InvalidPublicKey { key, .. } => {
                obj.key = Some(key.clone());
            }
            Self::ReviewActions {
                review_action_results,
                send_with_results,
                txid,
                tx,
                no_send_change,
                ..
            } => {
                obj.code = Some(5);
                obj.review_action_results = Some(review_action_results.clone());
                obj.send_with_results = Some(send_with_results.clone());
                obj.txid = txid.clone();
                obj.tx = tx.clone();
                obj.no_send_change = Some(no_send_change.clone());
            }
            Self::InvalidMerkleRoot {
                block_hash,
                block_height,
                merkle_root,
                txid,
                ..
            } => {
                obj.code = Some(8);
                obj.block_hash = Some(block_hash.clone());
                obj.block_height = Some(*block_height);
                obj.merkle_root = Some(merkle_root.clone());
                obj.txid = txid.clone();
            }
            _ => {}
        }

        obj
    }
}

/// Convert a `WalletErrorObject` received over the wire into the appropriate `WalletError` variant.
///
/// Maps WERR error code strings (e.g. "WERR_INVALID_PARAMETER") from a JSON-RPC error
/// response body onto the local `WalletError` enum. Used by `StorageClient::rpc_call`
/// after deserializing a remote error payload.
pub fn wallet_error_from_object(obj: WalletErrorObject) -> WalletError {
    match obj.name.as_str() {
        "WERR_INVALID_PARAMETER" => WalletError::InvalidParameter {
            parameter: obj.parameter.unwrap_or_default(),
            must_be: obj.message,
        },
        "WERR_NOT_IMPLEMENTED" => WalletError::NotImplemented(obj.message),
        "WERR_BAD_REQUEST" => WalletError::BadRequest(obj.message),
        "WERR_UNAUTHORIZED" => WalletError::Unauthorized(obj.message),
        "WERR_NOT_ACTIVE" => WalletError::NotActive(obj.message),
        "WERR_INVALID_OPERATION" => WalletError::InvalidOperation(obj.message),
        "WERR_MISSING_PARAMETER" => {
            WalletError::MissingParameter(obj.parameter.unwrap_or_else(|| obj.message.clone()))
        }
        "WERR_INSUFFICIENT_FUNDS" => WalletError::InsufficientFunds {
            message: obj.message,
            total_satoshis_needed: obj.total_satoshis_needed.unwrap_or(0),
            more_satoshis_needed: obj.more_satoshis_needed.unwrap_or(0),
        },
        "WERR_BROADCAST_UNAVAILABLE" => WalletError::BroadcastUnavailable,
        "WERR_NETWORK_CHAIN" => WalletError::NetworkChain(obj.message),
        "WERR_INVALID_PUBLIC_KEY" => WalletError::InvalidPublicKey {
            message: obj.message,
            key: obj.key.or(obj.parameter).unwrap_or_default(),
        },
        "WERR_REVIEW_ACTIONS" => WalletError::ReviewActions {
            message: obj.message,
            review_action_results: obj.review_action_results.unwrap_or_default(),
            send_with_results: obj.send_with_results.unwrap_or_default(),
            txid: obj.txid,
            tx: obj.tx,
            no_send_change: obj.no_send_change.unwrap_or_default(),
        },
        "WERR_INVALID_MERKLE_ROOT" => WalletError::InvalidMerkleRoot {
            message: obj.message,
            block_hash: obj.block_hash.unwrap_or_default(),
            block_height: obj.block_height.unwrap_or(0),
            merkle_root: obj.merkle_root.unwrap_or_default(),
            txid: obj.txid,
        },
        _ => WalletError::Internal(obj.message),
    }
}

/// JSON wire format for wallet errors, matching the TypeScript `WalletError.toJson()` output.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletErrorObject {
    /// Always `true` to indicate an error response.
    pub is_error: bool,
    /// The WERR error code string (e.g., "WERR_INTERNAL").
    pub name: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional numeric error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<u8>,
    /// The parameter name, if the error relates to a specific parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
    /// Total satoshis needed, present only for insufficient funds errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_satoshis_needed: Option<i64>,
    /// Additional satoshis needed, present only for insufficient funds errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_satoshis_needed: Option<i64>,
    /// Review action results for WERR_REVIEW_ACTIONS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_action_results: Option<Vec<ReviewActionResult>>,
    /// Send-with results for WERR_REVIEW_ACTIONS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_with_results: Option<Vec<SendWithResult>>,
    /// Transaction ID for WERR_REVIEW_ACTIONS and WERR_INVALID_MERKLE_ROOT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// Raw transaction bytes for WERR_REVIEW_ACTIONS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<Vec<u8>>,
    /// No-send change outpoints for WERR_REVIEW_ACTIONS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_send_change: Option<Vec<OutpointString>>,
    /// Block hash for WERR_INVALID_MERKLE_ROOT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<String>,
    /// Block height for WERR_INVALID_MERKLE_ROOT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_height: Option<u32>,
    /// Merkle root for WERR_INVALID_MERKLE_ROOT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merkle_root: Option<String>,
    /// The invalid key value for WERR_INVALID_PUBLIC_KEY.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::wallet::interfaces::{
        ActionResultStatus, ReviewActionResult, ReviewActionResultStatus, SendWithResult,
    };

    #[test]
    fn test_review_actions_error_roundtrip() {
        let err = WalletError::ReviewActions {
            message: "Undelayed results require review.".to_string(),
            review_action_results: vec![ReviewActionResult {
                txid: "aabb".to_string(),
                status: ReviewActionResultStatus::DoubleSpend,
                competing_txs: Some(vec!["ccdd".to_string()]),
                competing_beef: None,
            }],
            send_with_results: vec![SendWithResult {
                txid: "eeff".to_string(),
                status: ActionResultStatus::Sending,
            }],
            txid: Some("aabb".to_string()),
            tx: None,
            no_send_change: vec![],
        };
        assert_eq!(err.code(), "WERR_REVIEW_ACTIONS");
        let obj = err.to_wallet_error_object();
        assert_eq!(obj.code, Some(5));
        assert!(obj.review_action_results.is_some());
        let err2 = wallet_error_from_object(obj);
        assert_eq!(err2.code(), "WERR_REVIEW_ACTIONS");
    }

    #[test]
    fn test_invalid_merkle_root_error_roundtrip() {
        let err = WalletError::InvalidMerkleRoot {
            message: "Invalid merkleRoot abc for block def at height 100".to_string(),
            block_hash: "def".to_string(),
            block_height: 100,
            merkle_root: "abc".to_string(),
            txid: Some("1234".to_string()),
        };
        assert_eq!(err.code(), "WERR_INVALID_MERKLE_ROOT");
        let obj = err.to_wallet_error_object();
        assert_eq!(obj.code, Some(8));
        assert_eq!(obj.block_hash.as_deref(), Some("def"));
        let err2 = wallet_error_from_object(obj);
        assert_eq!(err2.code(), "WERR_INVALID_MERKLE_ROOT");
    }

    #[test]
    fn test_invalid_public_key_key_field_roundtrip() {
        let err = WalletError::InvalidPublicKey {
            message: "not on curve".to_string(),
            key: "02abcdef".to_string(),
        };
        let obj = err.to_wallet_error_object();
        assert_eq!(obj.key.as_deref(), Some("02abcdef"));
        let json = serde_json::to_string(&obj).unwrap();
        assert!(json.contains("\"key\":\"02abcdef\""));
        let obj2: WalletErrorObject = serde_json::from_str(&json).unwrap();
        let err2 = wallet_error_from_object(obj2);
        match err2 {
            WalletError::InvalidPublicKey { key, .. } => assert_eq!(key, "02abcdef"),
            _ => panic!("Expected InvalidPublicKey"),
        }
    }
}
