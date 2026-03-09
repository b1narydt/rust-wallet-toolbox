//! Permission token CRUD operations via inner wallet's WalletInterface.
//!
//! All operations use `admin_originator` to bypass permission checks on the
//! inner wallet (per CONTEXT.md locked decision: "Token CRUD operations go
//! through inner wallet's WalletInterface, not direct storage").

use chrono::Utc;

use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, DecryptArgs, EncryptArgs,
    ListOutputsArgs, OutputInclude, QueryMode, RelinquishOutputArgs, WalletInterface,
};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

use super::originator::{build_originator_lookup_values, normalize_originator};
use super::types::{
    GroupedPermissions, PermissionRequest, PermissionResponse, PermissionToken, PermissionType,
    BASKET_DBAP, BASKET_DCAP, BASKET_DPACP, BASKET_DSAP,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Protocol used for encrypting permission token fields.
fn perm_token_encryption_protocol() -> Protocol {
    Protocol {
        security_level: 2,
        protocol: "admin permission token encryption".to_string(),
    }
}

/// Counterparty::Self_ for token operations.
fn self_counterparty() -> Counterparty {
    Counterparty {
        counterparty_type: CounterpartyType::Self_,
        public_key: None,
    }
}

// ---------------------------------------------------------------------------
// Token field encryption/decryption
// ---------------------------------------------------------------------------

/// Encrypt a single token field using the admin permission token encryption protocol.
async fn encrypt_token_field(
    inner: &(dyn WalletInterface + Send + Sync),
    plaintext: &str,
    admin_originator: &str,
) -> Result<Vec<u8>, bsv::wallet::error::WalletError> {
    let result = inner
        .encrypt(
            EncryptArgs {
                protocol_id: perm_token_encryption_protocol(),
                key_id: "1".to_string(),
                counterparty: self_counterparty(),
                plaintext: plaintext.as_bytes().to_vec(),
                privileged: false,
                privileged_reason: None,
                seek_permission: Some(false),
            },
            Some(admin_originator),
        )
        .await?;
    Ok(result.ciphertext)
}

/// Decrypt a single token field using the admin permission token encryption protocol.
async fn decrypt_token_field(
    inner: &(dyn WalletInterface + Send + Sync),
    ciphertext: &[u8],
    admin_originator: &str,
) -> Result<String, bsv::wallet::error::WalletError> {
    match inner
        .decrypt(
            DecryptArgs {
                protocol_id: perm_token_encryption_protocol(),
                key_id: "1".to_string(),
                counterparty: self_counterparty(),
                ciphertext: ciphertext.to_vec(),
                privileged: false,
                privileged_reason: None,
                seek_permission: Some(false),
            },
            Some(admin_originator),
        )
        .await
    {
        Ok(result) => Ok(String::from_utf8_lossy(&result.plaintext).to_string()),
        Err(_) => {
            // On decryption failure, return the raw bytes as lossy UTF-8 (matching TS behavior).
            Ok(String::from_utf8_lossy(ciphertext).to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Token lookup filters
// ---------------------------------------------------------------------------

/// Filters for looking up permission tokens.
#[derive(Debug, Clone, Default)]
pub struct TokenLookupFilters {
    /// Protocol name filter for DPACP tokens.
    pub protocol: Option<String>,
    /// Security level filter for DPACP tokens.
    pub security_level: Option<u8>,
    /// Counterparty filter for DPACP tokens.
    pub counterparty: Option<String>,
    /// Basket name filter for DBAP tokens.
    pub basket_name: Option<String>,
    /// Certificate type filter for DCAP tokens.
    pub cert_type: Option<String>,
    /// Verifier public key filter for DCAP tokens.
    pub verifier: Option<String>,
    /// Whether this is a privileged operation (for DPACP/DCAP).
    pub privileged: Option<bool>,
    /// Whether to include expired tokens.
    pub include_expired: bool,
}

// ---------------------------------------------------------------------------
// Helper: basket name from permission type
// ---------------------------------------------------------------------------

/// Get the admin basket name for a given permission type.
fn basket_for_type(pt: PermissionType) -> &'static str {
    match pt {
        PermissionType::ProtocolPermission => BASKET_DPACP,
        PermissionType::BasketAccess => BASKET_DBAP,
        PermissionType::CertificateAccess => BASKET_DCAP,
        PermissionType::SpendingAuthorization => BASKET_DSAP,
    }
}

/// Check if a token's expiry timestamp means it is expired.
fn is_token_expired(expiry: u64) -> bool {
    if expiry == 0 {
        return false; // 0 means never expires
    }
    let now = Utc::now().timestamp() as u64;
    expiry < now
}

// ---------------------------------------------------------------------------
// Token creation
// ---------------------------------------------------------------------------

/// Create an on-chain permission token as an encrypted PushDrop UTXO.
///
/// Uses the inner wallet's `create_action` to store the token in the
/// appropriate admin basket with originator and type-specific tags.
pub async fn create_permission_token(
    inner: &(dyn WalletInterface + Send + Sync),
    request: &PermissionRequest,
    expiry: Option<u64>,
    admin_originator: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let expiry_val = expiry.unwrap_or(0);
    let basket = basket_for_type(request.permission_type).to_string();

    // Build encrypted PushDrop fields based on permission type.
    let fields = build_pushdrop_fields(inner, request, expiry_val, admin_originator).await?;

    // Build tags for token lookup.
    let tags = build_tags_for_request(request);

    // Concatenate fields into a simple locking script representation.
    // In a full implementation this would use PushDrop.lock() with a derived key.
    // For now, we store the concatenated encrypted fields as the locking script
    // since actual PushDrop template usage requires the full signing key flow
    // which Plan 05 will integrate with caching and callbacks.
    let mut script_data: Vec<u8> = Vec::new();
    for field in &fields {
        // Length-prefix each field for later parsing
        let len = field.len() as u32;
        script_data.extend_from_slice(&len.to_le_bytes());
        script_data.extend_from_slice(field);
    }

    inner
        .create_action(
            CreateActionArgs {
                description: format!("Grant {} permission", permission_type_label(request.permission_type)),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(script_data),
                    satoshis: 1,
                    output_description: format!(
                        "{} permission token for {}",
                        permission_type_label(request.permission_type),
                        request.originator
                    ),
                    basket: Some(basket),
                    custom_instructions: None,
                    tags,
                }],
                lock_time: None,
                version: None,
                labels: vec![],
                options: Some(CreateActionOptions {
                    accept_delayed_broadcast: Some(true).into(),
                    ..Default::default()
                }),
                reference: None,
            },
            Some(admin_originator),
        )
        .await?;

    Ok(())
}

/// Human-readable label for a permission type.
fn permission_type_label(pt: PermissionType) -> &'static str {
    match pt {
        PermissionType::ProtocolPermission => "protocol",
        PermissionType::BasketAccess => "basket",
        PermissionType::CertificateAccess => "certificate",
        PermissionType::SpendingAuthorization => "spending",
    }
}

/// Build encrypted PushDrop fields for a permission request.
async fn build_pushdrop_fields(
    inner: &(dyn WalletInterface + Send + Sync),
    request: &PermissionRequest,
    expiry: u64,
    admin_originator: &str,
) -> Result<Vec<Vec<u8>>, bsv::wallet::error::WalletError> {
    match request.permission_type {
        PermissionType::ProtocolPermission => {
            let protocol = request.protocol.as_deref().unwrap_or("");
            let sec_level = request.security_level.unwrap_or(0);
            let counterparty = if sec_level == 2 {
                request.counterparty.as_deref().unwrap_or("self")
            } else {
                ""
            };
            let privileged = request.privileged.unwrap_or(false);
            Ok(vec![
                encrypt_token_field(inner, &request.originator, admin_originator).await?,
                encrypt_token_field(inner, &expiry.to_string(), admin_originator).await?,
                encrypt_token_field(
                    inner,
                    if privileged { "true" } else { "false" },
                    admin_originator,
                )
                .await?,
                encrypt_token_field(inner, &sec_level.to_string(), admin_originator).await?,
                encrypt_token_field(inner, protocol, admin_originator).await?,
                encrypt_token_field(inner, counterparty, admin_originator).await?,
            ])
        }
        PermissionType::BasketAccess => {
            let basket = request.basket_name.as_deref().unwrap_or("");
            Ok(vec![
                encrypt_token_field(inner, &request.originator, admin_originator).await?,
                encrypt_token_field(inner, &expiry.to_string(), admin_originator).await?,
                encrypt_token_field(inner, basket, admin_originator).await?,
            ])
        }
        PermissionType::CertificateAccess => {
            let cert_type = request.cert_type.as_deref().unwrap_or("");
            let fields_json = match &request.cert_fields {
                Some(f) => serde_json::to_string(f).unwrap_or_else(|_| "[]".to_string()),
                None => "[]".to_string(),
            };
            let verifier = request.verifier.as_deref().unwrap_or("");
            let privileged = request.privileged.unwrap_or(false);
            Ok(vec![
                encrypt_token_field(inner, &request.originator, admin_originator).await?,
                encrypt_token_field(inner, &expiry.to_string(), admin_originator).await?,
                encrypt_token_field(
                    inner,
                    if privileged { "true" } else { "false" },
                    admin_originator,
                )
                .await?,
                encrypt_token_field(inner, cert_type, admin_originator).await?,
                encrypt_token_field(inner, &fields_json, admin_originator).await?,
                encrypt_token_field(inner, verifier, admin_originator).await?,
            ])
        }
        PermissionType::SpendingAuthorization => {
            let amount = request.amount.unwrap_or(0);
            Ok(vec![
                encrypt_token_field(inner, &request.originator, admin_originator).await?,
                encrypt_token_field(inner, &amount.to_string(), admin_originator).await?,
            ])
        }
    }
}

/// Build tags for a permission request (used during creation and lookup).
fn build_tags_for_request(request: &PermissionRequest) -> Vec<String> {
    let mut tags = vec![format!("originator {}", request.originator)];

    match request.permission_type {
        PermissionType::ProtocolPermission => {
            let privileged = request.privileged.unwrap_or(false);
            tags.push(format!("privileged {}", privileged));
            if let Some(ref proto) = request.protocol {
                tags.push(format!("protocolName {}", proto));
            }
            if let Some(sec) = request.security_level {
                tags.push(format!("protocolSecurityLevel {}", sec));
                if sec == 2 {
                    let cpty = request.counterparty.as_deref().unwrap_or("self");
                    tags.push(format!("counterparty {}", cpty));
                }
            }
        }
        PermissionType::BasketAccess => {
            if let Some(ref basket) = request.basket_name {
                tags.push(format!("basket {}", basket));
            }
        }
        PermissionType::CertificateAccess => {
            let privileged = request.privileged.unwrap_or(false);
            tags.push(format!("privileged {}", privileged));
            if let Some(ref ct) = request.cert_type {
                tags.push(format!("type {}", ct));
            }
            if let Some(ref v) = request.verifier {
                tags.push(format!("verifier {}", v));
            }
        }
        PermissionType::SpendingAuthorization => {
            // Only originator tag needed.
        }
    }

    tags
}

// ---------------------------------------------------------------------------
// Token lookup
// ---------------------------------------------------------------------------

/// Look up permission tokens of a given type for an originator, applying optional filters.
///
/// Uses `inner.list_outputs()` with basket and tag filters, then decodes and
/// decrypts each output's PushDrop fields into `PermissionToken` structs.
pub async fn lookup_permission_tokens(
    inner: &(dyn WalletInterface + Send + Sync),
    permission_type: PermissionType,
    originator: &str,
    admin_originator: &str,
    filters: &TokenLookupFilters,
) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
    let basket = basket_for_type(permission_type).to_string();
    let lookup_values = build_originator_lookup_values(originator);
    let normalized = normalize_originator(originator);

    let mut all_tokens: Vec<PermissionToken> = Vec::new();
    let mut seen_outpoints: std::collections::HashSet<String> = std::collections::HashSet::new();

    for origin_tag in &lookup_values {
        let mut tags = vec![format!("originator {}", origin_tag)];

        // Add type-specific filter tags.
        match permission_type {
            PermissionType::ProtocolPermission => {
                if let Some(ref priv_flag) = filters.privileged {
                    tags.push(format!("privileged {}", priv_flag));
                }
                if let Some(ref proto) = filters.protocol {
                    tags.push(format!("protocolName {}", proto));
                }
                if let Some(sec) = filters.security_level {
                    tags.push(format!("protocolSecurityLevel {}", sec));
                }
                if let Some(ref cpty) = filters.counterparty {
                    if filters.security_level == Some(2) {
                        tags.push(format!("counterparty {}", cpty));
                    }
                }
            }
            PermissionType::BasketAccess => {
                if let Some(ref bn) = filters.basket_name {
                    tags.push(format!("basket {}", bn));
                }
            }
            PermissionType::CertificateAccess => {
                if let Some(ref priv_flag) = filters.privileged {
                    tags.push(format!("privileged {}", priv_flag));
                }
                if let Some(ref ct) = filters.cert_type {
                    tags.push(format!("type {}", ct));
                }
                if let Some(ref v) = filters.verifier {
                    tags.push(format!("verifier {}", v));
                }
            }
            PermissionType::SpendingAuthorization => {
                // Only originator tag needed.
            }
        }

        let result = inner
            .list_outputs(
                ListOutputsArgs {
                    basket: basket.clone(),
                    tags,
                    tag_query_mode: Some(QueryMode::All),
                    include: Some(OutputInclude::EntireTransactions),
                    include_custom_instructions: Some(false).into(),
                    include_tags: Some(false).into(),
                    include_labels: Some(false).into(),
                    limit: Some(10000),
                    offset: None,
                    seek_permission: Some(false).into(),
                },
                Some(admin_originator),
            )
            .await?;

        for output in &result.outputs {
            if seen_outpoints.contains(&output.outpoint) {
                continue;
            }

            // Parse the locking script to extract PushDrop fields.
            let script = match &output.locking_script {
                Some(s) => s.clone(),
                None => continue,
            };

            let fields = parse_length_prefixed_fields(&script);

            match permission_type {
                PermissionType::ProtocolPermission => {
                    if fields.len() < 6 {
                        continue;
                    }
                    let domain = decrypt_token_field(inner, &fields[0], admin_originator).await?;
                    let norm_domain = normalize_originator(&domain);
                    if norm_domain != normalized {
                        continue;
                    }
                    let expiry_str =
                        decrypt_token_field(inner, &fields[1], admin_originator).await?;
                    let expiry_val = expiry_str.parse::<u64>().unwrap_or(0);
                    let priv_str =
                        decrypt_token_field(inner, &fields[2], admin_originator).await?;
                    let privileged = priv_str == "true";
                    let sec_str =
                        decrypt_token_field(inner, &fields[3], admin_originator).await?;
                    let sec_level = sec_str.parse::<u8>().unwrap_or(0);
                    let proto =
                        decrypt_token_field(inner, &fields[4], admin_originator).await?;
                    let cpty =
                        decrypt_token_field(inner, &fields[5], admin_originator).await?;

                    if !filters.include_expired && is_token_expired(expiry_val) {
                        continue;
                    }

                    let (txid, output_index) = parse_outpoint(&output.outpoint);
                    seen_outpoints.insert(output.outpoint.clone());
                    all_tokens.push(PermissionToken {
                        txid,
                        tx: result.beef.clone(),
                        output_index,
                        output_script: script.clone(),
                        satoshis: output.satoshis,
                        originator: norm_domain,
                        raw_originator: Some(domain),
                        expiry: Some(expiry_val),
                        privileged: Some(privileged),
                        protocol: Some(proto),
                        security_level: Some(sec_level),
                        counterparty: Some(cpty),
                        basket_name: None,
                        cert_type: None,
                        cert_fields: None,
                        verifier: None,
                        authorized_amount: None,
                    });
                }
                PermissionType::BasketAccess => {
                    if fields.len() < 3 {
                        continue;
                    }
                    let domain = decrypt_token_field(inner, &fields[0], admin_originator).await?;
                    let norm_domain = normalize_originator(&domain);
                    if norm_domain != normalized {
                        continue;
                    }
                    let expiry_str =
                        decrypt_token_field(inner, &fields[1], admin_originator).await?;
                    let expiry_val = expiry_str.parse::<u64>().unwrap_or(0);
                    let basket_decoded =
                        decrypt_token_field(inner, &fields[2], admin_originator).await?;

                    if !filters.include_expired && is_token_expired(expiry_val) {
                        continue;
                    }

                    let (txid, output_index) = parse_outpoint(&output.outpoint);
                    seen_outpoints.insert(output.outpoint.clone());
                    all_tokens.push(PermissionToken {
                        txid,
                        tx: result.beef.clone(),
                        output_index,
                        output_script: script.clone(),
                        satoshis: output.satoshis,
                        originator: norm_domain,
                        raw_originator: Some(domain),
                        expiry: Some(expiry_val),
                        privileged: None,
                        protocol: None,
                        security_level: None,
                        counterparty: None,
                        basket_name: Some(basket_decoded),
                        cert_type: None,
                        cert_fields: None,
                        verifier: None,
                        authorized_amount: None,
                    });
                }
                PermissionType::CertificateAccess => {
                    if fields.len() < 6 {
                        continue;
                    }
                    let domain = decrypt_token_field(inner, &fields[0], admin_originator).await?;
                    let norm_domain = normalize_originator(&domain);
                    if norm_domain != normalized {
                        continue;
                    }
                    let expiry_str =
                        decrypt_token_field(inner, &fields[1], admin_originator).await?;
                    let expiry_val = expiry_str.parse::<u64>().unwrap_or(0);
                    let priv_str =
                        decrypt_token_field(inner, &fields[2], admin_originator).await?;
                    let privileged = priv_str == "true";
                    let cert_type_decoded =
                        decrypt_token_field(inner, &fields[3], admin_originator).await?;
                    let fields_json =
                        decrypt_token_field(inner, &fields[4], admin_originator).await?;
                    let verifier_decoded =
                        decrypt_token_field(inner, &fields[5], admin_originator).await?;

                    if !filters.include_expired && is_token_expired(expiry_val) {
                        continue;
                    }

                    let (txid, output_index) = parse_outpoint(&output.outpoint);
                    seen_outpoints.insert(output.outpoint.clone());
                    all_tokens.push(PermissionToken {
                        txid,
                        tx: result.beef.clone(),
                        output_index,
                        output_script: script.clone(),
                        satoshis: output.satoshis,
                        originator: norm_domain,
                        raw_originator: Some(domain),
                        expiry: Some(expiry_val),
                        privileged: Some(privileged),
                        protocol: None,
                        security_level: None,
                        counterparty: None,
                        basket_name: None,
                        cert_type: Some(cert_type_decoded),
                        cert_fields: Some(fields_json),
                        verifier: Some(verifier_decoded),
                        authorized_amount: None,
                    });
                }
                PermissionType::SpendingAuthorization => {
                    if fields.len() < 2 {
                        continue;
                    }
                    let domain = decrypt_token_field(inner, &fields[0], admin_originator).await?;
                    let norm_domain = normalize_originator(&domain);
                    if norm_domain != normalized {
                        continue;
                    }
                    let amt_str =
                        decrypt_token_field(inner, &fields[1], admin_originator).await?;
                    let authorized_amount = amt_str.parse::<u64>().unwrap_or(0);

                    let (txid, output_index) = parse_outpoint(&output.outpoint);
                    seen_outpoints.insert(output.outpoint.clone());
                    all_tokens.push(PermissionToken {
                        txid,
                        tx: result.beef.clone(),
                        output_index,
                        output_script: script.clone(),
                        satoshis: output.satoshis,
                        originator: norm_domain,
                        raw_originator: Some(domain),
                        expiry: Some(0), // DSAP tokens do not expire
                        privileged: None,
                        protocol: None,
                        security_level: None,
                        counterparty: None,
                        basket_name: None,
                        cert_type: None,
                        cert_fields: None,
                        verifier: None,
                        authorized_amount: Some(authorized_amount),
                    });
                }
            }
        }
    }

    Ok(all_tokens)
}

// ---------------------------------------------------------------------------
// Token revocation
// ---------------------------------------------------------------------------

/// Revoke a single permission token by relinquishing its output.
pub async fn revoke_permission_token(
    inner: &(dyn WalletInterface + Send + Sync),
    txid: &str,
    output_index: u32,
    basket_name: &str,
    admin_originator: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    let outpoint = format!("{}.{}", txid, output_index);
    inner
        .relinquish_output(
            RelinquishOutputArgs {
                basket: basket_name.to_string(),
                output: outpoint,
            },
            Some(admin_originator),
        )
        .await?;
    Ok(())
}

/// Revoke all permission tokens for an originator across all 4 baskets.
///
/// Returns the number of tokens revoked.
pub async fn revoke_all_for_originator(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    admin_originator: &str,
) -> Result<u32, bsv::wallet::error::WalletError> {
    let empty_filters = TokenLookupFilters {
        include_expired: true,
        ..Default::default()
    };
    let mut count = 0u32;

    let types = [
        (PermissionType::ProtocolPermission, BASKET_DPACP),
        (PermissionType::BasketAccess, BASKET_DBAP),
        (PermissionType::CertificateAccess, BASKET_DCAP),
        (PermissionType::SpendingAuthorization, BASKET_DSAP),
    ];

    for (perm_type, basket) in types {
        let tokens =
            lookup_permission_tokens(inner, perm_type, originator, admin_originator, &empty_filters)
                .await?;
        for token in &tokens {
            if let Err(e) = revoke_permission_token(
                inner,
                &token.txid,
                token.output_index,
                basket,
                admin_originator,
            )
            .await
            {
                tracing::warn!(
                    "Failed to revoke token {}.{}: {}",
                    token.txid,
                    token.output_index,
                    e
                );
            } else {
                count += 1;
            }
        }
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Permission granting/denying public methods
// ---------------------------------------------------------------------------

/// Grant a permission by creating an on-chain token.
pub async fn grant_permission(
    inner: &(dyn WalletInterface + Send + Sync),
    request: &PermissionRequest,
    expiry: Option<u64>,
    admin_originator: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    create_permission_token(inner, request, expiry, admin_originator).await
}

/// Deny a permission request, returning a Deny response with a formatted reason.
pub fn deny_permission(request: &PermissionRequest) -> PermissionResponse {
    let reason = match request.permission_type {
        PermissionType::ProtocolPermission => format!(
            "Protocol permission denied for {} (protocol: {})",
            request.originator,
            request.protocol.as_deref().unwrap_or("unknown")
        ),
        PermissionType::BasketAccess => format!(
            "Basket access denied for {} (basket: {})",
            request.originator,
            request.basket_name.as_deref().unwrap_or("unknown")
        ),
        PermissionType::CertificateAccess => format!(
            "Certificate access denied for {} (type: {})",
            request.originator,
            request.cert_type.as_deref().unwrap_or("unknown")
        ),
        PermissionType::SpendingAuthorization => format!(
            "Spending authorization denied for {} (amount: {})",
            request.originator,
            request.amount.unwrap_or(0)
        ),
    };
    PermissionResponse::Deny { reason }
}

/// Grant all permissions in a grouped permission set.
pub async fn grant_grouped_permission(
    inner: &(dyn WalletInterface + Send + Sync),
    grouped: &GroupedPermissions,
    expiry: Option<u64>,
    admin_originator: &str,
) -> Result<(), bsv::wallet::error::WalletError> {
    for req in &grouped.protocol_permissions {
        grant_permission(inner, req, expiry, admin_originator).await?;
    }
    for req in &grouped.basket_access {
        grant_permission(inner, req, expiry, admin_originator).await?;
    }
    for req in &grouped.certificate_access {
        grant_permission(inner, req, expiry, admin_originator).await?;
    }
    if let Some(ref req) = grouped.spending_authorization {
        grant_permission(inner, req, expiry, admin_originator).await?;
    }
    Ok(())
}

/// Deny all permissions in a grouped permission set.
pub fn deny_grouped_permission(grouped: &GroupedPermissions) -> Vec<PermissionResponse> {
    let mut responses = Vec::new();
    for req in &grouped.protocol_permissions {
        responses.push(deny_permission(req));
    }
    for req in &grouped.basket_access {
        responses.push(deny_permission(req));
    }
    for req in &grouped.certificate_access {
        responses.push(deny_permission(req));
    }
    if let Some(ref req) = grouped.spending_authorization {
        responses.push(deny_permission(req));
    }
    responses
}

// ---------------------------------------------------------------------------
// Permission querying
// ---------------------------------------------------------------------------

/// List all protocol permission tokens (DPACP) for an originator.
pub async fn list_protocol_permissions(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    admin_originator: &str,
) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
    lookup_permission_tokens(
        inner,
        PermissionType::ProtocolPermission,
        originator,
        admin_originator,
        &TokenLookupFilters::default(),
    )
    .await
}

/// Check if an originator has a specific protocol permission.
pub async fn has_protocol_permission(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    protocol: &str,
    security_level: u8,
    counterparty: &str,
    admin_originator: &str,
) -> Result<bool, bsv::wallet::error::WalletError> {
    let filters = TokenLookupFilters {
        protocol: Some(protocol.to_string()),
        security_level: Some(security_level),
        counterparty: if security_level == 2 {
            Some(counterparty.to_string())
        } else {
            None
        },
        ..Default::default()
    };
    let tokens = lookup_permission_tokens(
        inner,
        PermissionType::ProtocolPermission,
        originator,
        admin_originator,
        &filters,
    )
    .await?;
    Ok(!tokens.is_empty())
}

/// List all basket access tokens (DBAP) for an originator.
pub async fn list_basket_access(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    admin_originator: &str,
) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
    lookup_permission_tokens(
        inner,
        PermissionType::BasketAccess,
        originator,
        admin_originator,
        &TokenLookupFilters::default(),
    )
    .await
}

/// Check if an originator has basket access permission.
pub async fn has_basket_access(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    basket_name: &str,
    admin_originator: &str,
) -> Result<bool, bsv::wallet::error::WalletError> {
    let filters = TokenLookupFilters {
        basket_name: Some(basket_name.to_string()),
        ..Default::default()
    };
    let tokens = lookup_permission_tokens(
        inner,
        PermissionType::BasketAccess,
        originator,
        admin_originator,
        &filters,
    )
    .await?;
    Ok(!tokens.is_empty())
}

/// List all certificate access tokens (DCAP) for an originator.
pub async fn list_certificate_access(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    admin_originator: &str,
) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
    lookup_permission_tokens(
        inner,
        PermissionType::CertificateAccess,
        originator,
        admin_originator,
        &TokenLookupFilters::default(),
    )
    .await
}

/// List all spending authorization tokens (DSAP) for an originator.
pub async fn list_spending_authorizations(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    admin_originator: &str,
) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
    lookup_permission_tokens(
        inner,
        PermissionType::SpendingAuthorization,
        originator,
        admin_originator,
        &TokenLookupFilters::default(),
    )
    .await
}

/// Query total satoshis spent by an originator since a given timestamp.
///
/// Uses `list_actions` to find actions from the originator and sum their
/// output satoshis within the time window.
pub async fn query_spent_since(
    inner: &(dyn WalletInterface + Send + Sync),
    originator: &str,
    since: u64,
    admin_originator: &str,
) -> Result<u64, bsv::wallet::error::WalletError> {
    use bsv::wallet::interfaces::ListActionsArgs;

    let labels = vec![
        format!("originator {}", originator),
        super::brc114::make_brc114_action_time_label(since),
    ];

    let result = inner
        .list_actions(
            ListActionsArgs {
                labels,
                label_query_mode: Some(QueryMode::All),
                include_labels: Some(false).into(),
                include_inputs: Some(false).into(),
                include_input_source_locking_scripts: Some(false).into(),
                include_input_unlocking_scripts: Some(false).into(),
                include_outputs: Some(true).into(),
                include_output_locking_scripts: Some(false).into(),
                limit: Some(10000),
                offset: None,
                seek_permission: Some(false).into(),
            },
            Some(admin_originator),
        )
        .await?;

    let mut total: u64 = 0;
    for action in &result.actions {
        for output in &action.outputs {
            total = total.saturating_add(output.satoshis);
        }
    }

    Ok(total)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an outpoint string "txid.index" into (txid, index).
fn parse_outpoint(outpoint: &str) -> (String, u32) {
    if let Some(dot_pos) = outpoint.rfind('.') {
        let txid = outpoint[..dot_pos].to_string();
        let index = outpoint[dot_pos + 1..].parse::<u32>().unwrap_or(0);
        (txid, index)
    } else {
        (outpoint.to_string(), 0)
    }
}

/// Parse length-prefixed fields from a script byte buffer.
///
/// Each field is preceded by a 4-byte little-endian length prefix.
fn parse_length_prefixed_fields(data: &[u8]) -> Vec<Vec<u8>> {
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + len > data.len() {
            break;
        }
        fields.push(data[offset..offset + len].to_vec());
        offset += len;
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basket_for_type() {
        assert_eq!(basket_for_type(PermissionType::ProtocolPermission), BASKET_DPACP);
        assert_eq!(basket_for_type(PermissionType::BasketAccess), BASKET_DBAP);
        assert_eq!(basket_for_type(PermissionType::CertificateAccess), BASKET_DCAP);
        assert_eq!(basket_for_type(PermissionType::SpendingAuthorization), BASKET_DSAP);
    }

    #[test]
    fn test_is_token_expired() {
        assert!(!is_token_expired(0));
        // A timestamp far in the past should be expired.
        assert!(is_token_expired(1000));
        // A timestamp far in the future should not be expired.
        assert!(!is_token_expired(u64::MAX / 2));
    }

    #[test]
    fn test_parse_outpoint() {
        let (txid, idx) = parse_outpoint("abc123.5");
        assert_eq!(txid, "abc123");
        assert_eq!(idx, 5);

        let (txid2, idx2) = parse_outpoint("noDotHere");
        assert_eq!(txid2, "noDotHere");
        assert_eq!(idx2, 0);
    }

    #[test]
    fn test_parse_length_prefixed_fields() {
        let mut data = Vec::new();
        // Field 1: "hello"
        data.extend_from_slice(&5u32.to_le_bytes());
        data.extend_from_slice(b"hello");
        // Field 2: "world!"
        data.extend_from_slice(&6u32.to_le_bytes());
        data.extend_from_slice(b"world!");

        let fields = parse_length_prefixed_fields(&data);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], b"hello");
        assert_eq!(fields[1], b"world!");
    }

    #[test]
    fn test_build_tags_for_request_protocol() {
        let req = PermissionRequest {
            permission_type: PermissionType::ProtocolPermission,
            originator: "example.com".to_string(),
            protocol: Some("my-proto".to_string()),
            security_level: Some(2),
            counterparty: Some("someone".to_string()),
            basket_name: None,
            cert_type: None,
            cert_fields: None,
            verifier: None,
            amount: None,
            description: None,
            labels: None,
            is_new_user: false,
            privileged: Some(false),
        };
        let tags = build_tags_for_request(&req);
        assert!(tags.contains(&"originator example.com".to_string()));
        assert!(tags.contains(&"privileged false".to_string()));
        assert!(tags.contains(&"protocolName my-proto".to_string()));
        assert!(tags.contains(&"protocolSecurityLevel 2".to_string()));
        assert!(tags.contains(&"counterparty someone".to_string()));
    }

    #[test]
    fn test_build_tags_for_request_basket() {
        let req = PermissionRequest {
            permission_type: PermissionType::BasketAccess,
            originator: "example.com".to_string(),
            protocol: None,
            security_level: None,
            counterparty: None,
            basket_name: Some("my-basket".to_string()),
            cert_type: None,
            cert_fields: None,
            verifier: None,
            amount: None,
            description: None,
            labels: None,
            is_new_user: false,
            privileged: None,
        };
        let tags = build_tags_for_request(&req);
        assert!(tags.contains(&"originator example.com".to_string()));
        assert!(tags.contains(&"basket my-basket".to_string()));
    }

    #[test]
    fn test_deny_permission() {
        let req = PermissionRequest {
            permission_type: PermissionType::ProtocolPermission,
            originator: "example.com".to_string(),
            protocol: Some("test".to_string()),
            security_level: None,
            counterparty: None,
            basket_name: None,
            cert_type: None,
            cert_fields: None,
            verifier: None,
            amount: None,
            description: None,
            labels: None,
            is_new_user: false,
            privileged: None,
        };
        let resp = deny_permission(&req);
        match resp {
            PermissionResponse::Deny { reason } => {
                assert!(reason.contains("Protocol permission denied"));
                assert!(reason.contains("example.com"));
            }
            _ => panic!("Expected Deny response"),
        }
    }

    #[test]
    fn test_deny_grouped_permission() {
        let grouped = GroupedPermissions {
            protocol_permissions: vec![PermissionRequest {
                permission_type: PermissionType::ProtocolPermission,
                originator: "example.com".to_string(),
                protocol: Some("test".to_string()),
                security_level: None,
                counterparty: None,
                basket_name: None,
                cert_type: None,
                cert_fields: None,
                verifier: None,
                amount: None,
                description: None,
                labels: None,
                is_new_user: false,
                privileged: None,
            }],
            basket_access: vec![],
            certificate_access: vec![],
            spending_authorization: None,
        };
        let responses = deny_grouped_permission(&grouped);
        assert_eq!(responses.len(), 1);
    }
}
