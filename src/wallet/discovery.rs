//! Discovery methods for identity certificate lookup via overlay network.
//!
//! Implements `discover_by_identity_key` and `discover_by_attributes` with
//! 2-minute caching of both trust settings and overlay query results.
//! Ported from:
//!   - wallet-toolbox/src/Wallet.ts (lines 637-733)
//!   - wallet-toolbox/src/utility/identityUtils.ts

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bsv::auth::certificates::certificate::AuthCertificate;
use bsv::auth::certificates::verifiable::VerifiableCertificate;
use bsv::primitives::public_key::PublicKey;
use bsv::script::templates::push_drop::PushDrop;
use bsv::services::overlay_tools::{LookupAnswer, LookupQuestion, LookupResolver};
use bsv::transaction::beef::Beef;
use bsv::wallet::interfaces::{
    Certificate as SdkCertificate, DiscoverByAttributesArgs, DiscoverByIdentityKeyArgs,
    DiscoverCertificatesResult, IdentityCertificate, IdentityCertifier,
};
use bsv::wallet::proto_wallet::ProtoWallet;

use crate::error::WalletError;
use crate::wallet::settings::{TrustSettings, TrustedCertifier, WalletSettingsManager};

/// Cache TTL: 2 minutes (matching TS Wallet TTL_MS = 2 * 60 * 1000).
const CACHE_TTL: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// OverlayCache -- per-query and trust settings cache
// ---------------------------------------------------------------------------

/// Cache for overlay discovery queries and trust settings.
///
/// Both the trust settings and individual query results are cached with
/// a 2-minute TTL to avoid redundant overlay network calls.
pub struct OverlayCache {
    trust_settings_cache: tokio::sync::Mutex<Option<CacheEntry<TrustSettings>>>,
    query_cache: tokio::sync::Mutex<HashMap<String, CacheEntry<Vec<VerifiableCertificate>>>>,
}

/// A cached value with an expiration timestamp.
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl OverlayCache {
    /// Create a new empty OverlayCache.
    pub fn new() -> Self {
        OverlayCache {
            trust_settings_cache: tokio::sync::Mutex::new(None),
            query_cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// discover_by_identity_key
// ---------------------------------------------------------------------------

/// Discover certificates for a specific identity key via overlay lookup.
///
/// Steps:
/// 1. Get trust settings (from cache or settings manager)
/// 2. Extract sorted certifier identity keys
/// 3. Build cache key from fn name + identityKey + certifiers
/// 4. Check query cache; use if fresh
/// 5. Otherwise call query_overlay with the lookup resolver
/// 6. Cache the result
/// 7. Transform results with trust settings
///
/// Ported from Wallet.ts discoverByIdentityKey (lines 637-680).
pub async fn discover_by_identity_key(
    settings_manager: &WalletSettingsManager,
    lookup_resolver: &LookupResolver,
    cache: &OverlayCache,
    args: &DiscoverByIdentityKeyArgs,
) -> Result<DiscoverCertificatesResult, WalletError> {
    let trust_settings = get_trust_settings(settings_manager, cache).await?;
    let certifiers = sorted_certifier_keys(&trust_settings);

    // Build cache key
    let cache_key = serde_json::json!({
        "fn": "discoverByIdentityKey",
        "identityKey": args.identity_key.to_der_hex(),
        "certifiers": certifiers,
    })
    .to_string();

    // Check query cache
    let cached_certs = get_cached_query(cache, &cache_key).await;
    let certs = match cached_certs {
        Some(c) => c,
        None => {
            let query = serde_json::json!({
                "identityKey": args.identity_key.to_der_hex(),
                "certifiers": certifiers,
            });
            let result = query_overlay(query, lookup_resolver).await;
            let certs = result.unwrap_or_default();
            set_cached_query(cache, &cache_key, certs.clone()).await;
            certs
        }
    };

    if certs.is_empty() {
        return Ok(DiscoverCertificatesResult {
            total_certificates: 0,
            certificates: vec![],
        });
    }

    Ok(transform_verifiable_certificates_with_trust(
        &trust_settings,
        &certs,
    ))
}

// ---------------------------------------------------------------------------
// discover_by_attributes
// ---------------------------------------------------------------------------

/// Discover certificates matching specific attributes via overlay lookup.
///
/// Same pattern as discover_by_identity_key but with attributes instead
/// of identityKey. Attributes are sorted for stable cache keys.
///
/// Ported from Wallet.ts discoverByAttributes (lines 682-733).
pub async fn discover_by_attributes(
    settings_manager: &WalletSettingsManager,
    lookup_resolver: &LookupResolver,
    cache: &OverlayCache,
    args: &DiscoverByAttributesArgs,
) -> Result<DiscoverCertificatesResult, WalletError> {
    let trust_settings = get_trust_settings(settings_manager, cache).await?;
    let certifiers = sorted_certifier_keys(&trust_settings);

    // Normalize attributes for stable cache key (sorted keys)
    let mut sorted_attrs: Vec<(&String, &String)> = args.attributes.iter().collect();
    sorted_attrs.sort_by_key(|(k, _)| k.as_str());
    let attributes_key: serde_json::Value = serde_json::json!(
        sorted_attrs
            .iter()
            .map(|(k, v)| ((*k).clone(), (*v).clone()))
            .collect::<HashMap<String, String>>()
    );

    // Build cache key
    let cache_key = serde_json::json!({
        "fn": "discoverByAttributes",
        "attributes": attributes_key,
        "certifiers": certifiers,
    })
    .to_string();

    // Check query cache
    let cached_certs = get_cached_query(cache, &cache_key).await;
    let certs = match cached_certs {
        Some(c) => c,
        None => {
            let query = serde_json::json!({
                "attributes": &args.attributes,
                "certifiers": certifiers,
            });
            let result = query_overlay(query, lookup_resolver).await;
            let certs = result.unwrap_or_default();
            set_cached_query(cache, &cache_key, certs.clone()).await;
            certs
        }
    };

    if certs.is_empty() {
        return Ok(DiscoverCertificatesResult {
            total_certificates: 0,
            certificates: vec![],
        });
    }

    Ok(transform_verifiable_certificates_with_trust(
        &trust_settings,
        &certs,
    ))
}

// ---------------------------------------------------------------------------
// query_overlay -- port of identityUtils.ts queryOverlay
// ---------------------------------------------------------------------------

/// Query the overlay network for identity certificates.
///
/// Sends a lookup query to the `ls_identity` service and parses the
/// returned UTXOs into VerifiableCertificates by decoding PushDrop
/// outputs and decrypting with an "anyone" ProtoWallet.
///
/// Ported from identityUtils.ts queryOverlay + parseResults.
async fn query_overlay(
    query: serde_json::Value,
    resolver: &LookupResolver,
) -> Result<Vec<VerifiableCertificate>, WalletError> {
    let question = LookupQuestion {
        service: "ls_identity".to_string(),
        query,
    };

    let answer = resolver.query(&question, None).await.map_err(|e| {
        WalletError::Internal(format!("Overlay lookup failed: {}", e))
    })?;

    parse_results(answer).await
}

/// Parse overlay lookup results into VerifiableCertificates.
///
/// For each output in the lookup answer:
/// 1. Parse the transaction from BEEF
/// 2. Decode the PushDrop output script
/// 3. Parse the JSON certificate from the first field
/// 4. Build a VerifiableCertificate
/// 5. Decrypt fields using an "anyone" ProtoWallet
/// 6. Verify the certificate signature
///
/// Errors on individual outputs are silently skipped (matching TS behavior).
///
/// Ported from identityUtils.ts parseResults.
async fn parse_results(
    answer: LookupAnswer,
) -> Result<Vec<VerifiableCertificate>, WalletError> {
    let outputs = match answer {
        LookupAnswer::OutputList { outputs } => outputs,
        _ => return Ok(vec![]),
    };

    // Create an "anyone" ProtoWallet for decryption (matches TS: new ProtoWallet('anyone'))
    let anyone_key = bsv::primitives::private_key::PrivateKey::from_hex("01")
        .unwrap_or_else(|_| {
            // Fallback: use a deterministic key
            bsv::primitives::private_key::PrivateKey::from_hex(
                "0000000000000000000000000000000000000000000000000000000000000001",
            )
            .expect("valid private key")
        });
    let anyone_wallet = ProtoWallet::new(anyone_key);

    let mut parsed = Vec::new();

    for output in &outputs {
        // Each output error is silently skipped (matching TS try/catch behavior)
        if let Ok(cert) = parse_single_output(output, &anyone_wallet).await {
            parsed.push(cert);
        }
    }

    Ok(parsed)
}

/// Parse a single overlay output into a VerifiableCertificate.
async fn parse_single_output(
    output: &bsv::services::overlay_tools::LookupOutputEntry,
    anyone_wallet: &ProtoWallet,
) -> Result<VerifiableCertificate, WalletError> {
    // Parse BEEF and extract the target transaction using SDK 0.1.6 into_transaction
    let beef = Beef::from_binary(&mut std::io::Cursor::new(&output.beef)).map_err(|e| {
        WalletError::Internal(format!("Failed to parse BEEF: {}", e))
    })?;

    let tx = beef.into_transaction().map_err(|e| {
        WalletError::Internal(format!("Failed to get transaction from BEEF: {}", e))
    })?;

    // Get the output at the specified index
    let tx_output = tx
        .outputs
        .get(output.output_index as usize)
        .ok_or_else(|| {
            WalletError::Internal(format!(
                "Output index {} out of range",
                output.output_index
            ))
        })?;

    // Decode PushDrop fields from the locking script using SDK 0.1.6 PushDrop::decode
    let push_drop = PushDrop::decode(&tx_output.locking_script)
        .map_err(|e| WalletError::Internal(format!("PushDrop decode failed: {}", e)))?;
    let fields = push_drop.fields;

    if fields.is_empty() {
        return Err(WalletError::Internal(
            "No PushDrop fields found in locking script".to_string(),
        ));
    }

    // Parse JSON certificate from the first field
    let cert_json = String::from_utf8(fields[0].clone()).map_err(|e| {
        WalletError::Internal(format!("Invalid UTF-8 in certificate field: {}", e))
    })?;

    let cert_data: serde_json::Value = serde_json::from_str(&cert_json).map_err(|e| {
        WalletError::Internal(format!("Invalid JSON in certificate: {}", e))
    })?;

    // Build the SDK Certificate from parsed JSON
    let cert_type_str = cert_data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let serial_str = cert_data
        .get("serialNumber")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let subject_str = cert_data
        .get("subject")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let certifier_str = cert_data
        .get("certifier")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let revocation_str = cert_data
        .get("revocationOutpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let fields: Option<HashMap<String, String>> = cert_data
        .get("fields")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let keyring: HashMap<String, String> = cert_data
        .get("keyring")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let signature: Option<Vec<u8>> = cert_data
        .get("signature")
        .and_then(|v| v.as_str())
        .map(|s| hex_decode(s));

    // Parse cert_type as base64-decoded 32-byte array
    let cert_type_bytes = base64_decode_to_32(cert_type_str);
    let serial_bytes = base64_decode_to_32(serial_str);

    let subject = PublicKey::from_string(subject_str).map_err(|e| {
        WalletError::Internal(format!("Invalid subject key: {}", e))
    })?;
    let certifier = PublicKey::from_string(certifier_str).map_err(|e| {
        WalletError::Internal(format!("Invalid certifier key: {}", e))
    })?;

    let sdk_cert = SdkCertificate {
        cert_type: bsv::wallet::interfaces::CertificateType(cert_type_bytes),
        serial_number: bsv::wallet::interfaces::SerialNumber(serial_bytes),
        subject,
        certifier,
        revocation_outpoint: revocation_str,
        fields,
        signature,
    };

    // Build VerifiableCertificate and decrypt fields
    let mut verifiable = VerifiableCertificate::new(sdk_cert, keyring);

    // Decrypt fields using "anyone" wallet (publicly revealed fields)
    let decrypted = verifiable
        .decrypt_fields(anyone_wallet)
        .await
        .map_err(|e| {
            WalletError::Internal(format!("Field decryption failed: {}", e))
        })?;
    verifiable.decrypted_fields = Some(decrypted);

    // Verify the certificate signature
    let sig_valid =
        AuthCertificate::verify(&verifiable.certificate, anyone_wallet)
            .await
            .map_err(|e| {
                WalletError::Internal(format!("Certificate verification failed: {}", e))
            })?;

    if !sig_valid {
        return Err(WalletError::Internal(
            "Certificate signature invalid".to_string(),
        ));
    }

    Ok(verifiable)
}

// ---------------------------------------------------------------------------
// transform_verifiable_certificates_with_trust
// ---------------------------------------------------------------------------

/// Transform verified certificates with trust settings.
///
/// Groups certificates by subject, accumulates trust from matching certifiers,
/// filters out groups below the trust threshold, and annotates each certificate
/// with certifier information.
///
/// Ported from identityUtils.ts transformVerifiableCertificatesWithTrust.
fn transform_verifiable_certificates_with_trust(
    trust_settings: &TrustSettings,
    certificates: &[VerifiableCertificate],
) -> DiscoverCertificatesResult {
    // Group certificates by subject while accumulating trust
    let mut identity_groups: HashMap<String, IdentityGroup> = HashMap::new();
    let mut certifier_cache: HashMap<String, &TrustedCertifier> = HashMap::new();

    for cert in certificates {
        let subject = cert.certificate.subject.to_der_hex();
        let certifier = cert.certificate.certifier.to_der_hex();

        // Lookup certifier in trust settings (skip if not trusted)
        if !certifier_cache.contains_key(&certifier) {
            if let Some(found) = trust_settings
                .trusted_certifiers
                .iter()
                .find(|c| c.identity_key == certifier)
            {
                certifier_cache.insert(certifier.clone(), found);
            } else {
                continue; // Skip untrusted certifier
            }
        }

        let trusted = certifier_cache[&certifier];

        // Build IdentityCertifier for the output
        let certifier_info = IdentityCertifier {
            name: trusted.name.clone(),
            icon_url: trusted.icon_url.clone().unwrap_or_default(),
            description: trusted.description.clone(),
            trust: trusted.trust as u8,
        };

        // Build IdentityCertificate
        let identity_cert = IdentityCertificate {
            certificate: cert.certificate.clone(),
            certifier_info: certifier_info.clone(),
            publicly_revealed_keyring: cert.keyring.clone(),
            decrypted_fields: cert
                .decrypted_fields
                .clone()
                .unwrap_or_default(),
        };

        let group = identity_groups
            .entry(subject)
            .or_insert_with(|| IdentityGroup {
                total_trust: 0,
                members: vec![],
            });
        group.total_trust += trusted.trust;
        group.members.push(identity_cert);
    }

    // Filter groups below trust threshold and flatten
    let mut final_results: Vec<IdentityCertificate> = Vec::new();
    for group in identity_groups.values() {
        if group.total_trust >= trust_settings.trust_level {
            final_results.extend(group.members.clone());
        }
    }

    // Sort by certifier trust descending
    final_results.sort_by(|a, b| b.certifier_info.trust.cmp(&a.certifier_info.trust));

    DiscoverCertificatesResult {
        total_certificates: final_results.len() as u32,
        certificates: final_results,
    }
}

/// Helper struct for grouping certificates by subject.
struct IdentityGroup {
    total_trust: u32,
    members: Vec<IdentityCertificate>,
}

// ---------------------------------------------------------------------------
// Cache helpers
// ---------------------------------------------------------------------------

/// Get trust settings from cache or settings manager.
async fn get_trust_settings(
    settings_manager: &WalletSettingsManager,
    cache: &OverlayCache,
) -> Result<TrustSettings, WalletError> {
    let mut ts_cache = cache.trust_settings_cache.lock().await;
    if let Some(ref entry) = *ts_cache {
        if entry.expires_at > Instant::now() {
            return Ok(entry.value.clone());
        }
    }

    let settings = settings_manager.get().await?;
    let trust = settings.trust_settings;
    *ts_cache = Some(CacheEntry {
        value: trust.clone(),
        expires_at: Instant::now() + CACHE_TTL,
    });
    Ok(trust)
}

/// Extract sorted certifier identity keys from trust settings.
fn sorted_certifier_keys(trust_settings: &TrustSettings) -> Vec<String> {
    let mut keys: Vec<String> = trust_settings
        .trusted_certifiers
        .iter()
        .map(|c| c.identity_key.clone())
        .collect();
    keys.sort();
    keys
}

/// Get a cached query result if fresh.
async fn get_cached_query(
    cache: &OverlayCache,
    key: &str,
) -> Option<Vec<VerifiableCertificate>> {
    let qc = cache.query_cache.lock().await;
    if let Some(entry) = qc.get(key) {
        if entry.expires_at > Instant::now() {
            return Some(entry.value.clone());
        }
    }
    None
}

/// Store a query result in the cache.
async fn set_cached_query(
    cache: &OverlayCache,
    key: &str,
    value: Vec<VerifiableCertificate>,
) {
    let mut qc = cache.query_cache.lock().await;
    qc.insert(
        key.to_string(),
        CacheEntry {
            value,
            expires_at: Instant::now() + CACHE_TTL,
        },
    );
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

/// Decode a base64 string into a 32-byte array (zero-padded or truncated).
fn base64_decode_to_32(s: &str) -> [u8; 32] {
    let decoded = base64_decode_bytes(s);
    let mut arr = [0u8; 32];
    let len = decoded.len().min(32);
    arr[..len].copy_from_slice(&decoded[..len]);
    arr
}

/// Decode base64 bytes.
fn base64_decode_bytes(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}

/// Decode hex string to bytes.
fn hex_decode(s: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let hex = s.as_bytes();
    let mut i = 0;
    while i + 1 < hex.len() {
        let hi = hex_nibble(hex[i]);
        let lo = hex_nibble(hex[i + 1]);
        bytes.push((hi << 4) | lo);
        i += 2;
    }
    bytes
}

fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_cache_new() {
        let cache = OverlayCache::new();
        // Just verify construction succeeds
        drop(cache);
    }

    #[test]
    fn test_sorted_certifier_keys() {
        let trust = TrustSettings {
            trust_level: 2,
            trusted_certifiers: vec![
                TrustedCertifier {
                    name: "B".to_string(),
                    description: "".to_string(),
                    identity_key: "bbb".to_string(),
                    trust: 3,
                    icon_url: None,
                    base_url: None,
                },
                TrustedCertifier {
                    name: "A".to_string(),
                    description: "".to_string(),
                    identity_key: "aaa".to_string(),
                    trust: 4,
                    icon_url: None,
                    base_url: None,
                },
            ],
        };
        let keys = sorted_certifier_keys(&trust);
        assert_eq!(keys, vec!["aaa".to_string(), "bbb".to_string()]);
    }

    #[test]
    fn test_base64_decode_to_32() {
        let arr = base64_decode_to_32("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert_eq!(arr, [0u8; 32]);
    }

    #[test]
    fn test_transform_empty_certificates() {
        let trust = TrustSettings {
            trust_level: 2,
            trusted_certifiers: vec![],
        };
        let result = transform_verifiable_certificates_with_trust(&trust, &[]);
        assert_eq!(result.total_certificates, 0);
        assert!(result.certificates.is_empty());
    }
}
