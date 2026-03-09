//! Error types for the BSV Wallet Toolbox.
//!
//! Provides a unified `WalletError` enum with variants matching all WERR codes
//! from the TypeScript wallet-toolbox. Each variant includes contextual data and
//! displays with a WERR prefix for wire-format consistency.

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
        }
    }

    /// Converts this error into a `WalletErrorObject` suitable for JSON wire format.
    ///
    /// This matches the TypeScript `WalletError.toJson()` output format.
    pub fn to_wallet_error_object(&self) -> WalletErrorObject {
        WalletErrorObject {
            is_error: true,
            name: self.code().to_string(),
            message: self.to_string(),
            code: None,
            parameter: match self {
                Self::InvalidParameter { parameter, .. } => Some(parameter.clone()),
                Self::MissingParameter(p) => Some(p.clone()),
                _ => None,
            },
            total_satoshis_needed: match self {
                Self::InsufficientFunds {
                    total_satoshis_needed,
                    ..
                } => Some(*total_satoshis_needed),
                _ => None,
            },
            more_satoshis_needed: match self {
                Self::InsufficientFunds {
                    more_satoshis_needed,
                    ..
                } => Some(*more_satoshis_needed),
                _ => None,
            },
        }
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
}
