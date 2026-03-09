//! Authentication state types for the WalletAuthenticationManager.
//!
//! Provides `AuthState`, `StateSnapshot`, `Profile`, and the `WalletBuilderFn`
//! type alias used to construct a wallet after authentication completes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::wab_client::types::LinkedMethod;
use crate::wallet::privileged::PrivilegedKeyManager;
use crate::WalletError;
use bsv::wallet::interfaces::WalletInterface;

/// Current authentication state of the manager.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthState {
    /// No authentication has been attempted.
    Unauthenticated,
    /// Authentication flow is in progress.
    Authenticating,
    /// Successfully authenticated; inner wallet available.
    Authenticated,
    /// Authentication failed with an error description.
    Failed(String),
}

/// Serializable snapshot of authentication state for persistence across restarts.
///
/// This struct is serialized to JSON bytes and passed as the `state_snapshot`
/// parameter to reconstruct manager state without re-authenticating.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot {
    /// Hex-encoded presentation key, if available.
    pub presentation_key: Option<String>,
    /// Current auth state.
    pub auth_state: AuthState,
    /// User profile, if authenticated.
    pub profile: Option<Profile>,
    /// Whether this is a new user (None if unknown).
    pub is_new_user: Option<bool>,
}

/// User profile information derived from authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// Server-assigned user ID.
    pub user_id: String,
    /// Hex-encoded identity public key.
    pub identity_key: String,
    /// Hex-encoded presentation key.
    pub presentation_key: String,
    /// Authentication methods linked to this user.
    pub linked_methods: Vec<LinkedMethod>,
}

/// Type alias for the async closure that constructs a wallet from key material.
///
/// Called after authentication succeeds with the derived root key bytes and
/// a privileged key manager. Returns the inner wallet that will handle all
/// subsequent WalletInterface calls.
pub type WalletBuilderFn = Box<
    dyn Fn(
            Vec<u8>,
            Arc<dyn PrivilegedKeyManager>,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Arc<dyn WalletInterface + Send + Sync>, WalletError>>
                    + Send,
            >,
        > + Send
        + Sync,
>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_serialization() {
        // Round-trip each variant through JSON
        let states = vec![
            AuthState::Unauthenticated,
            AuthState::Authenticating,
            AuthState::Authenticated,
            AuthState::Failed("test error".to_string()),
        ];

        for state in &states {
            let json = serde_json::to_string(state).expect("serialize AuthState");
            let back: AuthState = serde_json::from_str(&json).expect("deserialize AuthState");
            assert_eq!(&back, state, "AuthState round-trip failed for {:?}", state);
        }
    }

    #[test]
    fn test_state_snapshot_serialization() {
        let snapshot = StateSnapshot {
            presentation_key: Some("abcd1234".to_string()),
            auth_state: AuthState::Authenticated,
            profile: Some(Profile {
                user_id: "user-1".to_string(),
                identity_key: "deadbeef".to_string(),
                presentation_key: "abcd1234".to_string(),
                linked_methods: vec![],
            }),
            is_new_user: Some(false),
        };

        let json = serde_json::to_vec(&snapshot).expect("serialize StateSnapshot");
        let back: StateSnapshot = serde_json::from_slice(&json).expect("deserialize StateSnapshot");

        assert_eq!(back.auth_state, AuthState::Authenticated);
        assert_eq!(back.presentation_key.as_deref(), Some("abcd1234"));
        assert_eq!(back.is_new_user, Some(false));
        assert!(back.profile.is_some());
        let prof = back.profile.unwrap();
        assert_eq!(prof.user_id, "user-1");
        assert_eq!(prof.identity_key, "deadbeef");
    }
}
