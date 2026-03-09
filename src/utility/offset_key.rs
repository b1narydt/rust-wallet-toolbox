//! Commission key offset derivation.
//!
//! Ported from wallet-toolbox/src/storage/methods/offsetKey.ts.
//! Derives offset public keys for commission outputs using EC point math.

use bsv::primitives::big_number::{BigNumber, Endian};
use bsv::primitives::curve::Curve;
use bsv::primitives::hash::sha256;
use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::public_key::PublicKey;

use crate::error::{WalletError, WalletResult};

/// Compute the hashed secret from a public key and a key offset.
///
/// Algorithm:
/// 1. Compute shared secret = pub * key_offset (EC point multiplication)
/// 2. Compress the shared secret point
/// 3. Hash with SHA-256 to get a scalar
fn key_offset_to_hashed_secret(
    pub_key: &PublicKey,
    key_offset: &PrivateKey,
) -> WalletResult<BigNumber> {
    // EC point multiplication: pub_key * key_offset scalar
    let shared_secret_point = pub_key.point().mul(key_offset.bn());

    // Compress the shared secret to DER bytes
    let compressed = shared_secret_to_compressed(&shared_secret_point);

    // SHA-256 hash of compressed shared secret
    let hash = sha256(&compressed);

    Ok(BigNumber::from_bytes(&hash, Endian::Big))
}

/// Compress a Point to its DER-encoded compressed form (33 bytes).
fn shared_secret_to_compressed(point: &bsv::primitives::point::Point) -> Vec<u8> {
    point.to_der(true)
}

/// Derive an offset public key from a commission public key and a key offset.
///
/// Algorithm:
/// 1. Compute hashed secret from pub_key and key_offset
/// 2. Multiply generator point by hashed secret to get offset point
/// 3. Add offset point to the original public key
pub fn offset_pub_key(
    commission_pub_key: &PublicKey,
    key_offset: &PrivateKey,
) -> WalletResult<PublicKey> {
    let hashed_secret = key_offset_to_hashed_secret(commission_pub_key, key_offset)?;

    // Multiply generator by hashed secret
    let curve = Curve::secp256k1();
    let offset_point = curve.generator().mul(&hashed_secret);

    // Add to commission public key
    let result_point = commission_pub_key.point().add(&offset_point);

    Ok(PublicKey::from_point(result_point))
}

/// Generate a random private key for use as a commission key offset.
pub fn generate_key_offset() -> WalletResult<PrivateKey> {
    PrivateKey::from_random()
        .map_err(|e| WalletError::Internal(format!("Failed to generate random key offset: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offset_pub_key_produces_valid_pubkey() {
        let commission_priv = PrivateKey::from_hex("cc").unwrap();
        let commission_pub = commission_priv.to_public_key();
        let key_offset = PrivateKey::from_hex("dd").unwrap();

        let result = offset_pub_key(&commission_pub, &key_offset).unwrap();

        // Result should be a valid public key (can be serialized)
        let der = result.to_der();
        assert_eq!(der.len(), 33, "compressed public key should be 33 bytes");
        assert!(
            der[0] == 0x02 || der[0] == 0x03,
            "should start with 02 or 03"
        );
    }

    #[test]
    fn test_offset_pub_key_is_deterministic() {
        let commission_priv = PrivateKey::from_hex("cc").unwrap();
        let commission_pub = commission_priv.to_public_key();
        let key_offset = PrivateKey::from_hex("dd").unwrap();

        let result1 = offset_pub_key(&commission_pub, &key_offset).unwrap();
        let result2 = offset_pub_key(&commission_pub, &key_offset).unwrap();

        assert_eq!(
            result1.to_der_hex(),
            result2.to_der_hex(),
            "offset_pub_key should be deterministic"
        );
    }

    #[test]
    fn test_offset_pub_key_differs_from_original() {
        let commission_priv = PrivateKey::from_hex("cc").unwrap();
        let commission_pub = commission_priv.to_public_key();
        let key_offset = PrivateKey::from_hex("dd").unwrap();

        let result = offset_pub_key(&commission_pub, &key_offset).unwrap();
        assert_ne!(
            result.to_der_hex(),
            commission_pub.to_der_hex(),
            "offset key should differ from original"
        );
    }

    #[test]
    fn test_different_offsets_produce_different_keys() {
        let commission_priv = PrivateKey::from_hex("cc").unwrap();
        let commission_pub = commission_priv.to_public_key();
        let offset1 = PrivateKey::from_hex("dd").unwrap();
        let offset2 = PrivateKey::from_hex("ee").unwrap();

        let result1 = offset_pub_key(&commission_pub, &offset1).unwrap();
        let result2 = offset_pub_key(&commission_pub, &offset2).unwrap();

        assert_ne!(
            result1.to_der_hex(),
            result2.to_der_hex(),
            "different offsets should produce different keys"
        );
    }

    #[test]
    fn test_generate_key_offset_produces_valid_key() {
        let offset = generate_key_offset().unwrap();
        let pub_key = offset.to_public_key();
        let der = pub_key.to_der();
        assert_eq!(der.len(), 33);
    }
}
