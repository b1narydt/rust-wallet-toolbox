//! Wallet Authentication Backend (WAB) client for identity verification.
//!
//! Provides high-level methods to:
//! - Retrieve server info (supported auth methods, faucet info)
//! - Generate a random presentation key
//! - Start/Complete authentication with a chosen AuthMethodInteractor
//! - Link/unlink methods
//! - Request faucet
//! - Delete user
//! - Manage Shamir secret shares for key recovery

pub mod interactors;
pub mod types;

use crate::error::WalletError;
use bsv::primitives::private_key::PrivateKey;
use serde::de::DeserializeOwned;
use types::{
    CompleteAuthResponse, DeleteUserResponse, FaucetResponse, LinkedMethodsResponse,
    ShamirShareResponse, StartAuthResponse, UnlinkResponse, WABInfo,
};

/// HTTP client for communicating with a Wallet Authentication Backend (WAB) server.
///
/// WABClient has no wallet dependency -- it is a pure HTTP client that constructs
/// requests to a WAB server for authentication, identity, and Shamir share operations.
pub struct WABClient {
    /// Base URL of the WAB server (no trailing slash).
    server_url: String,
    /// Reusable HTTP client instance.
    client: reqwest::Client,
}

impl WABClient {
    /// Create a new WABClient targeting the given server URL.
    ///
    /// The URL is trimmed of any trailing slash for consistent path construction.
    pub fn new(server_url: &str) -> Self {
        Self {
            server_url: server_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Return the configured server URL.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    // ========================================================================
    // Private helper
    // ========================================================================

    /// POST a JSON body to `{server_url}{path}` and deserialize the response.
    async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, WalletError> {
        let url = format!("{}{}", self.server_url, path);
        let response = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| WalletError::Internal(format!("HTTP request to {} failed: {}", url, e)))?;

        if !response.status().is_success() {
            return Err(WalletError::Internal(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        response
            .json::<T>()
            .await
            .map_err(|e| WalletError::Internal(format!("Failed to parse response from {}: {}", url, e)))
    }

    // ========================================================================
    // Core methods (8 total)
    // ========================================================================

    /// Retrieve WAB server info (GET /info).
    pub async fn get_info(&self) -> Result<WABInfo, WalletError> {
        let url = format!("{}/info", self.server_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::Internal(format!("HTTP GET {} failed: {}", url, e)))?;

        response
            .json::<WABInfo>()
            .await
            .map_err(|e| WalletError::Internal(format!("Failed to parse info response: {}", e)))
    }

    /// Generate a random 256-bit presentation key as a hex string (client side).
    ///
    /// Uses the BSV SDK's PrivateKey::from_random() for cryptographically secure
    /// random key generation.
    pub fn generate_random_presentation_key() -> Result<String, WalletError> {
        let key = PrivateKey::from_random()
            .map_err(|e| WalletError::Internal(format!("Failed to generate random key: {}", e)))?;
        Ok(key.to_hex())
    }

    /// Start an authentication flow (POST /auth/start).
    pub async fn start_auth_method(
        &self,
        presentation_key: &str,
        method_type: &str,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
            "methodType": method_type,
            "payload": payload,
        });
        self.post_json("/auth/start", &body).await
    }

    /// Complete an authentication flow (POST /auth/complete).
    pub async fn complete_auth_method(
        &self,
        presentation_key: &str,
        method_type: &str,
        payload: serde_json::Value,
    ) -> Result<CompleteAuthResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
            "methodType": method_type,
            "payload": payload,
        });
        self.post_json("/auth/complete", &body).await
    }

    /// List user-linked authentication methods (POST /user/linkedMethods).
    pub async fn list_linked_methods(
        &self,
        presentation_key: &str,
    ) -> Result<LinkedMethodsResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
        });
        self.post_json("/user/linkedMethods", &body).await
    }

    /// Unlink an authentication method by ID (POST /user/unlinkMethod).
    pub async fn unlink_method(
        &self,
        presentation_key: &str,
        auth_method_id: i64,
    ) -> Result<UnlinkResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
            "authMethodId": auth_method_id,
        });
        self.post_json("/user/unlinkMethod", &body).await
    }

    /// Request faucet funds (POST /faucet/request).
    pub async fn request_faucet(
        &self,
        presentation_key: &str,
    ) -> Result<FaucetResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
        });
        self.post_json("/faucet/request", &body).await
    }

    /// Delete user account (POST /user/delete).
    pub async fn delete_user(
        &self,
        presentation_key: &str,
    ) -> Result<DeleteUserResponse, WalletError> {
        let body = serde_json::json!({
            "presentationKey": presentation_key,
        });
        self.post_json("/user/delete", &body).await
    }

    // ========================================================================
    // Shamir share methods (5 total, matching TS WABClient)
    // ========================================================================

    /// Start OTP verification for share operations (POST /auth/start).
    ///
    /// This initiates the auth flow (e.g., sends SMS code via Twilio)
    /// using `user_id_hash` as the presentation key.
    pub async fn start_share_auth(
        &self,
        method_type: &str,
        user_id_hash: &str,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError> {
        let body = serde_json::json!({
            "methodType": method_type,
            "presentationKey": user_id_hash,
            "payload": payload,
        });
        self.post_json("/auth/start", &body).await
    }

    /// Store a Shamir share (Share B) on the server (POST /share/store).
    ///
    /// Requires prior OTP verification via `start_share_auth`.
    pub async fn store_share(
        &self,
        method_type: &str,
        payload: serde_json::Value,
        share_b: &str,
        user_id_hash: &str,
    ) -> Result<ShamirShareResponse, WalletError> {
        let body = serde_json::json!({
            "methodType": method_type,
            "payload": payload,
            "shareB": share_b,
            "userIdHash": user_id_hash,
        });
        self.post_json("/share/store", &body).await
    }

    /// Retrieve a Shamir share (Share B) from the server (POST /share/retrieve).
    ///
    /// Requires OTP verification.
    pub async fn retrieve_share(
        &self,
        method_type: &str,
        payload: serde_json::Value,
        user_id_hash: &str,
    ) -> Result<ShamirShareResponse, WalletError> {
        let body = serde_json::json!({
            "methodType": method_type,
            "payload": payload,
            "userIdHash": user_id_hash,
        });
        self.post_json("/share/retrieve", &body).await
    }

    /// Update a Shamir share for key rotation (POST /share/update).
    ///
    /// Requires OTP verification.
    pub async fn update_share(
        &self,
        method_type: &str,
        payload: serde_json::Value,
        user_id_hash: &str,
        new_share_b: &str,
    ) -> Result<ShamirShareResponse, WalletError> {
        let body = serde_json::json!({
            "methodType": method_type,
            "payload": payload,
            "userIdHash": user_id_hash,
            "newShareB": new_share_b,
        });
        self.post_json("/share/update", &body).await
    }

    /// Delete a Shamir user's account and stored share (POST /share/delete).
    ///
    /// Requires OTP verification.
    pub async fn delete_shamir_user(
        &self,
        method_type: &str,
        payload: serde_json::Value,
        user_id_hash: &str,
    ) -> Result<DeleteUserResponse, WalletError> {
        let body = serde_json::json!({
            "methodType": method_type,
            "payload": payload,
            "userIdHash": user_id_hash,
        });
        self.post_json("/share/delete", &body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wab_client_new() {
        let client = WABClient::new("http://localhost:3000/");
        assert_eq!(client.server_url(), "http://localhost:3000");

        let client2 = WABClient::new("http://example.com");
        assert_eq!(client2.server_url(), "http://example.com");

        let client3 = WABClient::new("http://example.com///");
        assert_eq!(client3.server_url(), "http://example.com");
    }

    #[test]
    fn test_generate_random_presentation_key() {
        let key = WABClient::generate_random_presentation_key().unwrap();
        // A hex-encoded 256-bit private key should be 64 characters
        assert_eq!(key.len(), 64, "Presentation key should be 64 hex chars, got {}", key.len());
        // Verify it is valid hex
        assert!(
            key.chars().all(|c| c.is_ascii_hexdigit()),
            "Presentation key should be valid hex, got: {}",
            key
        );
    }

    #[test]
    fn test_generate_random_presentation_key_uniqueness() {
        let key1 = WABClient::generate_random_presentation_key().unwrap();
        let key2 = WABClient::generate_random_presentation_key().unwrap();
        assert_ne!(key1, key2, "Two random keys should not be identical");
    }
}
