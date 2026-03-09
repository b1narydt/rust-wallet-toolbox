//! P-Module delegation trait for BRC-98/99 patterns.
//!
//! Defines the `PermissionsModule` trait that allows external modules to
//! intercept and transform wallet requests/responses for protocols or baskets
//! using the `p <scheme_id> <rest>` naming convention.

use async_trait::async_trait;

use super::types::{PermissionRequest, PermissionResponse};
use crate::WalletError;

/// A permissions module handles request/response transformation for a specific
/// P-protocol or P-basket scheme under BRC-98/99.
///
/// Modules are registered by scheme ID in the `WalletPermissionsManager` and
/// are invoked when a protocol or basket name starts with the `"p "` prefix.
#[async_trait]
pub trait PermissionsModule: Send + Sync {
    /// Called when a permission request uses the `"p "` prefix scheme.
    ///
    /// The module can check permissions, modify arguments, or reject the
    /// request by returning an error.
    async fn on_request(
        &self,
        request: &PermissionRequest,
        originator: &str,
    ) -> Result<PermissionResponse, WalletError>;

    /// Called after the inner wallet returns a result, allowing transformation.
    ///
    /// The module can modify, filter, or enrich the response before it is
    /// returned to the caller.
    async fn on_response(
        &self,
        request: &PermissionRequest,
        result: serde_json::Value,
        originator: &str,
    ) -> Result<serde_json::Value, WalletError>;
}

/// Detect if a protocol name uses the P-module scheme.
///
/// If the protocol starts with `"p "`, returns the scheme ID after the prefix.
/// Otherwise returns `None`.
///
/// # Examples
///
/// ```
/// use bsv_wallet_toolbox::permissions::p_module::detect_p_module_scheme;
///
/// assert_eq!(detect_p_module_scheme("p custom-scheme"), Some("custom-scheme"));
/// assert_eq!(detect_p_module_scheme("p btms rest"), Some("btms rest"));
/// assert_eq!(detect_p_module_scheme("normal-protocol"), None);
/// assert_eq!(detect_p_module_scheme("p"), None);
/// assert_eq!(detect_p_module_scheme("p "), None);
/// ```
pub fn detect_p_module_scheme(protocol: &str) -> Option<&str> {
    if let Some(rest) = protocol.strip_prefix("p ") {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            None
        } else {
            Some(rest)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_p_module_scheme() {
        assert_eq!(
            detect_p_module_scheme("p custom-scheme"),
            Some("custom-scheme")
        );
        assert_eq!(detect_p_module_scheme("p btms"), Some("btms"));
        assert_eq!(detect_p_module_scheme("normal"), None);
        assert_eq!(detect_p_module_scheme("p"), None);
        assert_eq!(detect_p_module_scheme("p "), None);
        assert_eq!(detect_p_module_scheme("protocol"), None);
        assert_eq!(detect_p_module_scheme(""), None);
    }
}
