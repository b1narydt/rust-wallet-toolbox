//! Permission checking functions for wallet operations.
//!
//! Each `ensure_*` function follows the full permission resolution flow:
//! 1. Admin bypass (return Ok if originator is admin)
//! 2. Config flag check (skip if disabled)
//! 3. Cache check (manifest cache hit -> immediate result)
//! 4. Recent grant check (15-sec window -> Ok)
//! 5. Deduplication (concurrent identical requests -> wait for first)
//! 6. P-Module delegation (if protocol starts with "p ")
//! 7. Token lookup via `token_crud::lookup_permission_tokens`
//! 8. Fire callback if no token found -> Grant/EphemeralGrant/Deny

use super::cache::{DeduplicateResult, PermissionCache, MANIFEST_CACHE_TTL};
use super::callbacks::fire_permission_callback;
use super::config::PermissionsManagerConfig;
use super::originator::normalize_originator;
use super::p_module::detect_p_module_scheme;
use super::token_crud::{lookup_permission_tokens, query_spent_since, TokenLookupFilters};
use super::types::{PermissionRequest, PermissionResponse, PermissionType};
use super::WalletPermissionsManager;

/// Short TTL for ephemeral grants (30 seconds).
const EPHEMERAL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Config check helper
// ---------------------------------------------------------------------------

/// Determine if a config flag requires a check for the given usage type.
fn config_requires_protocol_check(config: &PermissionsManagerConfig, usage_type: &str) -> bool {
    match usage_type {
        "signing" => config.require_protocol_permission_for_signing,
        "encrypting" => config.require_protocol_permission_for_encrypting,
        "hmac" => config.require_protocol_permission_for_hmac,
        "publicKey" => config.require_protocol_permission_for_public_key,
        "signatureVerification" => config.require_protocol_permission_for_signature_verification,
        "hmacVerification" => config.require_protocol_permission_for_hmac_verification,
        "decryption" => config.require_protocol_permission_for_decryption,
        "keyLinkage" => config.require_protocol_permission_for_key_linkage,
        _ => true, // Default: require check
    }
}

// ---------------------------------------------------------------------------
// Admin protocol/basket detection
// ---------------------------------------------------------------------------

/// Check if a protocol is admin-reserved.
fn is_admin_protocol(protocol: &str) -> bool {
    protocol.starts_with("admin ")
}

/// Check if a basket name is admin-reserved.
fn is_admin_basket(basket: &str) -> bool {
    basket.starts_with("admin ")
}

/// Check if a label is admin-reserved.
fn is_admin_label(label: &str) -> bool {
    label.starts_with("admin ")
}

// ---------------------------------------------------------------------------
// Helper: resolve permission via cache + token + callback flow
// ---------------------------------------------------------------------------

/// The core resolution flow shared by all ensure* functions.
///
/// Steps after admin/config checks:
/// 1. Cache check
/// 2. Recent grant check
/// 3. Deduplication
/// 4. Token lookup
/// 5. Fire callback if no token
/// 6. Process callback response (Grant/EphemeralGrant/Deny)
async fn resolve_permission(
    mgr: &WalletPermissionsManager,
    cache_key: &str,
    permission_type: PermissionType,
    normalized_originator: &str,
    filters: &TokenLookupFilters,
    request: &PermissionRequest,
    denial_msg: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let perm_cache = mgr.cache();

    // Step 1: Cache check
    if let Some(granted) = perm_cache.check_cache(cache_key).await {
        if granted {
            return Ok(());
        } else {
            return Err(bsv::wallet::error::WalletError::NotImplemented(
                denial_msg.to_string(),
            ));
        }
    }

    // Step 2: Recent grant check
    if perm_cache.is_recent_grant(cache_key).await {
        return Ok(());
    }

    // Step 3: Deduplication
    match perm_cache.try_deduplicate(cache_key).await {
        DeduplicateResult::Wait(notify, result_holder) => {
            // Wait for the first caller to resolve
            notify.notified().await;
            let result_lock = result_holder.lock().await;
            match result_lock.as_ref() {
                Some(Ok(true)) => return Ok(()),
                Some(Ok(false)) => {
                    return Err(bsv::wallet::error::WalletError::NotImplemented(
                        denial_msg.to_string(),
                    ))
                }
                Some(Err(e)) => {
                    return Err(bsv::wallet::error::WalletError::NotImplemented(
                        e.clone(),
                    ))
                }
                None => {
                    return Err(bsv::wallet::error::WalletError::NotImplemented(
                        denial_msg.to_string(),
                    ))
                }
            }
        }
        DeduplicateResult::Proceed(_handle) => {
            // We are the first caller -- proceed with token lookup + callback
        }
    }

    // Step 4: Token lookup
    let tokens = lookup_permission_tokens(
        mgr.inner().as_ref(),
        permission_type,
        normalized_originator,
        &mgr.admin_originator(),
        filters,
    )
    .await;

    match tokens {
        Ok(ref t) if !t.is_empty() => {
            // Token found -- cache and complete
            perm_cache
                .insert_cache(cache_key, true, MANIFEST_CACHE_TTL)
                .await;
            perm_cache.record_recent_grant(cache_key).await;
            perm_cache.complete_request(cache_key, Ok(true)).await;
            return Ok(());
        }
        Err(e) => {
            // Token lookup error -- complete as failure
            perm_cache
                .complete_request(
                    cache_key,
                    Err(crate::WalletError::Internal(format!("{}", e))),
                )
                .await;
            return Err(e);
        }
        _ => {
            // No token found -- continue to callback
        }
    }

    // Step 5: Fire callback
    let response = fire_permission_callback(mgr.callbacks(), request).await;

    match response {
        Ok(PermissionResponse::Grant { expiry }) => {
            // Create on-chain token via grant_permission
            if let Err(e) = mgr.grant_permission(request, expiry).await {
                tracing::warn!("Failed to create on-chain permission token: {}", e);
                // Still treat as granted even if on-chain token creation fails
            }
            perm_cache
                .insert_cache(cache_key, true, MANIFEST_CACHE_TTL)
                .await;
            perm_cache.record_recent_grant(cache_key).await;
            perm_cache.complete_request(cache_key, Ok(true)).await;
            Ok(())
        }
        Ok(PermissionResponse::EphemeralGrant) => {
            // Cache as granted with short TTL, no on-chain token
            perm_cache
                .insert_cache(cache_key, true, EPHEMERAL_CACHE_TTL)
                .await;
            perm_cache.complete_request(cache_key, Ok(true)).await;
            Ok(())
        }
        Ok(PermissionResponse::Deny { reason }) => {
            perm_cache
                .insert_cache(cache_key, false, MANIFEST_CACHE_TTL)
                .await;
            perm_cache.complete_request(cache_key, Ok(false)).await;
            Err(bsv::wallet::error::WalletError::NotImplemented(reason))
        }
        Err(e) => {
            perm_cache
                .complete_request(
                    cache_key,
                    Err(crate::WalletError::Internal(format!("{}", e))),
                )
                .await;
            Err(bsv::wallet::error::WalletError::NotImplemented(format!(
                "Callback error: {}",
                e
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// ensure_protocol_permission
// ---------------------------------------------------------------------------

/// Ensure the originator has protocol usage permission (DPACP).
///
/// Flow: admin bypass -> security level 0 bypass -> admin protocol block ->
/// config flag check -> P-Module delegation -> cache -> token -> callback.
pub async fn ensure_protocol_permission(
    mgr: &WalletPermissionsManager,
    originator: &str,
    protocol: &str,
    security_level: u8,
    counterparty: &str,
    _privileged: bool,
    usage_type: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let normalized = normalize_originator(originator);

    // 1. Admin bypass
    if mgr.is_admin(Some(&normalized)) {
        return Ok(());
    }

    // 2. Security level 0 is open usage
    if security_level == 0 {
        return Ok(());
    }

    // Counterparty is empty for security level 1
    let effective_counterparty = if security_level == 1 {
        ""
    } else {
        counterparty
    };

    // 3. Block admin-reserved protocols
    if is_admin_protocol(protocol) {
        return Err(bsv::wallet::error::WalletError::NotImplemented(format!(
            "Protocol \"{}\" is admin-only.",
            protocol
        )));
    }

    // 4. Config flag check
    if !config_requires_protocol_check(mgr.config(), usage_type) {
        return Ok(());
    }

    // 5. P-Module delegation
    if let Some(scheme_id) = detect_p_module_scheme(protocol) {
        if mgr.has_permission_module(scheme_id) {
            return Ok(());
        }
    }

    // Build cache key and filters
    let details = format!("{}:{}:{}", protocol, security_level, effective_counterparty);
    let cache_key =
        PermissionCache::build_cache_key(&PermissionType::ProtocolPermission, &normalized, &details);

    let filters = TokenLookupFilters {
        protocol: Some(protocol.to_string()),
        security_level: Some(security_level),
        counterparty: if security_level == 2 {
            Some(effective_counterparty.to_string())
        } else {
            None
        },
        ..Default::default()
    };

    let request = PermissionRequest {
        permission_type: PermissionType::ProtocolPermission,
        originator: normalized.clone(),
        privileged: Some(_privileged),
        protocol: Some(protocol.to_string()),
        security_level: Some(security_level),
        counterparty: Some(effective_counterparty.to_string()),
        basket_name: None,
        cert_type: None,
        cert_fields: None,
        verifier: None,
        amount: None,
        description: Some(format!("Protocol permission for \"{}\" ({})", protocol, usage_type)),
        labels: None,
        is_new_user: false,
    };

    let denial_msg = format!(
        "Permission denied: protocol permission required for \"{}\" (originator: {})",
        protocol, normalized
    );

    resolve_permission(
        mgr,
        &cache_key,
        PermissionType::ProtocolPermission,
        &normalized,
        &filters,
        &request,
        &denial_msg,
    )
    .await
}

// ---------------------------------------------------------------------------
// ensure_basket_access
// ---------------------------------------------------------------------------

/// Ensure the originator has basket access permission (DBAP).
pub async fn ensure_basket_access(
    mgr: &WalletPermissionsManager,
    originator: &str,
    basket_name: &str,
    usage_type: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let normalized = normalize_originator(originator);

    if mgr.is_admin(Some(&normalized)) {
        return Ok(());
    }

    // Block admin-reserved baskets
    if is_admin_basket(basket_name) {
        return Err(bsv::wallet::error::WalletError::NotImplemented(format!(
            "Basket \"{}\" is admin-only.",
            basket_name
        )));
    }

    // Config flag check
    let should_check = match usage_type {
        "insertion" => mgr.config().require_basket_access_for_insertion,
        "listing" => mgr.config().require_basket_access_for_listing,
        "relinquishing" => mgr.config().require_basket_access_for_relinquishing,
        _ => true,
    };
    if !should_check {
        return Ok(());
    }

    let details = basket_name.to_string();
    let cache_key =
        PermissionCache::build_cache_key(&PermissionType::BasketAccess, &normalized, &details);

    let filters = TokenLookupFilters {
        basket_name: Some(basket_name.to_string()),
        ..Default::default()
    };

    let request = PermissionRequest {
        permission_type: PermissionType::BasketAccess,
        originator: normalized.clone(),
        privileged: None,
        protocol: None,
        security_level: None,
        counterparty: None,
        basket_name: Some(basket_name.to_string()),
        cert_type: None,
        cert_fields: None,
        verifier: None,
        amount: None,
        description: Some(format!("Basket access for \"{}\" ({})", basket_name, usage_type)),
        labels: None,
        is_new_user: false,
    };

    let denial_msg = format!(
        "Permission denied: basket access required for \"{}\" (originator: {})",
        basket_name, normalized
    );

    resolve_permission(
        mgr,
        &cache_key,
        PermissionType::BasketAccess,
        &normalized,
        &filters,
        &request,
        &denial_msg,
    )
    .await
}

// ---------------------------------------------------------------------------
// ensure_certificate_access
// ---------------------------------------------------------------------------

/// Ensure the originator has certificate access permission (DCAP).
pub async fn ensure_certificate_access(
    mgr: &WalletPermissionsManager,
    originator: &str,
    cert_type: &str,
    _cert_fields: Option<&[String]>,
    verifier: Option<&str>,
    _privileged: bool,
    usage_type: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let normalized = normalize_originator(originator);

    if mgr.is_admin(Some(&normalized)) {
        return Ok(());
    }

    // Config flag check
    let should_check = match usage_type {
        "acquisition" => mgr.config().require_certificate_access_for_acquisition,
        "listing" => mgr.config().require_certificate_access_for_listing,
        "proving" => mgr.config().require_certificate_access_for_proving,
        "relinquishing" => mgr.config().require_certificate_access_for_relinquishing,
        "discovery" => mgr.config().require_certificate_access_for_discovery,
        _ => true,
    };
    if !should_check {
        return Ok(());
    }

    let verifier_str = verifier.unwrap_or("");
    let details = format!("{}:{}", cert_type, verifier_str);
    let cache_key =
        PermissionCache::build_cache_key(&PermissionType::CertificateAccess, &normalized, &details);

    let filters = TokenLookupFilters {
        cert_type: Some(cert_type.to_string()),
        verifier: verifier.map(|v| v.to_string()),
        ..Default::default()
    };

    let request = PermissionRequest {
        permission_type: PermissionType::CertificateAccess,
        originator: normalized.clone(),
        privileged: Some(_privileged),
        protocol: None,
        security_level: None,
        counterparty: None,
        basket_name: None,
        cert_type: Some(cert_type.to_string()),
        cert_fields: _cert_fields.map(|f| f.to_vec()),
        verifier: verifier.map(|v| v.to_string()),
        amount: None,
        description: Some(format!(
            "Certificate access for type \"{}\" ({})",
            cert_type, usage_type
        )),
        labels: None,
        is_new_user: false,
    };

    let denial_msg = format!(
        "Permission denied: certificate access required for type \"{}\" (originator: {})",
        cert_type, normalized
    );

    resolve_permission(
        mgr,
        &cache_key,
        PermissionType::CertificateAccess,
        &normalized,
        &filters,
        &request,
        &denial_msg,
    )
    .await
}

// ---------------------------------------------------------------------------
// ensure_spending_authorization
// ---------------------------------------------------------------------------

/// Ensure the originator has spending authorization (DSAP) for the given amount.
pub async fn ensure_spending_authorization(
    mgr: &WalletPermissionsManager,
    originator: &str,
    amount: u64,
) -> Result<(), bsv::wallet::error::WalletError> {
    let normalized = normalize_originator(originator);

    if mgr.is_admin(Some(&normalized)) {
        return Ok(());
    }

    if !mgr.config().require_spending_authorization {
        return Ok(());
    }

    let cache_key = PermissionCache::build_cache_key(
        &PermissionType::SpendingAuthorization,
        &normalized,
        &format!("amount:{}", amount),
    );

    let perm_cache = mgr.cache();

    // Cache check for spending (only check recent grant -- spending amounts vary)
    if perm_cache.is_recent_grant(&cache_key).await {
        return Ok(());
    }

    let filters = TokenLookupFilters {
        include_expired: false,
        ..Default::default()
    };

    let tokens = lookup_permission_tokens(
        mgr.inner().as_ref(),
        PermissionType::SpendingAuthorization,
        &normalized,
        &mgr.admin_originator(),
        &filters,
    )
    .await?;

    // Find a token with sufficient authorized_amount
    for token in &tokens {
        if let Some(authorized_amount) = token.authorized_amount {
            let spent_so_far = query_spent_since(
                mgr.inner().as_ref(),
                &normalized,
                0,
                &mgr.admin_originator(),
            )
            .await
            .unwrap_or(0);

            if spent_so_far.saturating_add(amount) <= authorized_amount {
                perm_cache.record_recent_grant(&cache_key).await;
                return Ok(());
            }
        }
    }

    // No sufficient token found -- fire callback
    let request = PermissionRequest {
        permission_type: PermissionType::SpendingAuthorization,
        originator: normalized.clone(),
        privileged: None,
        protocol: None,
        security_level: None,
        counterparty: None,
        basket_name: None,
        cert_type: None,
        cert_fields: None,
        verifier: None,
        amount: Some(amount),
        description: Some(format!("Spending authorization for {} satoshis", amount)),
        labels: None,
        is_new_user: false,
    };

    let response = fire_permission_callback(mgr.callbacks(), &request).await;

    match response {
        Ok(PermissionResponse::Grant { expiry }) => {
            if let Err(e) = mgr.grant_permission(&request, expiry).await {
                tracing::warn!("Failed to create spending authorization token: {}", e);
            }
            perm_cache.record_recent_grant(&cache_key).await;
            Ok(())
        }
        Ok(PermissionResponse::EphemeralGrant) => {
            perm_cache.record_recent_grant(&cache_key).await;
            Ok(())
        }
        Ok(PermissionResponse::Deny { reason }) => {
            Err(bsv::wallet::error::WalletError::NotImplemented(reason))
        }
        Err(e) => Err(bsv::wallet::error::WalletError::NotImplemented(format!(
            "Callback error: {}",
            e
        ))),
    }
}

// ---------------------------------------------------------------------------
// ensure_label_access
// ---------------------------------------------------------------------------

/// Ensure the originator has label usage permission.
///
/// Uses the protocol permission system with a label-derived protocol name.
pub async fn ensure_label_access(
    mgr: &WalletPermissionsManager,
    originator: &str,
    labels: &[String],
    usage_type: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let normalized = normalize_originator(originator);

    if mgr.is_admin(Some(&normalized)) {
        return Ok(());
    }

    // Config flag check
    let should_check = match usage_type {
        "apply" => mgr.config().require_label_access_for_actions,
        "list" => mgr.config().require_label_access_for_listing,
        _ => true,
    };
    if !should_check {
        return Ok(());
    }

    for label in labels {
        // Block admin-reserved labels
        if is_admin_label(label) {
            return Err(bsv::wallet::error::WalletError::NotImplemented(format!(
                "Label \"{}\" is admin-only.",
                label
            )));
        }

        // Use protocol permission with label-derived protocol name
        let label_protocol = format!("action label {}", label);
        ensure_protocol_permission(
            mgr,
            originator,
            &label_protocol,
            1, // security level 1
            "self",
            false,
            "generic",
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_requires_protocol_check() {
        let config = PermissionsManagerConfig::default();
        assert!(config_requires_protocol_check(&config, "signing"));
        assert!(config_requires_protocol_check(&config, "encrypting"));
        assert!(config_requires_protocol_check(&config, "hmac"));
        assert!(config_requires_protocol_check(&config, "publicKey"));
        assert!(config_requires_protocol_check(&config, "keyLinkage"));
        assert!(config_requires_protocol_check(&config, "unknown"));
    }

    #[test]
    fn test_config_skips_when_disabled() {
        let mut config = PermissionsManagerConfig::default();
        config.require_protocol_permission_for_signing = false;
        assert!(!config_requires_protocol_check(&config, "signing"));
    }

    #[test]
    fn test_is_admin_protocol() {
        assert!(is_admin_protocol("admin permission token encryption"));
        assert!(is_admin_protocol("admin metadata encryption"));
        assert!(!is_admin_protocol("my-protocol"));
        assert!(!is_admin_protocol(""));
    }

    #[test]
    fn test_is_admin_basket() {
        assert!(is_admin_basket("admin protocol-permission"));
        assert!(!is_admin_basket("my-basket"));
    }

    #[test]
    fn test_is_admin_label() {
        assert!(is_admin_label("admin something"));
        assert!(!is_admin_label("my-label"));
    }
}
