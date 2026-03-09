//! Async callback trait for permission request events.
//!
//! Applications register a `PermissionCallbacks` implementation to receive
//! prompts when a permission check finds no stored or cached grant. The
//! manager dispatches to the appropriate method based on `PermissionType`.

use std::sync::Arc;

use async_trait::async_trait;

use super::types::{
    GroupedPermissions, PermissionRequest, PermissionResponse, PermissionType,
};
use crate::WalletError;

// ---------------------------------------------------------------------------
// PermissionCallbacks trait
// ---------------------------------------------------------------------------

/// Async trait for receiving permission request events.
///
/// Implementations present the request to the user (via UI, CLI, etc.) and
/// return the user's decision as a `PermissionResponse`.
#[async_trait]
pub trait PermissionCallbacks: Send + Sync {
    /// Called when a protocol permission is needed and no cached/stored grant exists.
    async fn on_protocol_permission_requested(
        &self,
        request: &PermissionRequest,
    ) -> Result<PermissionResponse, WalletError>;

    /// Called when basket access is needed.
    async fn on_basket_access_requested(
        &self,
        request: &PermissionRequest,
    ) -> Result<PermissionResponse, WalletError>;

    /// Called when certificate access is needed.
    async fn on_certificate_access_requested(
        &self,
        request: &PermissionRequest,
    ) -> Result<PermissionResponse, WalletError>;

    /// Called when spending authorization is needed.
    async fn on_spending_authorization_requested(
        &self,
        request: &PermissionRequest,
    ) -> Result<PermissionResponse, WalletError>;

    /// Called for grouped permission requests (BRC-73).
    async fn on_grouped_permission_requested(
        &self,
        grouped: &GroupedPermissions,
        originator: &str,
    ) -> Result<Vec<PermissionResponse>, WalletError>;

    /// Called for counterparty permission requests.
    async fn on_counterparty_permission_requested(
        &self,
        request: &PermissionRequest,
    ) -> Result<PermissionResponse, WalletError>;
}

// ---------------------------------------------------------------------------
// Helper: fire callback by permission type
// ---------------------------------------------------------------------------

/// Dispatch a permission request to the appropriate callback method.
///
/// If `callbacks` is `None`, returns `Deny` with a message indicating no
/// callback is registered. If the callback itself returns an error, the error
/// is swallowed and a `Deny` response is returned instead (matching the TS
/// `callEvent` error-swallowing pattern).
pub async fn fire_permission_callback(
    callbacks: &Option<Arc<dyn PermissionCallbacks>>,
    request: &PermissionRequest,
) -> Result<PermissionResponse, WalletError> {
    let cbs = match callbacks {
        Some(c) => c,
        None => {
            return Ok(PermissionResponse::Deny {
                reason: "No permission callback registered".to_string(),
            });
        }
    };

    let result = match request.permission_type {
        PermissionType::ProtocolPermission => {
            cbs.on_protocol_permission_requested(request).await
        }
        PermissionType::BasketAccess => {
            cbs.on_basket_access_requested(request).await
        }
        PermissionType::CertificateAccess => {
            cbs.on_certificate_access_requested(request).await
        }
        PermissionType::SpendingAuthorization => {
            cbs.on_spending_authorization_requested(request).await
        }
    };

    match result {
        Ok(resp) => Ok(resp),
        Err(e) => {
            tracing::warn!("Permission callback error (swallowed): {}", e);
            Ok(PermissionResponse::Deny {
                reason: format!("Callback error: {}", e),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test callback that always grants.
    struct AlwaysGrantCallbacks;

    #[async_trait]
    impl PermissionCallbacks for AlwaysGrantCallbacks {
        async fn on_protocol_permission_requested(
            &self,
            _request: &PermissionRequest,
        ) -> Result<PermissionResponse, WalletError> {
            Ok(PermissionResponse::Grant { expiry: None })
        }

        async fn on_basket_access_requested(
            &self,
            _request: &PermissionRequest,
        ) -> Result<PermissionResponse, WalletError> {
            Ok(PermissionResponse::Grant { expiry: None })
        }

        async fn on_certificate_access_requested(
            &self,
            _request: &PermissionRequest,
        ) -> Result<PermissionResponse, WalletError> {
            Ok(PermissionResponse::Grant { expiry: None })
        }

        async fn on_spending_authorization_requested(
            &self,
            _request: &PermissionRequest,
        ) -> Result<PermissionResponse, WalletError> {
            Ok(PermissionResponse::Grant { expiry: None })
        }

        async fn on_grouped_permission_requested(
            &self,
            _grouped: &GroupedPermissions,
            _originator: &str,
        ) -> Result<Vec<PermissionResponse>, WalletError> {
            Ok(vec![PermissionResponse::Grant { expiry: None }])
        }

        async fn on_counterparty_permission_requested(
            &self,
            _request: &PermissionRequest,
        ) -> Result<PermissionResponse, WalletError> {
            Ok(PermissionResponse::Grant { expiry: None })
        }
    }

    fn make_request(ptype: PermissionType) -> PermissionRequest {
        PermissionRequest {
            permission_type: ptype,
            originator: "test.example.com".to_string(),
            privileged: None,
            protocol: Some("test-proto".to_string()),
            security_level: Some(1),
            counterparty: None,
            basket_name: None,
            cert_type: None,
            cert_fields: None,
            verifier: None,
            amount: None,
            description: None,
            labels: None,
            is_new_user: false,
        }
    }

    #[tokio::test]
    async fn test_fire_callback_no_callbacks() {
        let req = make_request(PermissionType::ProtocolPermission);
        let resp = fire_permission_callback(&None, &req).await.unwrap();
        match resp {
            PermissionResponse::Deny { reason } => {
                assert!(reason.contains("No permission callback registered"));
            }
            _ => panic!("Expected Deny when no callbacks registered"),
        }
    }

    #[tokio::test]
    async fn test_fire_callback_dispatches_by_type() {
        let cbs: Option<Arc<dyn PermissionCallbacks>> =
            Some(Arc::new(AlwaysGrantCallbacks));

        for ptype in [
            PermissionType::ProtocolPermission,
            PermissionType::BasketAccess,
            PermissionType::CertificateAccess,
            PermissionType::SpendingAuthorization,
        ] {
            let req = make_request(ptype);
            let resp = fire_permission_callback(&cbs, &req).await.unwrap();
            assert!(matches!(resp, PermissionResponse::Grant { .. }));
        }
    }

    #[tokio::test]
    async fn test_fire_callback_swallows_errors() {
        struct FailingCallbacks;

        #[async_trait]
        impl PermissionCallbacks for FailingCallbacks {
            async fn on_protocol_permission_requested(
                &self,
                _request: &PermissionRequest,
            ) -> Result<PermissionResponse, WalletError> {
                Err(WalletError::Internal("intentional test failure".into()))
            }
            async fn on_basket_access_requested(
                &self,
                _request: &PermissionRequest,
            ) -> Result<PermissionResponse, WalletError> {
                Err(WalletError::Internal("fail".into()))
            }
            async fn on_certificate_access_requested(
                &self,
                _request: &PermissionRequest,
            ) -> Result<PermissionResponse, WalletError> {
                Err(WalletError::Internal("fail".into()))
            }
            async fn on_spending_authorization_requested(
                &self,
                _request: &PermissionRequest,
            ) -> Result<PermissionResponse, WalletError> {
                Err(WalletError::Internal("fail".into()))
            }
            async fn on_grouped_permission_requested(
                &self,
                _grouped: &GroupedPermissions,
                _originator: &str,
            ) -> Result<Vec<PermissionResponse>, WalletError> {
                Err(WalletError::Internal("fail".into()))
            }
            async fn on_counterparty_permission_requested(
                &self,
                _request: &PermissionRequest,
            ) -> Result<PermissionResponse, WalletError> {
                Err(WalletError::Internal("fail".into()))
            }
        }

        let cbs: Option<Arc<dyn PermissionCallbacks>> =
            Some(Arc::new(FailingCallbacks));
        let req = make_request(PermissionType::ProtocolPermission);
        let resp = fire_permission_callback(&cbs, &req).await.unwrap();
        assert!(matches!(resp, PermissionResponse::Deny { .. }));
    }
}
