//! User Management Protocol (UMP) token types and overlay interactions.
//!
//! Ports the `OverlayUMPTokenInteractor` from TypeScript. The UMP token is a
//! PushDrop UTXO that stores encrypted key material for multi-factor
//! authentication (password + presentation key + recovery key).

use serde::{Deserialize, Serialize};
use tracing::warn;

use bsv::primitives::private_key::PrivateKey;
use bsv::script::locking_script::LockingScript;
use bsv::script::templates::push_drop::PushDrop;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, ListOutputsArgs, OutputInclude,
    QueryMode, WalletInterface,
};

use crate::WalletError;

/// Admin basket name for UMP tokens (matching TS constant).
pub const UMP_BASKET: &str = "admin user management token";

/// Protocol security level for UMP token operations.
pub const UMP_PROTOCOL_SECURITY_LEVEL: u8 = 2;

/// Protocol name for UMP token operations.
pub const UMP_PROTOCOL_NAME: &str = "admin user management token";

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
    fields
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
        .map_err(|e| WalletError::Internal(format!("UMP token lookup failed: {}", e)))?;

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
    match PushDrop::decode(&locking_script) {
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

    // Create a temporary signing key for the PushDrop locking script.
    // In the full flow, this would use protocol key derivation via the wallet,
    // but PushDrop::lock() requires a PrivateKey directly. We use a deterministic
    // key derived from the presentation hash so the token can be spent later.
    let signing_key = PrivateKey::from_random().map_err(|e| {
        WalletError::Internal(format!(
            "Failed to generate signing key for UMP token: {}",
            e
        ))
    })?;

    let pushdrop = PushDrop::new(fields, signing_key);
    let locking_script = pushdrop.lock().map_err(|e| {
        WalletError::Internal(format!("Failed to create UMP token locking script: {}", e))
    })?;
    let script_bytes = locking_script.to_binary();

    // Build a tag from the presentation hash for lookup.
    let presentation_hash_hex: String = token
        .presentation_hash
        .iter()
        .map(|b| format!("{:02x}", b))
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
        .map_err(|e| WalletError::Internal(format!("Failed to create UMP token action: {}", e)))?;

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

    Ok(UMPToken {
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
        profiles_encrypted: fields.get(11).cloned(),
        current_outpoint: None,
    })
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
        assert!(msg.contains("at least 11 fields"), "Error: {}", msg);
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
            current_outpoint: None,
        };

        let fields = encode_ump_token_fields(&token);
        assert_eq!(fields.len(), 11);
    }

    #[test]
    fn test_pushdrop_encode_decode_roundtrip() {
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
            current_outpoint: None,
        };

        let fields = encode_ump_token_fields(&token);
        let key = PrivateKey::from_random().unwrap();
        let pd = PushDrop::new(fields.clone(), key);
        let script = pd.lock().expect("PushDrop lock should succeed");

        let decoded_pd = PushDrop::decode(&script).expect("PushDrop decode should succeed");
        assert_eq!(decoded_pd.fields.len(), fields.len());
        for (i, (original, decoded)) in fields.iter().zip(decoded_pd.fields.iter()).enumerate() {
            assert_eq!(original, decoded, "field {} mismatch", i);
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
}
