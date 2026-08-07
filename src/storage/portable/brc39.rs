//! BRC-39 encrypted container (TS `encryptBRC39` / `decryptBRC39`).
//!
//! File layout: 33-byte header || salt || nonce || ciphertext || 16-byte tag.
//!
//! Header bytes:
//! - 0..4  magic `WDAT`
//! - 4     format version (1)
//! - 5     protector type (1 = password)
//! - 6     inner format (38)
//! - 7     KDF type (1 = Argon2id)
//! - 8     flags (0)
//! - 9     salt length, 10 nonce length
//! - 11..15 iterations (u32 BE), 15..19 memory KiB (u32 BE)
//! - 19    parallelism, 20 hash length (32)
//! - 21..33 reserved (zero)
//!
//! The key is Argon2id over the NFC-normalized password. Encryption is
//! AES-256-GCM with the TS SDK's non-standard GHASH framing and 32-byte
//! nonce -- `bsv_sdk`'s `*_ts_compat` functions, whose byte-compatibility is
//! proven against TS-produced fixtures in this module's tests.

use argon2::{Algorithm, Argon2, Params, Version};
use bsv::primitives::aes_gcm::{aes_gcm_decrypt_ts_compat, aes_gcm_encrypt_ts_compat};
use rand::RngCore;
use unicode_normalization::UnicodeNormalization;

use crate::error::{WalletError, WalletResult};
use crate::storage::traits::provider::StorageProvider;

use super::canonical::canonicalize;
use super::export::export_brc38;
use super::import::{import_brc38, parse_brc38_json, Brc38ImportOptions, Brc38ImportResult, PortableStorage};
use super::validate::Brc38WalletData;

const BRC39_MAGIC: [u8; 4] = *b"WDAT";
const BRC39_HEADER_LENGTH: usize = 33;
const BRC39_TAG_LENGTH: usize = 16;
const BRC39_DEFAULT_ITERATIONS: u32 = 7;
const BRC39_DEFAULT_MEMORY_KIB: u32 = 131072;
const BRC39_DEFAULT_PARALLELISM: u8 = 1;
const BRC39_HASH_LENGTH: usize = 32;
const BRC39_SALT_LENGTH: usize = 32;
const BRC39_NONCE_LENGTH: usize = 32;

/// Argon2id cost overrides for BRC-39 encryption (TS `BRC39Options`).
/// Values weaker than the canonical defaults are rejected on export.
#[derive(Debug, Clone, Default)]
pub struct Brc39Options {
    /// Argon2id iteration count (default 7).
    pub iterations: Option<u32>,
    /// Argon2id memory cost in KiB (default 131072 = 128 MiB).
    pub memory_kib: Option<u32>,
    /// Argon2id parallelism (default 1).
    pub parallelism: Option<u8>,
}

/// Export a user's wallet state as an encrypted BRC-39 file.
pub async fn export_brc39(
    storage: &dyn StorageProvider,
    identity_key: &str,
    password: &str,
    options: Option<&Brc39Options>,
) -> WalletResult<Vec<u8>> {
    encrypt_brc39(&export_brc38(storage, identity_key).await?, password, options)
}

/// Import an encrypted BRC-39 file into storage.
pub async fn import_brc39<S: PortableStorage>(
    storage: &S,
    bytes: &[u8],
    password: &str,
    options: &Brc38ImportOptions,
) -> WalletResult<Brc38ImportResult> {
    import_brc38(storage, &decrypt_brc39(bytes, password)?, options).await
}

/// Encrypt a validated BRC-38 document into a BRC-39 file (TS `encryptBRC39`).
pub fn encrypt_brc39(
    data: &Brc38WalletData,
    password: &str,
    options: Option<&Brc39Options>,
) -> WalletResult<Vec<u8>> {
    let mut salt = [0u8; BRC39_SALT_LENGTH];
    let mut nonce = [0u8; BRC39_NONCE_LENGTH];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let iterations = options.and_then(|o| o.iterations).unwrap_or(BRC39_DEFAULT_ITERATIONS);
    let memory_kib = options.and_then(|o| o.memory_kib).unwrap_or(BRC39_DEFAULT_MEMORY_KIB);
    let parallelism = options.and_then(|o| o.parallelism).unwrap_or(BRC39_DEFAULT_PARALLELISM);
    if iterations < BRC39_DEFAULT_ITERATIONS {
        return Err(WalletError::BadRequest(
            "BRC-39 export iterations must not be weaker than the canonical default".to_string(),
        ));
    }
    if memory_kib < BRC39_DEFAULT_MEMORY_KIB {
        return Err(WalletError::BadRequest(
            "BRC-39 export memoryKiB must not be weaker than the canonical default".to_string(),
        ));
    }
    encrypt_with_params(data, password, &salt, &nonce, iterations, memory_kib, parallelism)
}

/// Assemble a BRC-39 file from explicit KDF inputs. Split from
/// [`encrypt_brc39`] so tests can reproduce TS fixtures deterministically;
/// only `encrypt_brc39` enforces the export-strength floor.
pub(super) fn encrypt_with_params(
    data: &Brc38WalletData,
    password: &str,
    salt: &[u8],
    nonce: &[u8],
    iterations: u32,
    memory_kib: u32,
    parallelism: u8,
) -> WalletResult<Vec<u8>> {
    validate_kdf_params(iterations, memory_kib, parallelism, BRC39_HASH_LENGTH as u8)?;
    let plaintext = canonicalize(data.as_value())?.into_bytes();
    let key = derive_brc39_key(password, salt, iterations, memory_kib, parallelism)?;
    let ciphertext_and_tag = aes_gcm_encrypt_ts_compat(&key, nonce, &plaintext)
        .map_err(|e| WalletError::Internal(format!("BRC-39 encryption failed: {e}")))?;

    let mut file = Vec::with_capacity(BRC39_HEADER_LENGTH + salt.len() + nonce.len() + ciphertext_and_tag.len());
    file.extend_from_slice(&BRC39_MAGIC);
    file.push(1); // format version
    file.push(1); // protector type: password
    file.push(38); // inner format
    file.push(1); // KDF type: Argon2id
    file.push(0); // flags
    file.push(salt.len() as u8);
    file.push(nonce.len() as u8);
    file.extend_from_slice(&iterations.to_be_bytes());
    file.extend_from_slice(&memory_kib.to_be_bytes());
    file.push(parallelism);
    file.push(BRC39_HASH_LENGTH as u8);
    file.extend_from_slice(&[0u8; BRC39_HEADER_LENGTH - 21]); // reserved
    file.extend_from_slice(salt);
    file.extend_from_slice(nonce);
    file.extend_from_slice(&ciphertext_and_tag);
    Ok(file)
}

struct Brc39Header {
    salt_length: usize,
    nonce_length: usize,
    iterations: u32,
    memory_kib: u32,
    parallelism: u8,
}

/// Parse and check the fixed header (TS `parseBRC39Header` +
/// `assertHeaderConstants`).
fn parse_brc39_header(file: &[u8]) -> WalletResult<Brc39Header> {
    let err = |msg: &str| WalletError::BadRequest(msg.to_string());
    if file.len() < BRC39_HEADER_LENGTH + BRC39_TAG_LENGTH + 2 {
        return Err(err("Invalid BRC-39 file: too short"));
    }
    if file[..4] != BRC39_MAGIC {
        return Err(err("Invalid BRC-39 file: bad magic"));
    }
    if file[4] != 1 {
        return Err(err("Unsupported BRC-39 format version"));
    }
    if file[5] != 1 {
        return Err(err("Unsupported BRC-39 protector type"));
    }
    if file[6] != 38 {
        return Err(err("Unsupported BRC-39 inner format"));
    }
    if file[7] != 1 {
        return Err(err("Unsupported BRC-39 KDF type"));
    }
    if file[8] != 0 {
        return Err(err("Invalid BRC-39 flags"));
    }
    if file[21..BRC39_HEADER_LENGTH].iter().any(|b| *b != 0) {
        return Err(err("Invalid BRC-39 reserved bytes"));
    }
    let salt_length = file[9] as usize;
    let nonce_length = file[10] as usize;
    if salt_length == 0 {
        return Err(err("Invalid BRC-39 salt length"));
    }
    if nonce_length == 0 {
        return Err(err("Invalid BRC-39 nonce length"));
    }
    let iterations = u32::from_be_bytes(file[11..15].try_into().expect("length checked"));
    let memory_kib = u32::from_be_bytes(file[15..19].try_into().expect("length checked"));
    let parallelism = file[19];
    let hash_length = file[20];
    validate_kdf_params(iterations, memory_kib, parallelism, hash_length)?;
    Ok(Brc39Header {
        salt_length,
        nonce_length,
        iterations,
        memory_kib,
        parallelism,
    })
}

/// Decrypt a BRC-39 file to its validated BRC-38 document (TS `decryptBRC39`).
pub fn decrypt_brc39(bytes: &[u8], password: &str) -> WalletResult<Brc38WalletData> {
    let header = parse_brc39_header(bytes)?;
    let payload_start = BRC39_HEADER_LENGTH + header.salt_length + header.nonce_length;
    if bytes.len() <= payload_start + BRC39_TAG_LENGTH {
        return Err(WalletError::BadRequest("Invalid BRC-39 ciphertext".to_string()));
    }
    let salt = &bytes[BRC39_HEADER_LENGTH..BRC39_HEADER_LENGTH + header.salt_length];
    let nonce = &bytes[BRC39_HEADER_LENGTH + header.salt_length..payload_start];
    let ciphertext_and_tag = &bytes[payload_start..];
    let key = derive_brc39_key(
        password,
        salt,
        header.iterations,
        header.memory_kib,
        header.parallelism,
    )?;
    let plaintext = aes_gcm_decrypt_ts_compat(&key, nonce, ciphertext_and_tag)
        .map_err(|_| WalletError::BadRequest("BRC-39 authentication failed".to_string()))?;
    let json = String::from_utf8(plaintext)
        .map_err(|_| WalletError::BadRequest("BRC-39 plaintext is not UTF-8".to_string()))?;
    parse_brc38_json(&json)
}

/// Derive the AES key: Argon2id (v0x13) over the NFC-normalized password
/// (TS `deriveBRC39Key` via hash-wasm).
fn derive_brc39_key(
    password: &str,
    salt: &[u8],
    iterations: u32,
    memory_kib: u32,
    parallelism: u8,
) -> WalletResult<[u8; BRC39_HASH_LENGTH]> {
    let normalized: String = password.nfc().collect();
    let params = Params::new(memory_kib, iterations, parallelism as u32, Some(BRC39_HASH_LENGTH))
        .map_err(|e| WalletError::BadRequest(format!("Invalid BRC-39 Argon2id parameters: {e}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; BRC39_HASH_LENGTH];
    argon2
        .hash_password_into(normalized.as_bytes(), salt, &mut key)
        .map_err(|e| WalletError::Internal(format!("BRC-39 key derivation failed: {e}")))?;
    Ok(key)
}

/// TS `validateKdfParams`.
fn validate_kdf_params(
    iterations: u32,
    memory_kib: u32,
    parallelism: u8,
    hash_length: u8,
) -> WalletResult<()> {
    if iterations == 0 {
        return Err(WalletError::BadRequest("Invalid BRC-39 Argon2id iterations".to_string()));
    }
    if memory_kib == 0 {
        return Err(WalletError::BadRequest("Invalid BRC-39 Argon2id memoryKiB".to_string()));
    }
    if parallelism == 0 {
        return Err(WalletError::BadRequest("Invalid BRC-39 Argon2id parallelism".to_string()));
    }
    if hash_length as usize != BRC39_HASH_LENGTH {
        return Err(WalletError::BadRequest("Invalid BRC-39 Argon2id hashLength".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/portable/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture generated by wallet-toolbox portable-fixture-gen.test.ts")
    }

    /// The fixtures were encrypted by TS with the decomposed password
    /// "Cafe\u{301} fixture pw"; decrypting with the precomposed form proves
    /// NFC normalization matches across implementations.
    const PASSWORD_NFC: &str = "Caf\u{e9} fixture pw";

    /// Round-trip the deterministic TS fixture: Rust must decrypt it, and
    /// re-encrypting the same plaintext with the same salt/nonce/KDF params
    /// must reproduce the file byte-for-byte. This is the empirical proof
    /// that Argon2id and the TS SDK AES-GCM framing match, established
    /// against real TS output rather than by reading docs.
    #[test]
    fn brc39_lowcost_fixture_encrypts_and_decrypts_byte_exactly() {
        let file = fixture("brc39-ts-lowcost.bin");
        let plaintext = fixture("brc38-ts-export.json");

        let decrypted = decrypt_brc39(&file, PASSWORD_NFC).unwrap();
        assert_eq!(
            canonicalize(decrypted.as_value()).unwrap().into_bytes(),
            plaintext,
            "decrypted document must canonicalize to the TS plaintext bytes"
        );

        let salt = [7u8; 32];
        let nonce = [9u8; 32];
        let reencrypted =
            encrypt_with_params(&decrypted, PASSWORD_NFC, &salt, &nonce, 1, 64, 1).unwrap();
        assert_eq!(reencrypted, file, "Rust encryption must be byte-identical to TS");
    }

    /// The default-KDF fixture came from the real TS `encryptBRC39` path
    /// (7 iterations, 128 MiB): random salt/nonce, so only decryption is
    /// byte-comparable -- via the canonical plaintext.
    #[test]
    fn brc39_default_kdf_fixture_decrypts() {
        let file = fixture("brc39-ts-default.bin");
        let plaintext = fixture("brc38-ts-export.json");
        let decrypted = decrypt_brc39(&file, PASSWORD_NFC).unwrap();
        assert_eq!(
            canonicalize(decrypted.as_value()).unwrap().into_bytes(),
            plaintext
        );
    }

    #[test]
    fn brc39_wrong_password_fails_authentication() {
        let file = fixture("brc39-ts-lowcost.bin");
        let err = decrypt_brc39(&file, "wrong-password").unwrap_err();
        assert!(err.to_string().contains("authentication failed"), "{err}");
    }

    #[test]
    fn brc39_rejects_tampered_headers() {
        let file = fixture("brc39-ts-lowcost.bin");
        let tamper = |offset: usize, value: u8| {
            let mut copy = file.clone();
            copy[offset] = value;
            decrypt_brc39(&copy, PASSWORD_NFC).unwrap_err().to_string()
        };
        assert!(tamper(0, 0).contains("bad magic"));
        assert!(tamper(4, 2).contains("format version"));
        assert!(tamper(5, 2).contains("protector type"));
        assert!(tamper(6, 39).contains("inner format"));
        assert!(tamper(7, 2).contains("KDF type"));
        assert!(tamper(8, 1).contains("flags"));
        assert!(tamper(21, 1).contains("reserved"));
        assert!(tamper(9, 0).contains("salt length"));
        assert!(tamper(10, 0).contains("nonce length"));
        assert!(tamper(20, 31).contains("hashLength"));
        // Zeroed iterations (u32 BE at 11..15; fixture value is 1).
        let mut zero_iter = file.clone();
        zero_iter[14] = 0;
        assert!(decrypt_brc39(&zero_iter, PASSWORD_NFC)
            .unwrap_err()
            .to_string()
            .contains("iterations"));
        // Flipped last ciphertext/tag byte fails authentication.
        let last = file.len() - 1;
        assert!(tamper(last, file[last] ^ 1).contains("authentication failed"));
    }

    #[test]
    fn brc39_export_rejects_weakened_kdf_params() {
        let doc = decrypt_brc39(&fixture("brc39-ts-lowcost.bin"), PASSWORD_NFC).unwrap();
        let weak_iterations = Brc39Options {
            iterations: Some(1),
            ..Default::default()
        };
        let err = encrypt_brc39(&doc, "pw", Some(&weak_iterations)).unwrap_err();
        assert!(err.to_string().contains("canonical default"), "{err}");
        let weak_memory = Brc39Options {
            memory_kib: Some(64),
            ..Default::default()
        };
        let err = encrypt_brc39(&doc, "pw", Some(&weak_memory)).unwrap_err();
        assert!(err.to_string().contains("canonical default"), "{err}");
    }
}
