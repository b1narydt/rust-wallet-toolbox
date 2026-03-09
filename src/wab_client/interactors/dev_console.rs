//! DevConsole authentication interactor.
//!
//! A development-only auth method that generates OTP codes and logs them to
//! the server console. Used for testing without Twilio or Persona dependencies.

use async_trait::async_trait;
use crate::error::WalletError;
use crate::wab_client::types::{CompleteAuthResponse, StartAuthResponse};
use super::{AuthMethodInteractor, post_auth_request};

/// Development console auth method interactor.
///
/// Delegates to the WAB server which generates and logs OTP codes to the console.
/// This interactor is intended for development and testing only.
pub struct DevConsoleInteractor;

#[async_trait]
impl AuthMethodInteractor for DevConsoleInteractor {
    fn method_type(&self) -> &str {
        "DevConsole"
    }

    async fn start_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError> {
        post_auth_request(server_url, "/auth/start", self.method_type(), presentation_key, &payload).await
    }

    async fn complete_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<CompleteAuthResponse, WalletError> {
        post_auth_request(server_url, "/auth/complete", self.method_type(), presentation_key, &payload).await
    }
}
