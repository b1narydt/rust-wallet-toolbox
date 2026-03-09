//! Permission-related types for wallet operations.
//!
//! Defines the core types used by `WalletPermissionsManager` for permission
//! checking, token storage, and request/response flows following BRC-73.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Permission type enum
// ---------------------------------------------------------------------------

/// Category of permission being requested or checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionType {
    /// Protocol usage permission (DPACP).
    ProtocolPermission,
    /// Basket access permission (DBAP).
    BasketAccess,
    /// Certificate access permission (DCAP).
    CertificateAccess,
    /// Spending authorization (DSAP).
    SpendingAuthorization,
}

// ---------------------------------------------------------------------------
// Permission token
// ---------------------------------------------------------------------------

/// An on-chain permission token stored as a PushDrop UTXO in an admin basket.
///
/// Represents a granted permission that can be checked, renewed, or revoked.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionToken {
    /// Transaction ID where this token resides.
    pub txid: String,
    /// The raw transaction bytes (BEEF).
    pub tx: Option<Vec<u8>>,
    /// Output index within the transaction.
    pub output_index: u32,
    /// Locking script bytes for the permission output.
    pub output_script: Vec<u8>,
    /// Satoshis assigned to the permission output (often 1).
    pub satoshis: u64,
    /// Normalized originator domain allowed to use this permission.
    pub originator: String,
    /// Raw unnormalized originator string for legacy compatibility.
    pub raw_originator: Option<String>,
    /// Expiration time as UNIX epoch seconds (0 or None for indefinite).
    pub expiry: Option<u64>,
    /// Whether this grants privileged usage (for protocol or certificate).
    pub privileged: Option<bool>,
    /// Protocol name for DPACP tokens.
    pub protocol: Option<String>,
    /// Security level (0, 1, 2) for DPACP tokens.
    pub security_level: Option<u8>,
    /// Counterparty public key or "self"/"anyone" for DPACP tokens.
    pub counterparty: Option<String>,
    /// Basket name for DBAP tokens.
    pub basket_name: Option<String>,
    /// Certificate type for DCAP tokens.
    pub cert_type: Option<String>,
    /// Certificate fields covered by this DCAP token (JSON-encoded).
    pub cert_fields: Option<String>,
    /// Verifier public key for DCAP tokens.
    pub verifier: Option<String>,
    /// Maximum authorized spending amount for DSAP tokens.
    pub authorized_amount: Option<u64>,
}

// ---------------------------------------------------------------------------
// Permission request
// ---------------------------------------------------------------------------

/// A request for permission from an originator.
///
/// Used to prompt the user for authorization before performing a wallet operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    /// Category of permission being requested.
    pub permission_type: PermissionType,
    /// Normalized originator domain.
    pub originator: String,
    /// Whether this is a privileged operation (for protocol or certificate usage).
    pub privileged: Option<bool>,
    /// Protocol name for protocol-type requests.
    pub protocol: Option<String>,
    /// Security level for protocol-type requests.
    pub security_level: Option<u8>,
    /// Counterparty for protocol-type requests.
    pub counterparty: Option<String>,
    /// Basket name for basket-type requests.
    pub basket_name: Option<String>,
    /// Certificate type for certificate-type requests.
    pub cert_type: Option<String>,
    /// Certificate fields for certificate-type requests.
    pub cert_fields: Option<Vec<String>>,
    /// Verifier public key for certificate-type requests.
    pub verifier: Option<String>,
    /// Spending amount for spending-type requests.
    pub amount: Option<u64>,
    /// Human-readable reason for the request.
    pub description: Option<String>,
    /// Action labels for time-based filtering.
    pub labels: Option<Vec<String>>,
    /// Whether this is a new user (first interaction with the originator).
    pub is_new_user: bool,
}

// ---------------------------------------------------------------------------
// Permission response
// ---------------------------------------------------------------------------

/// Response to a permission request from the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionResponse {
    /// Permission granted with optional expiry.
    Grant {
        /// Expiry as UNIX epoch seconds; None for indefinite.
        expiry: Option<u64>,
    },
    /// Permission denied with a reason.
    Deny {
        /// Human-readable denial reason.
        reason: String,
    },
    /// One-time grant without persistent on-chain token.
    EphemeralGrant,
}

// ---------------------------------------------------------------------------
// Grouped permissions
// ---------------------------------------------------------------------------

/// A group of permissions that can be requested together (BRC-73).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupedPermissions {
    /// Protocol usage permissions (DPACP).
    pub protocol_permissions: Vec<PermissionRequest>,
    /// Basket access permissions (DBAP).
    pub basket_access: Vec<PermissionRequest>,
    /// Certificate access permissions (DCAP).
    pub certificate_access: Vec<PermissionRequest>,
    /// Spending authorization (DSAP), if any.
    pub spending_authorization: Option<PermissionRequest>,
}

// ---------------------------------------------------------------------------
// Admin basket name constants
// ---------------------------------------------------------------------------

/// Admin basket name for protocol permission tokens (DPACP).
pub const BASKET_DPACP: &str = "admin protocol-permission";
/// Admin basket name for basket access tokens (DBAP).
pub const BASKET_DBAP: &str = "admin basket-access";
/// Admin basket name for certificate access tokens (DCAP).
pub const BASKET_DCAP: &str = "admin certificate-access";
/// Admin basket name for spending authorization tokens (DSAP).
pub const BASKET_DSAP: &str = "admin spending-authorization";

// ---------------------------------------------------------------------------
// Tag prefix constants for originator-based token lookup
// ---------------------------------------------------------------------------

/// Tag prefix for protocol permission token lookup.
pub const TAG_PREFIX_PROTOCOL: &str = "perm-proto:";
/// Tag prefix for basket access token lookup.
pub const TAG_PREFIX_BASKET: &str = "perm-basket:";
/// Tag prefix for certificate access token lookup.
pub const TAG_PREFIX_CERTIFICATE: &str = "perm-cert:";
/// Tag prefix for spending authorization token lookup.
pub const TAG_PREFIX_SPENDING: &str = "perm-spend:";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_type_equality() {
        assert_eq!(
            PermissionType::ProtocolPermission,
            PermissionType::ProtocolPermission
        );
        assert_ne!(
            PermissionType::ProtocolPermission,
            PermissionType::BasketAccess
        );
        assert_ne!(
            PermissionType::CertificateAccess,
            PermissionType::SpendingAuthorization
        );

        // Test Copy trait
        let a = PermissionType::BasketAccess;
        let b = a;
        assert_eq!(a, b);
    }
}
