//! Persona ID verification interactor.
//!
//! Client-side class that calls the WAB server for Persona-based identity verification.

use async_trait::async_trait;
use crate::error::WalletError;
use crate::wab_client::types::{CompleteAuthResponse, StartAuthResponse};
use super::{AuthMethodInteractor, post_auth_request};

/// Persona ID verification auth method interactor.
///
/// Delegates to the WAB server for Persona-based document/identity verification.
pub struct PersonaIDInteractor;

#[async_trait]
impl AuthMethodInteractor for PersonaIDInteractor {
    fn method_type(&self) -> &str {
        "PersonaID"
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
