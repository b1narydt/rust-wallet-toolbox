//! Request and response types for the Wallet Authentication Backend (WAB) API.
//!
//! All types use `#[serde(rename_all = "camelCase")]` for JSON wire compatibility
//! with the WAB server protocol.

use serde::{Deserialize, Serialize};

/// WAB server info returned from GET /info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WABInfo {
    /// Name of the WAB server instance.
    pub server_name: Option<String>,
    /// Authentication methods supported by this server.
    pub supported_methods: Option<Vec<String>>,
    /// Feature flags enabled on the server.
    pub features: Option<serde_json::Value>,
    /// Server version string.
    pub version: Option<String>,
}

/// Response from starting an authentication flow via /auth/start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAuthResponse {
    /// Whether the auth start request succeeded.
    pub success: bool,
    /// Optional message from the server.
    pub message: Option<String>,
    /// Optional additional data (e.g., session tokens, challenge info).
    pub data: Option<serde_json::Value>,
}

/// Response from completing an authentication flow via /auth/complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteAuthResponse {
    /// Whether the auth completion succeeded.
    pub success: bool,
    /// Optional message from the server.
    pub message: Option<String>,
    /// The authenticated presentation key, returned on success.
    pub presentation_key: Option<String>,
}

/// Response from the faucet endpoint /faucet/request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaucetResponse {
    /// Transaction ID (hex-encoded).
    pub txid: Option<String>,
    /// Raw transaction (hex-encoded).
    pub tx: Option<String>,
    /// Key data (hex-encoded).
    pub k: Option<String>,
    /// Whether the faucet request succeeded.
    pub success: Option<bool>,
    /// Optional message from the server.
    pub message: Option<String>,
}

/// Response from Shamir share operations (/share/store, /share/retrieve, /share/update).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShamirShareResponse {
    /// Whether the share operation succeeded.
    pub success: bool,
    /// Optional message from the server.
    pub message: Option<String>,
    /// The share data (returned from retrieve operations).
    pub share_b: Option<String>,
    /// User ID (returned from store operations).
    pub user_id: Option<i64>,
    /// Share version (returned from update operations).
    pub share_version: Option<i64>,
}

/// Response from listing linked auth methods via /user/linkedMethods.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedMethodsResponse {
    /// List of authentication methods linked to the user.
    pub methods: Option<Vec<LinkedMethod>>,
}

/// A single linked authentication method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedMethod {
    /// The type of authentication method (e.g., "TwilioPhone").
    pub method_type: Option<String>,
    /// The method identifier (e.g., phone number).
    pub identifier: Option<String>,
    /// The server-assigned auth method ID.
    pub auth_method_id: Option<i64>,
}

/// Response from unlinking an auth method via /user/unlinkMethod.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkResponse {
    /// Whether the unlink operation succeeded.
    pub success: bool,
    /// Optional message from the server.
    pub message: Option<String>,
}

/// Response from deleting a user via /user/delete or /share/delete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteUserResponse {
    /// Whether the delete operation succeeded.
    pub success: bool,
    /// Optional message from the server.
    pub message: Option<String>,
}
