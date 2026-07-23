//! User Management Protocol (UMP) token types and overlay interactions.
//!
//! Ports the `OverlayUMPTokenInteractor` from TypeScript. The UMP token is a
//! PushDrop UTXO that stores encrypted key material for multi-factor
//! authentication (password + presentation key + recovery key).

use serde::{Deserialize, Serialize};
use tracing::warn;

use bsv::script::locking_script::LockingScript;
use bsv::script::templates::push_drop::{decode as decode_push_drop, LockPosition, PushDrop};
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, ListOutputsArgs, OutputInclude,
    QueryMode, WalletInterface,
};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

use bsv::primitives::hash::sha256;
use bsv::primitives::random::random_bytes;
use bsv::primitives::symmetric_key::SymmetricKey;

use crate::auth_manager::cwi_logic::{derive_password_key, xor_keys};
use crate::WalletError;

/// Admin basket name for UMP tokens (matching TS constant).
pub const UMP_BASKET: &str = "admin user management token";

/// Protocol security level for UMP token operations.
pub const UMP_PROTOCOL_SECURITY_LEVEL: u8 = 2;

/// Protocol name for UMP token operations.
pub const UMP_PROTOCOL_NAME: &str = "admin user management token";

/// UMP on-chain token version carrying password-KDF metadata (Argon2id support).
pub const UMP_VERSION_V3: u8 = 3;

/// KDF identifier for legacy PBKDF2-HMAC-SHA512 password hardening.
pub const KDF_ALGORITHM_PBKDF2: &str = "pbkdf2-sha512";

/// KDF identifier for Argon2id (memory-hard) password hardening (UMP v3).
pub const KDF_ALGORITHM_ARGON2ID: &str = "argon2id";

/// Default Argon2id iteration (time) cost for new v3 tokens (matches TS
/// `ARGON2ID_DEFAULT_ITERATIONS`).
pub const ARGON2ID_DEFAULT_ITERATIONS: u32 = 7;

/// Default Argon2id memory cost in KiB (128 MiB) for new v3 tokens (matches TS
/// `ARGON2ID_DEFAULT_MEMORY_KIB`).
pub const ARGON2ID_DEFAULT_MEMORY_KIB: u32 = 131072;

/// Default Argon2id degree of parallelism for new v3 tokens (matches TS
/// `ARGON2ID_DEFAULT_PARALLELISM`).
pub const ARGON2ID_DEFAULT_PARALLELISM: u32 = 1;

/// Default Argon2id derived-key length in bytes for new v3 tokens (matches TS
/// `ARGON2ID_DEFAULT_HASH_LENGTH`).
pub const ARGON2ID_DEFAULT_HASH_LENGTH: u32 = 32;

/// Password key-derivation-function metadata carried by a UMP v3 token.
///
/// Mirrors the TS `UMPToken.passwordKdf`. `algorithm` is [`KDF_ALGORITHM_PBKDF2`]
/// or [`KDF_ALGORITHM_ARGON2ID`]. The Argon2id-only fields (`memory_kib`,
/// `parallelism`, `hash_length`) are `None` for PBKDF2 tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordKdf {
    /// KDF algorithm identifier ("pbkdf2-sha512" or "argon2id").
    pub algorithm: String,
    /// Iteration count (PBKDF2 rounds, or Argon2id time cost).
    pub iterations: u32,
    /// Argon2id memory cost in KiB (absent for PBKDF2).
    #[serde(rename = "memoryKiB", skip_serializing_if = "Option::is_none", default)]
    pub memory_kib: Option<u32>,
    /// Argon2id degree of parallelism (absent for PBKDF2).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parallelism: Option<u32>,
    /// Derived-key length in bytes (absent for PBKDF2; defaults to 32).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash_length: Option<u32>,
}

impl PasswordKdf {
    /// Default Argon2id KDF configuration for new UMP v3 tokens, matching the TS
    /// `kdfConfig` defaults (7 iterations, 128 MiB, parallelism 1, 32-byte output).
    pub fn argon2id_default() -> Self {
        Self {
            algorithm: KDF_ALGORITHM_ARGON2ID.to_string(),
            iterations: ARGON2ID_DEFAULT_ITERATIONS,
            memory_kib: Some(ARGON2ID_DEFAULT_MEMORY_KIB),
            parallelism: Some(ARGON2ID_DEFAULT_PARALLELISM),
            hash_length: Some(ARGON2ID_DEFAULT_HASH_LENGTH),
        }
    }
}

/// A User Management Protocol (UMP) token representing encrypted wallet key
/// material stored on-chain as a PushDrop UTXO.
///
/// The field layout matches the TypeScript `UMPToken` interface. Each field
/// is a byte array representing either encrypted keys, hashes, or salts.
///
/// Field order (matching PushDrop index):
/// 0. password_salt
/// 1. password_presentation_primary
/// 2. password_recovery_primary
/// 3. presentation_recovery_primary
/// 4. password_primary_privileged
/// 5. presentation_recovery_privileged
/// 6. presentation_hash
/// 7. recovery_hash
/// 8. presentation_key_encrypted
/// 9. password_key_encrypted
/// 10. recovery_key_encrypted
/// 11. profiles_encrypted (optional)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UMPToken {
    /// PBKDF2 salt used with the password to derive the password key.
    pub password_salt: Vec<u8>,
    /// Root primary key encrypted by XOR of password key and presentation key.
    pub password_presentation_primary: Vec<u8>,
    /// Root primary key encrypted by XOR of password key and recovery key.
    pub password_recovery_primary: Vec<u8>,
    /// Root primary key encrypted by XOR of presentation key and recovery key.
    pub presentation_recovery_primary: Vec<u8>,
    /// Root privileged key encrypted by XOR of password key and primary key.
    pub password_primary_privileged: Vec<u8>,
    /// Root privileged key encrypted by XOR of presentation key and recovery key.
    pub presentation_recovery_privileged: Vec<u8>,
    /// SHA-256 hash of the presentation key.
    pub presentation_hash: Vec<u8>,
    /// SHA-256 hash of the recovery key.
    pub recovery_hash: Vec<u8>,
    /// Presentation key encrypted with the root privileged key.
    pub presentation_key_encrypted: Vec<u8>,
    /// Password key encrypted with the root privileged key.
    pub password_key_encrypted: Vec<u8>,
    /// Recovery key encrypted with the root privileged key.
    pub recovery_key_encrypted: Vec<u8>,
    /// Optional encrypted profile data (JSON string encrypted with root privileged key).
    pub profiles_encrypted: Option<Vec<u8>>,
    /// On-chain UMP protocol version (3 for tokens carrying KDF metadata; `None`
    /// for legacy tokens).
    #[serde(default)]
    pub ump_version: Option<u8>,
    /// Password KDF metadata (present for UMP v3 tokens; `None` for legacy
    /// PBKDF2-SHA512/7777 tokens).
    #[serde(default)]
    pub password_kdf: Option<PasswordKdf>,
    /// On-chain outpoint where this token is located (e.g., "txid.0").
    pub current_outpoint: Option<String>,
}

/// Encode a UMP token's fields as a vector of byte arrays in the canonical order.
///
/// Returns 11 or 12 fields depending on whether `profiles_encrypted` is set.
fn encode_ump_token_fields(token: &UMPToken) -> Vec<Vec<u8>> {
    let mut fields = vec![
        token.password_salt.clone(),
        token.password_presentation_primary.clone(),
        token.password_recovery_primary.clone(),
        token.presentation_recovery_primary.clone(),
        token.password_primary_privileged.clone(),
        token.presentation_recovery_privileged.clone(),
        token.presentation_hash.clone(),
        token.recovery_hash.clone(),
        token.presentation_key_encrypted.clone(),
        token.password_key_encrypted.clone(),
        token.recovery_key_encrypted.clone(),
    ];
    if let Some(ref profiles) = token.profiles_encrypted {
        fields.push(profiles.clone());
    }
    // UMP v3: append KDF metadata. Layout mirrors TS `buildUMPTokenFields` — the
    // version byte lands at index 12 when profiles are present, else index 11,
    // followed by the algorithm (utf8) and the params JSON.
    if token.ump_version == Some(UMP_VERSION_V3) {
        if let Some(ref kdf) = token.password_kdf {
            fields.push(vec![UMP_VERSION_V3]);
            fields.push(kdf.algorithm.as_bytes().to_vec());
            fields.push(build_kdf_params_json(kdf).into_bytes());
        }
    }
    fields
}

/// Serialize UMP v3 KDF parameters to the compact JSON form TS `JSON.stringify`
/// emits (`iterations` first, then optional `memoryKiB`, `parallelism`,
/// `hashLength`), so the on-chain field is byte-identical across ports.
fn build_kdf_params_json(kdf: &PasswordKdf) -> String {
    let mut json = format!("{{\"iterations\":{}", kdf.iterations);
    if let Some(m) = kdf.memory_kib {
        json.push_str(&format!(",\"memoryKiB\":{m}"));
    }
    if let Some(p) = kdf.parallelism {
        json.push_str(&format!(",\"parallelism\":{p}"));
    }
    if let Some(h) = kdf.hash_length {
        json.push_str(&format!(",\"hashLength\":{h}"));
    }
    json.push('}');
    json
}

/// Look up a UMP token via wallet's list_outputs using a presentation key hash.
///
/// Queries the `admin user management token` basket with a tag filter matching
/// the presentation key hash. If an output is found, its locking script is
/// decoded as a PushDrop to extract UMP token fields.
///
/// Returns `None` for new users who have no UMP token on-chain.
///
/// # Arguments
/// * `wallet` - Wallet for making output queries
/// * `presentation_key_hash` - Hex-encoded SHA-256 hash of the presentation key
pub async fn lookup_ump_token(
    wallet: &(dyn WalletInterface + Send + Sync),
    presentation_key_hash: &str,
) -> Result<Option<UMPToken>, WalletError> {
    let result = wallet
        .list_outputs(
            ListOutputsArgs {
                basket: UMP_BASKET.to_string(),
                tags: vec![format!("presentationHash {}", presentation_key_hash)],
                tag_query_mode: Some(QueryMode::All),
                include: Some(OutputInclude::LockingScripts),
                include_custom_instructions: Some(false).into(),
                include_tags: Some(false).into(),
                include_labels: Some(false).into(),
                limit: Some(1),
                offset: None,
                seek_permission: Some(false).into(),
            },
            None,
        )
        .await
        .map_err(|e| WalletError::Internal(format!("UMP token lookup failed: {e}")))?;

    if result.outputs.is_empty() {
        return Ok(None);
    }

    let output = &result.outputs[0];
    let script_bytes = match &output.locking_script {
        Some(s) => s.clone(),
        None => {
            warn!("UMP token output has no locking script, treating as not found");
            return Ok(None);
        }
    };

    // Parse the locking script as PushDrop to extract fields.
    let locking_script = LockingScript::from_binary(&script_bytes);
    match decode_push_drop(&locking_script) {
        Ok(pd) => match decode_ump_token_fields(&pd.fields) {
            Ok(mut token) => {
                token.current_outpoint = Some(output.outpoint.clone());
                Ok(Some(token))
            }
            Err(e) => {
                warn!("Failed to decode UMP token fields: {}", e);
                Ok(None)
            }
        },
        Err(e) => {
            // PushDrop parsing failed -- may be length-prefixed format from
            // older code path. Try length-prefixed fallback.
            warn!(
                "PushDrop decode failed ({}), trying length-prefixed fallback",
                e
            );
            match parse_length_prefixed_fields(&script_bytes) {
                Some(fields) if fields.len() >= 11 => match decode_ump_token_fields(&fields) {
                    Ok(mut token) => {
                        token.current_outpoint = Some(output.outpoint.clone());
                        Ok(Some(token))
                    }
                    Err(e2) => {
                        warn!("Length-prefixed decode also failed: {}", e2);
                        Ok(None)
                    }
                },
                _ => {
                    warn!("UMP token script could not be parsed in any format");
                    Ok(None)
                }
            }
        }
    }
}

/// Parse length-prefixed fields from a byte buffer (fallback for non-PushDrop scripts).
///
/// Format: repeated [u32 LE length][field bytes].
fn parse_length_prefixed_fields(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos + 4 <= data.len() {
        let len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        if pos + len > data.len() {
            return None;
        }
        fields.push(data[pos..pos + len].to_vec());
        pos += len;
    }
    if fields.is_empty() {
        None
    } else {
        Some(fields)
    }
}

/// Create a new UMP token as a PushDrop UTXO on-chain.
///
/// Encodes UMP token fields into a PushDrop locking script, then calls
/// `wallet.create_action()` to publish the output in the admin basket with
/// the presentation hash as a tag for lookup.
///
/// # Arguments
/// * `wallet` - Wallet to create the action with
/// * `token` - The UMP token data to encode in the PushDrop output
/// * `admin_originator` - Admin domain for the protocol key derivation
pub async fn create_ump_token(
    wallet: &(dyn WalletInterface + Send + Sync),
    token: &UMPToken,
    admin_originator: &str,
) -> Result<(), WalletError> {
    let fields = encode_ump_token_fields(token);

    // Lock to a key the WALLET derives under the UMP protocol, keyed on the
    // presentation hash — so the holder of the wallet can actually spend (i.e.
    // update or revoke) the token later.
    //
    // This previously called `PrivateKey::from_random()` and threw the key away.
    // The comment claimed it was "a deterministic key derived from the presentation
    // hash so the token can be spent later"; it was neither derived nor retained,
    // so every UMP token ever minted was locked to a random key nobody holds and is
    // PERMANENTLY UNSPENDABLE. The old `PushDrop::lock()` demanded a raw PrivateKey
    // that `WalletInterface` cannot expose, and this was the workaround.
    // bsv-sdk 0.3 makes PushDrop wallet-driven, so the real derivation is now
    // expressible.
    let key_id: String = token
        .presentation_hash
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let locking_script = PushDrop::new(wallet, Some(admin_originator.to_string()))
        .lock(
            fields,
            Protocol {
                security_level: UMP_PROTOCOL_SECURITY_LEVEL,
                protocol: UMP_PROTOCOL_NAME.to_string(),
            },
            &key_id,
            Counterparty {
                counterparty_type: CounterpartyType::Self_,
                public_key: None,
            },
            false,
            // include_signature = FALSE, deliberately, unlike most PushDrop callers.
            //
            // The UMP field layout is positional and open-ended: `decode_ump_token_fields`
            // reads `fields[11]` as the OPTIONAL `profiles_encrypted`. A token without
            // profiles has 11 fields, so an appended signature would land at index 11
            // and be silently misread as profiles data. Every UMP token already on
            // chain was minted without one (the old lock never appended a signature),
            // so 11/12 is the established wire layout.
            //
            // Spending is still gated: the script's OP_CHECKSIG binds it to the
            // wallet-derived key above. The signature field is a data attestation, not
            // the spend guard — dropping it costs nothing here and keeps the wire
            // stable. If UMP ever needs one, the field list must become
            // self-describing first.
            false,
            LockPosition::Before,
        )
        .await
        .map_err(|e| {
            WalletError::Internal(format!("Failed to create UMP token locking script: {e}"))
        })?;
    let script_bytes = locking_script.to_binary();

    // Build a tag from the presentation hash for lookup.
    let presentation_hash_hex: String = token
        .presentation_hash
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    wallet
        .create_action(
            CreateActionArgs {
                description: "Create UMP token".to_string(),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(script_bytes),
                    satoshis: 1,
                    output_description: "UMP token PushDrop output".to_string(),
                    basket: Some(UMP_BASKET.to_string()),
                    custom_instructions: None,
                    tags: vec![format!("presentationHash {}", presentation_hash_hex)],
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
        .await
        .map_err(|e| WalletError::Internal(format!("Failed to create UMP token action: {e}")))?;

    Ok(())
}

/// Decode PushDrop script chunks into a UMPToken struct.
///
/// Expects at least 11 fields in the standard UMP token order.
/// Field 11 (profiles_encrypted) is optional.
///
/// # Arguments
/// * `fields` - Ordered byte array fields extracted from a PushDrop script
pub fn decode_ump_token_fields(fields: &[Vec<u8>]) -> Result<UMPToken, WalletError> {
    if fields.len() < 11 {
        return Err(WalletError::Internal(format!(
            "UMP token requires at least 11 fields, got {}",
            fields.len()
        )));
    }

    // v3 metadata detection mirrors TS `parseLookupAnswer`: the single-byte
    // version field (value 3) sits at index 12 when profilesEncrypted is present,
    // otherwise at index 11.
    let has_v3_with_profiles =
        fields.len() >= 15 && fields[12].len() == 1 && fields[12][0] == UMP_VERSION_V3;
    let has_v3_without_profiles =
        fields.len() >= 14 && fields[11].len() == 1 && fields[11][0] == UMP_VERSION_V3;
    let kdf_version_index = if has_v3_with_profiles {
        Some(12)
    } else if has_v3_without_profiles {
        Some(11)
    } else {
        None
    };

    let mut token = UMPToken {
        password_salt: fields[0].clone(),
        password_presentation_primary: fields[1].clone(),
        password_recovery_primary: fields[2].clone(),
        presentation_recovery_primary: fields[3].clone(),
        password_primary_privileged: fields[4].clone(),
        presentation_recovery_privileged: fields[5].clone(),
        presentation_hash: fields[6].clone(),
        recovery_hash: fields[7].clone(),
        presentation_key_encrypted: fields[8].clone(),
        password_key_encrypted: fields[9].clone(),
        recovery_key_encrypted: fields[10].clone(),
        profiles_encrypted: None,
        ump_version: None,
        password_kdf: None,
        current_outpoint: None,
    };

    // profilesEncrypted (index 11) is present unless index 11 is actually the v3
    // version byte (the without-profiles layout).
    let has_profiles_field =
        has_v3_with_profiles || (!has_v3_without_profiles && fields.get(11).is_some());
    if has_profiles_field {
        if let Some(profiles) = fields.get(11) {
            if !profiles.is_empty() {
                token.profiles_encrypted = Some(profiles.clone());
            }
        }
    }

    if let Some(vi) = kdf_version_index {
        token.ump_version = Some(fields[vi][0]);
        if let (Some(algorithm_field), Some(params_field)) =
            (fields.get(vi + 1), fields.get(vi + 2))
        {
            let algorithm = String::from_utf8_lossy(algorithm_field).to_string();
            if let Ok(params) = serde_json::from_slice::<serde_json::Value>(params_field) {
                token.password_kdf = Some(PasswordKdf {
                    algorithm,
                    iterations: params
                        .get("iterations")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    memory_kib: params
                        .get("memoryKiB")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    parallelism: params
                        .get("parallelism")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    hash_length: params
                        .get("hashLength")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                });
            }
        }
    }

    Ok(token)
}

/// Build a new-user UMP v3 token bound to a presentation key + password.
///
/// Ports the TS `handleNewUserPassword` key schedule: a random root primary key is
/// encrypted under `SymmetricKey(XOR(presentationKey, passwordKey))` (plus the
/// recovery/privileged variants), so the root can be recovered only with the
/// password factor — never from the presentation key alone. `password_key` is
/// derived from `kdf` (Argon2id for v3, PBKDF2 for legacy).
///
/// Note: the three `*_key_encrypted` fields are wrapped here with the root
/// privileged key via `SymmetricKey`. TS wraps them through the BRC-2
/// `PrivilegedKeyManager` ("admin key wrapping"), which is not yet ported; those
/// fields feed only the recovery/privileged flows (out of scope for the WAB/UMP
/// root-derivation parity), so a symmetric wrap keeps the token well-formed.
pub fn build_new_user_ump_token(
    presentation_key: &[u8],
    password: &[u8],
    kdf: &PasswordKdf,
) -> Result<UMPToken, WalletError> {
    if presentation_key.len() != 32 {
        return Err(WalletError::Internal(format!(
            "Presentation key must be 32 bytes for UMP token derivation, got {}",
            presentation_key.len()
        )));
    }

    let recovery_key = random_bytes(32);
    let password_salt = random_bytes(32);
    let password_key = derive_password_key(&password_salt, password, Some(kdf))?;
    if password_key.len() != 32 {
        return Err(WalletError::Internal(format!(
            "Derived password key must be 32 bytes for XOR, got {}",
            password_key.len()
        )));
    }
    let root_primary_key = random_bytes(32);
    let root_privileged_key = random_bytes(32);

    let presentation_password = sym_key(&xor_keys(presentation_key, &password_key))?;
    let presentation_recovery = sym_key(&xor_keys(presentation_key, &recovery_key))?;
    let recovery_password = sym_key(&xor_keys(&recovery_key, &password_key))?;
    let primary_password = sym_key(&xor_keys(&root_primary_key, &password_key))?;
    let privileged_wrap = sym_key(&root_privileged_key)?;

    let enc = |sym: &SymmetricKey, pt: &[u8]| -> Result<Vec<u8>, WalletError> {
        sym.encrypt(pt)
            .map_err(|e| WalletError::Internal(format!("UMP token field encryption failed: {e}")))
    };

    Ok(UMPToken {
        password_salt,
        password_presentation_primary: enc(&presentation_password, &root_primary_key)?,
        password_recovery_primary: enc(&recovery_password, &root_primary_key)?,
        presentation_recovery_primary: enc(&presentation_recovery, &root_primary_key)?,
        password_primary_privileged: enc(&primary_password, &root_privileged_key)?,
        presentation_recovery_privileged: enc(&presentation_recovery, &root_privileged_key)?,
        presentation_hash: sha256(presentation_key).to_vec(),
        recovery_hash: sha256(&recovery_key).to_vec(),
        presentation_key_encrypted: enc(&privileged_wrap, presentation_key)?,
        password_key_encrypted: enc(&privileged_wrap, &password_key)?,
        recovery_key_encrypted: enc(&privileged_wrap, &recovery_key)?,
        profiles_encrypted: None,
        ump_version: Some(UMP_VERSION_V3),
        password_kdf: Some(kdf.clone()),
        current_outpoint: None,
    })
}

/// Construct a `SymmetricKey` from raw bytes, mapping errors into `WalletError`.
fn sym_key(bytes: &[u8]) -> Result<SymmetricKey, WalletError> {
    SymmetricKey::from_bytes(bytes)
        .map_err(|e| WalletError::Internal(format!("Failed to build symmetric key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ump_token_serialization() {
        let token = UMPToken {
            password_salt: vec![1, 2, 3],
            password_presentation_primary: vec![4, 5, 6],
            password_recovery_primary: vec![7, 8, 9],
            presentation_recovery_primary: vec![10, 11, 12],
            password_primary_privileged: vec![13, 14, 15],
            presentation_recovery_privileged: vec![16, 17, 18],
            presentation_hash: vec![19, 20, 21],
            recovery_hash: vec![22, 23, 24],
            presentation_key_encrypted: vec![25, 26, 27],
            password_key_encrypted: vec![28, 29, 30],
            recovery_key_encrypted: vec![31, 32, 33],
            profiles_encrypted: None,
            ump_version: None,
            password_kdf: None,
            current_outpoint: Some("abc123.0".to_string()),
        };

        let json = serde_json::to_string(&token).expect("serialize UMPToken");
        let back: UMPToken = serde_json::from_str(&json).expect("deserialize UMPToken");
        assert_eq!(back.password_salt, vec![1, 2, 3]);
        assert_eq!(back.current_outpoint.as_deref(), Some("abc123.0"));
        assert!(back.profiles_encrypted.is_none());
    }

    #[test]
    fn test_decode_ump_token_fields() {
        let fields: Vec<Vec<u8>> = (0..11).map(|i| vec![i as u8; 4]).collect();
        let token = decode_ump_token_fields(&fields).expect("decode 11 fields");
        assert_eq!(token.password_salt, vec![0u8; 4]);
        assert_eq!(token.recovery_key_encrypted, vec![10u8; 4]);
        assert!(token.profiles_encrypted.is_none());
    }

    #[test]
    fn test_decode_ump_token_fields_with_profiles() {
        let mut fields: Vec<Vec<u8>> = (0..12).map(|i| vec![i as u8; 4]).collect();
        fields[11] = vec![0xFF, 0xFE];
        let token = decode_ump_token_fields(&fields).expect("decode 12 fields");
        assert_eq!(token.profiles_encrypted, Some(vec![0xFF, 0xFE]));
    }

    #[test]
    fn test_decode_ump_token_fields_too_few() {
        let fields: Vec<Vec<u8>> = (0..10).map(|i| vec![i as u8; 4]).collect();
        let result = decode_ump_token_fields(&fields);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("at least 11 fields"), "Error: {msg}");
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let token = UMPToken {
            password_salt: vec![1, 2, 3, 4],
            password_presentation_primary: vec![5, 6, 7, 8],
            password_recovery_primary: vec![9, 10, 11, 12],
            presentation_recovery_primary: vec![13, 14, 15, 16],
            password_primary_privileged: vec![17, 18, 19, 20],
            presentation_recovery_privileged: vec![21, 22, 23, 24],
            presentation_hash: vec![25, 26, 27, 28],
            recovery_hash: vec![29, 30, 31, 32],
            presentation_key_encrypted: vec![33, 34, 35, 36],
            password_key_encrypted: vec![37, 38, 39, 40],
            recovery_key_encrypted: vec![41, 42, 43, 44],
            profiles_encrypted: Some(vec![45, 46, 47, 48]),
            ump_version: None,
            password_kdf: None,
            current_outpoint: None,
        };

        let fields = encode_ump_token_fields(&token);
        assert_eq!(fields.len(), 12);
        let decoded = decode_ump_token_fields(&fields).unwrap();
        assert_eq!(decoded.password_salt, token.password_salt);
        assert_eq!(decoded.profiles_encrypted, Some(vec![45, 46, 47, 48]));
    }

    #[test]
    fn test_encode_without_profiles() {
        let token = UMPToken {
            password_salt: vec![1],
            password_presentation_primary: vec![2],
            password_recovery_primary: vec![3],
            presentation_recovery_primary: vec![4],
            password_primary_privileged: vec![5],
            presentation_recovery_privileged: vec![6],
            presentation_hash: vec![7],
            recovery_hash: vec![8],
            presentation_key_encrypted: vec![9],
            password_key_encrypted: vec![10],
            recovery_key_encrypted: vec![11],
            profiles_encrypted: None,
            ump_version: None,
            password_kdf: None,
            current_outpoint: None,
        };

        let fields = encode_ump_token_fields(&token);
        assert_eq!(fields.len(), 11);
    }

    #[tokio::test]
    async fn test_pushdrop_encode_decode_roundtrip() {
        // Verify that encoding as PushDrop and decoding recovers fields.
        let token = UMPToken {
            password_salt: vec![0xAA; 16],
            password_presentation_primary: vec![0xBB; 32],
            password_recovery_primary: vec![0xCC; 32],
            presentation_recovery_primary: vec![0xDD; 32],
            password_primary_privileged: vec![0xEE; 32],
            presentation_recovery_privileged: vec![0x11; 32],
            presentation_hash: vec![0x22; 32],
            recovery_hash: vec![0x33; 32],
            presentation_key_encrypted: vec![0x44; 32],
            password_key_encrypted: vec![0x55; 32],
            recovery_key_encrypted: vec![0x66; 32],
            profiles_encrypted: None,
            ump_version: None,
            password_kdf: None,
            current_outpoint: None,
        };

        let fields = encode_ump_token_fields(&token);

        // Lock the way production does: through a wallet, and WITHOUT the appended
        // signature field (see `create_ump_token` — an extra field would land at
        // index 11 and be misread as `profiles_encrypted`).
        let w = bsv::wallet::proto_wallet::ProtoWallet::new(
            bsv::primitives::private_key::PrivateKey::from_bytes(&[0x44u8; 32]).unwrap(),
        );
        let script = PushDrop::new(&w, None)
            .lock(
                fields.clone(),
                Protocol {
                    security_level: UMP_PROTOCOL_SECURITY_LEVEL,
                    protocol: UMP_PROTOCOL_NAME.to_string(),
                },
                "test-key-id",
                Counterparty {
                    counterparty_type: CounterpartyType::Self_,
                    public_key: None,
                },
                false,
                false,
                LockPosition::Before,
            )
            .await
            .expect("PushDrop lock should succeed");

        let decoded_pd = decode_push_drop(&script).expect("PushDrop decode should succeed");
        assert_eq!(
            decoded_pd.fields.len(),
            fields.len(),
            "no signature field: an 11-field token must decode as 11, or `profiles_encrypted` \
             would pick up the signature"
        );
        for (i, (original, decoded)) in fields.iter().zip(decoded_pd.fields.iter()).enumerate() {
            assert_eq!(original, decoded, "field {i} mismatch");
        }

        let decoded_token =
            decode_ump_token_fields(&decoded_pd.fields).expect("UMP token decode should succeed");
        assert_eq!(decoded_token.password_salt, token.password_salt);
        assert_eq!(decoded_token.presentation_hash, token.presentation_hash);
    }

    #[test]
    fn test_parse_length_prefixed_fields() {
        // Build length-prefixed data manually.
        let field1 = vec![0x01, 0x02, 0x03];
        let field2 = vec![0x04, 0x05];
        let mut data = Vec::new();
        data.extend_from_slice(&(field1.len() as u32).to_le_bytes());
        data.extend_from_slice(&field1);
        data.extend_from_slice(&(field2.len() as u32).to_le_bytes());
        data.extend_from_slice(&field2);

        let fields = parse_length_prefixed_fields(&data).expect("should parse");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], field1);
        assert_eq!(fields[1], field2);
    }

    #[test]
    fn test_parse_length_prefixed_fields_truncated() {
        let data = vec![0x05, 0x00, 0x00, 0x00, 0x01, 0x02]; // claims 5 bytes, only has 2
        let result = parse_length_prefixed_fields(&data);
        assert!(result.is_none());
    }

    #[test]
    fn test_constants() {
        assert_eq!(UMP_BASKET, "admin user management token");
        assert_eq!(UMP_PROTOCOL_SECURITY_LEVEL, 2);
        assert_eq!(UMP_PROTOCOL_NAME, "admin user management token");
    }

    #[test]
    fn test_v3_ump_token_argon2id_roundtrip_ts_params() {
        use super::super::cwi_logic::derive_root_primary_key;

        // TS-matching Argon2id defaults (7 iters, 128 MiB, parallelism 1, 32-byte key).
        let kdf = PasswordKdf::argon2id_default();
        assert_eq!(kdf.algorithm, KDF_ALGORITHM_ARGON2ID);
        assert_eq!(kdf.iterations, ARGON2ID_DEFAULT_ITERATIONS);
        assert_eq!(kdf.memory_kib, Some(ARGON2ID_DEFAULT_MEMORY_KIB));
        assert_eq!(kdf.parallelism, Some(ARGON2ID_DEFAULT_PARALLELISM));
        assert_eq!(kdf.hash_length, Some(ARGON2ID_DEFAULT_HASH_LENGTH));

        let presentation_key = vec![0x5Au8; 32];
        let password = b"correct horse battery staple";

        // Mint a v3 token and confirm it carries Argon2id metadata.
        let token = build_new_user_ump_token(&presentation_key, password, &kdf)
            .expect("build_new_user_ump_token should succeed");
        assert_eq!(token.ump_version, Some(UMP_VERSION_V3));
        assert_eq!(
            token.password_kdf.as_ref().unwrap().algorithm,
            KDF_ALGORITHM_ARGON2ID
        );

        // v3 KDF metadata round-trips through the on-chain field encoding.
        let fields = encode_ump_token_fields(&token);
        // 11 core fields + version + algorithm + params (no profiles) = 14.
        assert_eq!(
            fields.len(),
            14,
            "v3 token without profiles must encode 14 fields"
        );
        let decoded = decode_ump_token_fields(&fields).expect("decode v3 token");
        assert_eq!(decoded.ump_version, Some(UMP_VERSION_V3));
        let decoded_kdf = decoded
            .password_kdf
            .expect("decoded token must carry KDF metadata");
        assert_eq!(
            decoded_kdf, kdf,
            "Argon2id KDF metadata must round-trip exactly"
        );

        // The kdfParams JSON field is byte-identical to TS `JSON.stringify`.
        assert_eq!(
            String::from_utf8(fields[13].clone()).unwrap(),
            "{\"iterations\":7,\"memoryKiB\":131072,\"parallelism\":1,\"hashLength\":32}"
        );

        // The root primary key recovers only via presentation key + password
        // through the Argon2id KDF path.
        let root = derive_root_primary_key(&token, &presentation_key, password)
            .expect("root must recover with presentation key + password");
        assert_eq!(root.len(), 32);
    }
}
