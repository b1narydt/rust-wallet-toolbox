//! Metadata encryption/decryption for wallet descriptions.
//!
//! Uses the admin metadata encryption protocol to preserve the confidentiality
//! of transaction descriptions and input/output descriptions from lower storage layers.
//! Only used when `config.encrypt_wallet_metadata` is true.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};

use bsv::wallet::interfaces::{DecryptArgs, EncryptArgs, WalletInterface};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

/// Protocol used for encrypting wallet metadata (descriptions).
fn metadata_encryption_protocol() -> Protocol {
    Protocol {
        security_level: 2,
        protocol: "admin metadata encryption".to_string(),
    }
}

/// Counterparty::Self_ for metadata operations.
fn self_counterparty() -> Counterparty {
    Counterparty {
        counterparty_type: CounterpartyType::Self_,
        public_key: None,
    }
}

/// Encrypt a description/metadata string using the admin metadata encryption protocol.
///
/// Returns a base64-encoded ciphertext string.
pub async fn encrypt_action_metadata(
    inner: &(dyn WalletInterface + Send + Sync),
    description: &str,
    admin_originator: &str,
) -> Result<String, bsv::wallet::error::WalletError> {
    let result = inner
        .encrypt(
            EncryptArgs {
                protocol_id: metadata_encryption_protocol(),
                key_id: "1".to_string(),
                counterparty: self_counterparty(),
                plaintext: description.as_bytes().to_vec(),
                privileged: false,
                privileged_reason: None,
                seek_permission: Some(false),
            },
            Some(admin_originator),
        )
        .await?;
    Ok(BASE64_STANDARD.encode(&result.ciphertext))
}

/// Decrypt a base64-encoded metadata string using the admin metadata encryption protocol.
///
/// If decryption fails (e.g. the string is already plaintext), returns the original
/// value instead (matching TS behavior of graceful fallback).
pub async fn decrypt_action_metadata(
    inner: &(dyn WalletInterface + Send + Sync),
    encrypted: &str,
    admin_originator: &str,
) -> Result<String, bsv::wallet::error::WalletError> {
    // Try to decode from base64 first.
    let ciphertext = match BASE64_STANDARD.decode(encrypted) {
        Ok(bytes) => bytes,
        Err(_) => {
            // Not valid base64, return as-is (already plaintext).
            return Ok(encrypted.to_string());
        }
    };

    match inner
        .decrypt(
            DecryptArgs {
                protocol_id: metadata_encryption_protocol(),
                key_id: "1".to_string(),
                counterparty: self_counterparty(),
                ciphertext,
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
            // Decryption failed, assume plaintext.
            Ok(encrypted.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_protocol() {
        let proto = metadata_encryption_protocol();
        assert_eq!(proto.security_level, 2);
        assert_eq!(proto.protocol, "admin metadata encryption");
    }

    #[test]
    fn test_self_counterparty() {
        let cpty = self_counterparty();
        assert_eq!(cpty.counterparty_type, CounterpartyType::Self_);
        assert!(cpty.public_key.is_none());
    }
}
