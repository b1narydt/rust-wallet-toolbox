//! Configuration for the WalletPermissionsManager.
//!
//! Defines the `PermissionsManagerConfig` struct with boolean flags controlling
//! how strictly permissions are enforced. All flags default to `true` (most
//! secure configuration).

use serde::{Deserialize, Serialize};

/// Configuration controlling which permission checks the `WalletPermissionsManager`
/// enforces.
///
/// All flags default to `true` (most secure). Set individual flags to `false` to
/// skip specific permission checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsManagerConfig {
    /// Require protocol permission for `create_signature` / `verify_signature`.
    pub require_protocol_permission_for_signing: bool,
    /// Require protocol permission for `encrypt` / `decrypt`.
    pub require_protocol_permission_for_encrypting: bool,
    /// Require protocol permission for `create_hmac` / `verify_hmac`.
    pub require_protocol_permission_for_hmac: bool,
    /// Require protocol permission for key linkage revelation.
    pub require_protocol_permission_for_key_linkage: bool,
    /// Require protocol permission for `get_public_key`.
    pub require_protocol_permission_for_public_key: bool,
    /// Require protocol permission for `verify_signature`.
    pub require_protocol_permission_for_signature_verification: bool,
    /// Require protocol permission for `verify_hmac`.
    pub require_protocol_permission_for_hmac_verification: bool,
    /// Require protocol permission for `decrypt`.
    pub require_protocol_permission_for_decryption: bool,
    /// Require basket access permission for basket insertion.
    pub require_basket_access_for_insertion: bool,
    /// Require basket access permission for `list_outputs`.
    pub require_basket_access_for_listing: bool,
    /// Require basket access permission for `relinquish_output`.
    pub require_basket_access_for_relinquishing: bool,
    /// Require certificate access permission for `acquire_certificate`.
    pub require_certificate_access_for_acquisition: bool,
    /// Require certificate access permission for `list_certificates`.
    pub require_certificate_access_for_listing: bool,
    /// Require certificate access permission for `prove_certificate`.
    pub require_certificate_access_for_proving: bool,
    /// Require certificate access permission for `relinquish_certificate`.
    pub require_certificate_access_for_relinquishing: bool,
    /// Require certificate access permission for discovery methods.
    pub require_certificate_access_for_discovery: bool,
    /// Require spending authorization for `create_action` with net spend.
    pub require_spending_authorization: bool,
    /// Require label access permission when applying action labels.
    pub require_label_access_for_actions: bool,
    /// Require label access permission when listing actions by label.
    pub require_label_access_for_listing: bool,
    /// Encrypt wallet metadata (descriptions) before passing to inner wallet.
    pub encrypt_wallet_metadata: bool,
}

impl Default for PermissionsManagerConfig {
    fn default() -> Self {
        Self {
            require_protocol_permission_for_signing: true,
            require_protocol_permission_for_encrypting: true,
            require_protocol_permission_for_hmac: true,
            require_protocol_permission_for_key_linkage: true,
            require_protocol_permission_for_public_key: true,
            require_protocol_permission_for_signature_verification: true,
            require_protocol_permission_for_hmac_verification: true,
            require_protocol_permission_for_decryption: true,
            require_basket_access_for_insertion: true,
            require_basket_access_for_listing: true,
            require_basket_access_for_relinquishing: true,
            require_certificate_access_for_acquisition: true,
            require_certificate_access_for_listing: true,
            require_certificate_access_for_proving: true,
            require_certificate_access_for_relinquishing: true,
            require_certificate_access_for_discovery: true,
            require_spending_authorization: true,
            require_label_access_for_actions: true,
            require_label_access_for_listing: true,
            encrypt_wallet_metadata: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_all_true() {
        let config = PermissionsManagerConfig::default();
        assert!(config.require_protocol_permission_for_signing);
        assert!(config.require_protocol_permission_for_encrypting);
        assert!(config.require_protocol_permission_for_hmac);
        assert!(config.require_protocol_permission_for_key_linkage);
        assert!(config.require_protocol_permission_for_public_key);
        assert!(config.require_protocol_permission_for_signature_verification);
        assert!(config.require_protocol_permission_for_hmac_verification);
        assert!(config.require_protocol_permission_for_decryption);
        assert!(config.require_basket_access_for_insertion);
        assert!(config.require_basket_access_for_listing);
        assert!(config.require_basket_access_for_relinquishing);
        assert!(config.require_certificate_access_for_acquisition);
        assert!(config.require_certificate_access_for_listing);
        assert!(config.require_certificate_access_for_proving);
        assert!(config.require_certificate_access_for_relinquishing);
        assert!(config.require_certificate_access_for_discovery);
        assert!(config.require_spending_authorization);
        assert!(config.require_label_access_for_actions);
        assert!(config.require_label_access_for_listing);
        assert!(config.encrypt_wallet_metadata);
    }
}
