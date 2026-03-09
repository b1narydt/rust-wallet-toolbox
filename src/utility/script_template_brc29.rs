//! BRC-29 Script Template for authenticated P2PKH payments.
//!
//! Implements the Simple Authenticated BSV P2PKH Payment Protocol (BRC-29)
//! using Type-42 key derivation. Ported from wallet-toolbox/src/utility/ScriptTemplateBRC29.ts.

use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::public_key::PublicKey;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::key_deriver::KeyDeriver;
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

use crate::error::WalletResult;

/// P2PKH unlock script estimated length: 1 (push) + 73 (DER sig + sighash) + 1 (push) + 33 (pubkey) = 108
pub const BRC29_UNLOCK_LENGTH: usize = 108;

/// BRC-29 protocol ID as a tuple: (security_level, protocol_name)
pub const BRC29_PROTOCOL_ID: (u8, &str) = (2, "3241645161d8");

/// Returns the BRC-29 Protocol for use with KeyDeriver.
pub fn brc29_protocol() -> Protocol {
    Protocol {
        security_level: BRC29_PROTOCOL_ID.0,
        protocol: BRC29_PROTOCOL_ID.1.to_string(),
    }
}

/// BRC-29 authenticated P2PKH script template.
///
/// Uses Type-42 key derivation to produce deterministic locking and unlocking
/// scripts for authenticated payments between two parties.
#[derive(Debug, Clone)]
pub struct ScriptTemplateBRC29 {
    /// The derivation prefix (random per transaction).
    pub derivation_prefix: String,
    /// The derivation suffix (random per output).
    pub derivation_suffix: String,
}

impl ScriptTemplateBRC29 {
    /// Create a new BRC-29 script template.
    pub fn new(derivation_prefix: String, derivation_suffix: String) -> Self {
        Self {
            derivation_prefix,
            derivation_suffix,
        }
    }

    /// Returns the key ID for derivation: "{prefix} {suffix}"
    pub fn key_id(&self) -> String {
        format!("{} {}", self.derivation_prefix, self.derivation_suffix)
    }

    /// Produce a P2PKH locking script using BRC-29 key derivation.
    ///
    /// The locker derives a public key from the unlocker's public key using
    /// Type-42 derivation with the BRC-29 protocol.
    pub fn lock(
        &self,
        locker_priv_key: &PrivateKey,
        unlocker_pub_key: &PublicKey,
    ) -> WalletResult<Vec<u8>> {
        let key_deriver = KeyDeriver::new(locker_priv_key.clone());
        let counterparty = Counterparty {
            counterparty_type: CounterpartyType::Other,
            public_key: Some(unlocker_pub_key.clone()),
        };
        let derived_pub = key_deriver
            .derive_public_key(&brc29_protocol(), &self.key_id(), &counterparty, false)
            .map_err(|e| {
                crate::error::WalletError::Internal(format!("BRC-29 lock derivation failed: {}", e))
            })?;

        let p2pkh = P2PKH::from_public_key_hash(Self::pubkey_to_hash(&derived_pub));
        let locking_script = p2pkh.lock().map_err(|e| {
            crate::error::WalletError::Internal(format!("P2PKH lock failed: {}", e))
        })?;
        Ok(locking_script.to_binary())
    }

    /// Produce a P2PKH unlock template using BRC-29 key derivation.
    ///
    /// The unlocker derives a private key from the locker's public key using
    /// Type-42 derivation with the BRC-29 protocol, then creates a P2PKH
    /// signer from the derived private key.
    pub fn unlock(
        &self,
        unlocker_priv_key: &PrivateKey,
        locker_pub_key: &PublicKey,
    ) -> WalletResult<P2PKH> {
        let key_deriver = KeyDeriver::new(unlocker_priv_key.clone());
        let counterparty = Counterparty {
            counterparty_type: CounterpartyType::Other,
            public_key: Some(locker_pub_key.clone()),
        };
        let derived_priv = key_deriver
            .derive_private_key(&brc29_protocol(), &self.key_id(), &counterparty)
            .map_err(|e| {
                crate::error::WalletError::Internal(format!(
                    "BRC-29 unlock derivation failed: {}",
                    e
                ))
            })?;

        Ok(P2PKH::from_private_key(derived_priv))
    }

    /// Convert a PublicKey to a 20-byte hash suitable for P2PKH.
    fn pubkey_to_hash(pubkey: &PublicKey) -> [u8; 20] {
        let hash_vec = pubkey.to_hash();
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&hash_vec);
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keys() -> (PrivateKey, PublicKey, PrivateKey, PublicKey) {
        let locker_priv = PrivateKey::from_hex("aa").unwrap();
        let locker_pub = locker_priv.to_public_key();
        let unlocker_priv = PrivateKey::from_hex("bb").unwrap();
        let unlocker_pub = unlocker_priv.to_public_key();
        (locker_priv, locker_pub, unlocker_priv, unlocker_pub)
    }

    #[test]
    fn test_key_id_format() {
        let tmpl = ScriptTemplateBRC29::new("prefix123".to_string(), "suffix456".to_string());
        assert_eq!(tmpl.key_id(), "prefix123 suffix456");
    }

    #[test]
    fn test_brc29_protocol_values() {
        let proto = brc29_protocol();
        assert_eq!(proto.security_level, 2);
        assert_eq!(proto.protocol, "3241645161d8");
    }

    #[test]
    fn test_brc29_unlock_length_constant() {
        assert_eq!(BRC29_UNLOCK_LENGTH, 108);
    }

    #[test]
    fn test_lock_produces_valid_p2pkh_script() {
        let (locker_priv, _locker_pub, _unlocker_priv, unlocker_pub) = test_keys();
        let tmpl = ScriptTemplateBRC29::new("test_prefix".to_string(), "test_suffix".to_string());
        let script = tmpl.lock(&locker_priv, &unlocker_pub).unwrap();

        // P2PKH locking script is always 25 bytes
        assert_eq!(script.len(), 25, "P2PKH locking script should be 25 bytes");
        // OP_DUP OP_HASH160 PUSH20 <hash> OP_EQUALVERIFY OP_CHECKSIG
        assert_eq!(script[0], 0x76, "should start with OP_DUP");
        assert_eq!(script[1], 0xa9, "second byte should be OP_HASH160");
        assert_eq!(script[23], 0x88, "should have OP_EQUALVERIFY");
        assert_eq!(script[24], 0xac, "should end with OP_CHECKSIG");
    }

    #[test]
    fn test_lock_is_deterministic() {
        let (locker_priv, _locker_pub, _unlocker_priv, unlocker_pub) = test_keys();
        let tmpl = ScriptTemplateBRC29::new("det_prefix".to_string(), "det_suffix".to_string());
        let script1 = tmpl.lock(&locker_priv, &unlocker_pub).unwrap();
        let script2 = tmpl.lock(&locker_priv, &unlocker_pub).unwrap();
        assert_eq!(script1, script2, "lock should be deterministic");
    }

    #[test]
    fn test_unlock_produces_p2pkh_with_private_key() {
        let (_locker_priv, locker_pub, unlocker_priv, _unlocker_pub) = test_keys();
        let tmpl = ScriptTemplateBRC29::new("test_prefix".to_string(), "test_suffix".to_string());
        let p2pkh = tmpl.unlock(&unlocker_priv, &locker_pub).unwrap();

        // The P2PKH should have a private key set (for signing)
        assert!(
            p2pkh.private_key.is_some(),
            "unlock should produce P2PKH with private key"
        );
    }

    #[test]
    fn test_lock_unlock_key_correspondence() {
        // The locking script created by locker should match the hash of the
        // derived key that the unlocker produces via unlock.
        let (locker_priv, locker_pub, unlocker_priv, unlocker_pub) = test_keys();
        let tmpl = ScriptTemplateBRC29::new("test_prefix".to_string(), "test_suffix".to_string());

        let lock_script = tmpl.lock(&locker_priv, &unlocker_pub).unwrap();
        let p2pkh = tmpl.unlock(&unlocker_priv, &locker_pub).unwrap();

        // Extract the hash from the locking script (bytes 3..23)
        let lock_hash = &lock_script[3..23];

        // The P2PKH from unlock should have the same hash
        let unlock_hash = p2pkh.public_key_hash.unwrap();
        assert_eq!(
            lock_hash,
            &unlock_hash[..],
            "lock and unlock should derive corresponding keys"
        );
    }
}
