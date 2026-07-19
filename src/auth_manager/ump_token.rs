//! User Management Protocol (UMP) token types and overlay interactions.
//!
//! Ports the `OverlayUMPTokenInteractor` from TypeScript. The UMP token is a
//! PushDrop UTXO that stores encrypted key material for multi-factor
//! authentication (password + presentation key + recovery key).

use serde::{Deserialize, Serialize};
use tracing::warn;

use bsv::primitives::hash::sha256;
use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::random::random_bytes;
use bsv::primitives::symmetric_key::SymmetricKey;
use bsv::script::locking_script::LockingScript;
use bsv::script::templates::push_drop::{decode as decode_push_drop, LockPosition, PushDrop};
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, ListOutputsArgs, OutputInclude,
    QueryMode, WalletInterface,
};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

use super::cwi_logic::{derive_password_key, xor_keys};
use crate::WalletError;

/// Expected byte length of each UMP auth factor (presentation key, recovery
/// key, and the derived password key) and of the root primary/privileged keys.
pub const UMP_FACTOR_SIZE: usize = 32;

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

// ============================================================================
// 2-of-3 UMP threshold scheme (XOR + AES-GCM symmetric encryption, NOT Shamir)
// ============================================================================
//
// The wallet's `rootPrimaryKey` is stored three times in the token, each
// encrypted under the XOR of a *pair* of the three auth factors
// (presentation key, password key, recovery key):
//
//   password_presentation_primary = SymEnc(rootPrimaryKey, XOR(presentation, password))
//   password_recovery_primary     = SymEnc(rootPrimaryKey, XOR(password,     recovery))
//   presentation_recovery_primary = SymEnc(rootPrimaryKey, XOR(presentation, recovery))
//
// Any 2 of the 3 factors reconstruct the XOR key and decrypt the root key.
// A parallel `rootPrivilegedKey` is protected by the two combinations that
// don't require re-deriving the root key first: `password + rootPrimaryKey`
// and `presentation + recovery`. The three raw factors are also each backed
// up encrypted directly under the privileged key so they can be recovered
// once the privileged key is known (e.g. for a password change).

/// The two factors an existing user supplies to unlock a `UMPToken`'s
/// `rootPrimaryKey`. Any of the three 2-of-3 combinations works and all
/// yield the identical root key.
pub enum AuthFactors {
    /// Presentation key (from the WAB, after an Auth Method passes) + password.
    PresentationPassword {
        /// 256-bit presentation key bytes.
        presentation_key: Vec<u8>,
        /// Raw password bytes (PBKDF2'd internally against the token's salt).
        password: Vec<u8>,
    },
    /// Password + user-held recovery key (no WAB interaction needed).
    PasswordRecovery {
        /// Raw password bytes (PBKDF2'd internally against the token's salt).
        password: Vec<u8>,
        /// 256-bit recovery key bytes.
        recovery_key: Vec<u8>,
    },
    /// Presentation key + recovery key (password forgotten / not needed).
    PresentationRecovery {
        /// 256-bit presentation key bytes.
        presentation_key: Vec<u8>,
        /// 256-bit recovery key bytes.
        recovery_key: Vec<u8>,
    },
}

/// Validate that a factor byte slice is exactly [`UMP_FACTOR_SIZE`] bytes.
fn require_factor_size(name: &str, bytes: &[u8]) -> Result<(), WalletError> {
    if bytes.len() != UMP_FACTOR_SIZE {
        return Err(WalletError::Internal(format!(
            "{} must be {} bytes, got {}",
            name,
            UMP_FACTOR_SIZE,
            bytes.len()
        )));
    }
    Ok(())
}

/// Build a brand-new `UMPToken` for a first-time user, given all three
/// factors: the presentation key just released by the WAB, a password the
/// user just chose, and a recovery key just generated for them.
///
/// Generates a fresh random `rootPrimaryKey` (the wallet's actual root key)
/// and a fresh random `rootPrivilegedKey`, then populates every encrypted
/// UMP token field so that:
/// - any 2 of {presentation, password, recovery} unlock the root primary key
/// - {password, rootPrimaryKey} or {presentation, recovery} unlock the root
///   privileged key
/// - all three raw factors are recoverable once the privileged key is known
///
/// # Returns
/// `(token, root_primary_key)` — the caller is responsible for publishing
/// the token on-chain (via [`create_ump_token`]) and for using
/// `root_primary_key` to construct the user's wallet.
///
/// # Arguments
/// * `password` - Raw password bytes chosen by the user
/// * `presentation_key` - 32-byte presentation key released by the WAB
/// * `recovery_key` - 32-byte recovery key (generate randomly for new users)
pub fn build_ump_token(
    password: &[u8],
    presentation_key: &[u8],
    recovery_key: &[u8],
) -> Result<(UMPToken, Vec<u8>), WalletError> {
    require_factor_size("presentation_key", presentation_key)?;
    require_factor_size("recovery_key", recovery_key)?;

    let root_primary_key = random_bytes(UMP_FACTOR_SIZE);
    let root_privileged_key = random_bytes(UMP_FACTOR_SIZE);
    let password_salt = random_bytes(UMP_FACTOR_SIZE);
    let password_key = derive_password_key(password, &password_salt);
    require_factor_size("derived password_key", &password_key)?;

    let xor_pw_pres = xor_keys(presentation_key, &password_key);
    let xor_pw_rec = xor_keys(&password_key, recovery_key);
    let xor_pres_rec = xor_keys(presentation_key, recovery_key);

    let password_presentation_primary = sym_encrypt(&xor_pw_pres, &root_primary_key)?;
    let password_recovery_primary = sym_encrypt(&xor_pw_rec, &root_primary_key)?;
    let presentation_recovery_primary = sym_encrypt(&xor_pres_rec, &root_primary_key)?;

    let xor_pw_primary = xor_keys(&password_key, &root_primary_key);
    let password_primary_privileged = sym_encrypt(&xor_pw_primary, &root_privileged_key)?;
    let presentation_recovery_privileged = sym_encrypt(&xor_pres_rec, &root_privileged_key)?;

    let presentation_key_encrypted = sym_encrypt(&root_privileged_key, presentation_key)?;
    let password_key_encrypted = sym_encrypt(&root_privileged_key, &password_key)?;
    let recovery_key_encrypted = sym_encrypt(&root_privileged_key, recovery_key)?;

    let token = UMPToken {
        password_salt,
        password_presentation_primary,
        password_recovery_primary,
        presentation_recovery_primary,
        password_primary_privileged,
        presentation_recovery_privileged,
        presentation_hash: sha256(presentation_key).to_vec(),
        recovery_hash: sha256(recovery_key).to_vec(),
        presentation_key_encrypted,
        password_key_encrypted,
        recovery_key_encrypted,
        profiles_encrypted: None,
        current_outpoint: None,
    };

    Ok((token, root_primary_key))
}

/// Unlock (decrypt) an existing `UMPToken`'s `rootPrimaryKey` using any 2 of
/// the 3 auth factors.
///
/// Derives the password key from the token's stored salt when a password is
/// supplied, XORs the appropriate factor pair, and uses the result as an
/// AES-256 key to decrypt the matching primary-key slot. A wrong password
/// (or wrong recovery/presentation key) produces a different XOR key, which
/// fails AEAD decryption and returns an `Err`.
///
/// # Arguments
/// * `token` - The on-chain `UMPToken` to unlock
/// * `factors` - The 2 factors supplied by the caller
pub fn unlock_root_primary_key(
    token: &UMPToken,
    factors: &AuthFactors,
) -> Result<Vec<u8>, WalletError> {
    let (xor_key, ciphertext) = match factors {
        AuthFactors::PresentationPassword {
            presentation_key,
            password,
        } => {
            require_factor_size("presentation_key", presentation_key)?;
            let password_key = derive_password_key(password, &token.password_salt);
            (
                xor_keys(presentation_key, &password_key),
                &token.password_presentation_primary,
            )
        }
        AuthFactors::PasswordRecovery {
            password,
            recovery_key,
        } => {
            require_factor_size("recovery_key", recovery_key)?;
            let password_key = derive_password_key(password, &token.password_salt);
            (
                xor_keys(&password_key, recovery_key),
                &token.password_recovery_primary,
            )
        }
        AuthFactors::PresentationRecovery {
            presentation_key,
            recovery_key,
        } => {
            require_factor_size("presentation_key", presentation_key)?;
            require_factor_size("recovery_key", recovery_key)?;
            (
                xor_keys(presentation_key, recovery_key),
                &token.presentation_recovery_primary,
            )
        }
    };

    sym_decrypt(&xor_key, ciphertext)
}

/// Encrypt `plaintext` with `key_bytes` as an AES-256 `SymmetricKey`.
fn sym_encrypt(key_bytes: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, WalletError> {
    let key = SymmetricKey::from_bytes(key_bytes)
        .map_err(|e| WalletError::Internal(format!("Invalid UMP symmetric key: {e}")))?;
    key.encrypt(plaintext)
        .map_err(|e| WalletError::Internal(format!("UMP symmetric encryption failed: {e}")))
}

/// Decrypt `ciphertext` with `key_bytes` as an AES-256 `SymmetricKey`.
fn sym_decrypt(key_bytes: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, WalletError> {
    let key = SymmetricKey::from_bytes(key_bytes)
        .map_err(|e| WalletError::Internal(format!("Invalid UMP symmetric key: {e}")))?;
    key.decrypt(ciphertext)
        .map_err(|e| WalletError::Internal(format!("UMP symmetric decryption failed: {e}")))
}

/// Compute the SHA-256 hash of a presentation key, for UMP token lookup by tag.
pub fn presentation_hash_hex(presentation_key: &[u8]) -> String {
    sha256(presentation_key)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // 2-of-3 UMP threshold scheme: build_ump_token / unlock_root_primary_key
    // ------------------------------------------------------------------

    const PASSWORD: &[u8] = b"correct horse battery staple";

    fn make_test_token() -> (UMPToken, Vec<u8>, Vec<u8>, Vec<u8>) {
        let presentation_key = random_bytes(UMP_FACTOR_SIZE);
        let recovery_key = random_bytes(UMP_FACTOR_SIZE);
        let (token, root_primary_key) = build_ump_token(PASSWORD, &presentation_key, &recovery_key)
            .expect("build_ump_token should succeed");
        (token, root_primary_key, presentation_key, recovery_key)
    }

    #[test]
    fn test_unlock_mode1_presentation_password() {
        let (token, root_key, presentation_key, _recovery_key) = make_test_token();

        let unlocked = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationPassword {
                presentation_key,
                password: PASSWORD.to_vec(),
            },
        )
        .expect("presentation+password should unlock the root key");

        assert_eq!(unlocked, root_key);
    }

    #[test]
    fn test_unlock_mode2_presentation_recovery() {
        let (token, root_key, presentation_key, recovery_key) = make_test_token();

        let unlocked = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationRecovery {
                presentation_key,
                recovery_key,
            },
        )
        .expect("presentation+recovery should unlock the root key");

        assert_eq!(unlocked, root_key);
    }

    #[test]
    fn test_unlock_mode3_recovery_password() {
        let (token, root_key, _presentation_key, recovery_key) = make_test_token();

        let unlocked = unlock_root_primary_key(
            &token,
            &AuthFactors::PasswordRecovery {
                password: PASSWORD.to_vec(),
                recovery_key,
            },
        )
        .expect("password+recovery should unlock the root key");

        assert_eq!(unlocked, root_key);
    }

    #[test]
    fn test_all_three_modes_yield_the_same_root_key() {
        let (token, root_key, presentation_key, recovery_key) = make_test_token();

        let via_pw = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationPassword {
                presentation_key: presentation_key.clone(),
                password: PASSWORD.to_vec(),
            },
        )
        .unwrap();
        let via_recovery = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationRecovery {
                presentation_key: presentation_key.clone(),
                recovery_key: recovery_key.clone(),
            },
        )
        .unwrap();
        let via_pw_recovery = unlock_root_primary_key(
            &token,
            &AuthFactors::PasswordRecovery {
                password: PASSWORD.to_vec(),
                recovery_key: recovery_key.clone(),
            },
        )
        .unwrap();

        assert_eq!(via_pw, root_key);
        assert_eq!(via_recovery, root_key);
        assert_eq!(via_pw_recovery, root_key);
        assert_eq!(via_pw, via_recovery);
        assert_eq!(via_recovery, via_pw_recovery);
    }

    #[test]
    fn test_wrong_password_fails() {
        let (token, _root_key, presentation_key, _recovery_key) = make_test_token();

        let result = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationPassword {
                presentation_key,
                password: b"totally-wrong-password".to_vec(),
            },
        );

        assert!(
            result.is_err(),
            "wrong password must not unlock the root key"
        );
    }

    #[test]
    fn test_wrong_recovery_key_fails() {
        let (token, _root_key, presentation_key, _recovery_key) = make_test_token();
        let wrong_recovery = random_bytes(UMP_FACTOR_SIZE);

        let result = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationRecovery {
                presentation_key,
                recovery_key: wrong_recovery,
            },
        );

        assert!(
            result.is_err(),
            "wrong recovery key must not unlock the root key"
        );
    }

    #[test]
    fn test_wrong_presentation_key_fails() {
        let (token, _root_key, _presentation_key, recovery_key) = make_test_token();
        let wrong_presentation = random_bytes(UMP_FACTOR_SIZE);

        let result = unlock_root_primary_key(
            &token,
            &AuthFactors::PresentationRecovery {
                presentation_key: wrong_presentation,
                recovery_key,
            },
        );

        assert!(
            result.is_err(),
            "wrong presentation key must not unlock the root key"
        );
    }

    #[test]
    fn test_build_ump_token_rejects_bad_factor_sizes() {
        let short_presentation = vec![0u8; 16];
        let recovery_key = random_bytes(UMP_FACTOR_SIZE);
        let result = build_ump_token(PASSWORD, &short_presentation, &recovery_key);
        assert!(result.is_err(), "short presentation key must be rejected");

        let presentation_key = random_bytes(UMP_FACTOR_SIZE);
        let short_recovery = vec![0u8; 8];
        let result = build_ump_token(PASSWORD, &presentation_key, &short_recovery);
        assert!(result.is_err(), "short recovery key must be rejected");
    }

    #[test]
    fn test_build_ump_token_populates_hashes_and_salt() {
        let (token, _root_key, presentation_key, recovery_key) = make_test_token();

        assert_eq!(token.presentation_hash, sha256(&presentation_key).to_vec());
        assert_eq!(token.recovery_hash, sha256(&recovery_key).to_vec());
        assert_eq!(token.password_salt.len(), UMP_FACTOR_SIZE);
        assert_eq!(presentation_hash_hex(&presentation_key).len(), 64);
    }

    #[test]
    fn test_root_privileged_key_unlockable_via_password_and_primary() {
        // password_primary_privileged = SymEnc(rootPrivilegedKey, XOR(passwordKey, rootPrimaryKey))
        let (token, root_primary_key, _presentation_key, _recovery_key) = make_test_token();
        let password_key = derive_password_key(PASSWORD, &token.password_salt);
        let xor = xor_keys(&password_key, &root_primary_key);
        let root_privileged_key =
            sym_decrypt(&xor, &token.password_primary_privileged).expect("should decrypt");
        assert_eq!(root_privileged_key.len(), UMP_FACTOR_SIZE);
    }

    #[test]
    fn test_root_privileged_key_unlockable_via_presentation_and_recovery() {
        // presentation_recovery_privileged = SymEnc(rootPrivilegedKey, XOR(presentation, recovery))
        let (token, root_primary_key, presentation_key, recovery_key) = make_test_token();
        let xor = xor_keys(&presentation_key, &recovery_key);
        let root_privileged_key =
            sym_decrypt(&xor, &token.presentation_recovery_privileged).expect("should decrypt");

        // Cross-check: the same privileged key must also decrypt via the
        // password+primary path.
        let password_key = derive_password_key(PASSWORD, &token.password_salt);
        let xor2 = xor_keys(&password_key, &root_primary_key);
        let root_privileged_key2 =
            sym_decrypt(&xor2, &token.password_primary_privileged).expect("should decrypt");
        assert_eq!(root_privileged_key, root_privileged_key2);
    }

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
}
