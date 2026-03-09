//! Twilio phone verification interactor.
//!
//! Client-side class that calls the WAB server for Twilio-based phone verification.
//! The server sends an SMS code to the user's phone using Twilio Verify.

use super::{post_auth_request, AuthMethodInteractor};
use crate::error::WalletError;
use crate::wab_client::types::{CompleteAuthResponse, StartAuthResponse};
use async_trait::async_trait;

/// Twilio phone verification auth method interactor.
///
/// Delegates to the WAB server for SMS-based OTP verification via Twilio Verify.
pub struct TwilioPhoneInteractor;

#[async_trait]
impl AuthMethodInteractor for TwilioPhoneInteractor {
    fn method_type(&self) -> &str {
        "TwilioPhone"
    }

    async fn start_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError> {
        post_auth_request(
            server_url,
            "/auth/start",
            self.method_type(),
            presentation_key,
            &payload,
        )
        .await
    }

    async fn complete_auth(
        &self,
        server_url: &str,
        presentation_key: &str,
        payload: serde_json::Value,
    ) -> Result<CompleteAuthResponse, WalletError> {
        post_auth_request(
            server_url,
            "/auth/complete",
            self.method_type(),
            presentation_key,
            &payload,
        )
        .await
    }
}
