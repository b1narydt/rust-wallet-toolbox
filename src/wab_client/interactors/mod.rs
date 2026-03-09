//! Authentication method interactors for WAB client flows.
//!
//! Defines the [`AuthMethodInteractor`] trait and concrete implementations
//! for different authentication methods (Twilio, Persona, DevConsole).

pub mod dev_console;
pub mod persona;
pub mod twilio;

use crate::error::WalletError;
use crate::wab_client::types::{CompleteAuthResponse, StartAuthResponse};
use async_trait::async_trait;

pub use dev_console::DevConsoleInteractor;
pub use persona::PersonaIDInteractor;
pub use twilio::TwilioPhoneInteractor;

/// Trait for client-side authentication method interactors.
///
/// Each interactor knows how to call the WAB server for a specific
/// authentication method's start and complete flows. The only difference
/// between interactors is the `method_type` string; the HTTP logic is shared.
#[async_trait]
pub trait AuthMethodInteractor: Send + Sync {
    /// Returns the WAB method type string (e.g. "TwilioPhone", "PersonaID", "DevConsole").
    fn method_type(&self) -> &str;

    /// Starts an auth flow by POSTing to the WAB server's /auth/start endpoint.
    async fn start_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError>;

    /// Completes an auth flow by POSTing to the WAB server's /auth/complete endpoint.
    async fn complete_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<CompleteAuthResponse, WalletError>;
}

/// Shared HTTP helper for auth interactors.
///
/// POSTs to `{server_url}{path}` with a JSON body containing the method type,
/// presentation key, and payload. Deserializes the response as type `T`.
pub(crate) async fn post_auth_request<T: serde::de::DeserializeOwned>(
    server_url: &str,
    path: &str,
    method_type: &str,
    presentation_key: &str,
    payload: &serde_json::Value,
) -> Result<T, WalletError> {
    let url = format!("{}{}", server_url.trim_end_matches('/'), path);
    let body = serde_json::json!({
        "methodType": method_type,
        "presentationKey": presentation_key,
        "payload": payload,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .json(&body)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twilio_method_type() {
        let interactor = TwilioPhoneInteractor;
        assert_eq!(interactor.method_type(), "TwilioPhone");
    }

    #[test]
    fn test_persona_method_type() {
        let interactor = PersonaIDInteractor;
        assert_eq!(interactor.method_type(), "PersonaID");
    }

    #[test]
    fn test_dev_console_method_type() {
        let interactor = DevConsoleInteractor;
        assert_eq!(interactor.method_type(), "DevConsole");
    }
}
