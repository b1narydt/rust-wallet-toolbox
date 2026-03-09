//! CWI (CreateWalletIfNeeded) key management logic as free functions.
//!
//! Ports the key derivation and XOR logic from the TypeScript
//! `CWIStyleWalletManager` into standalone Rust functions. These are used
//! by `WalletAuthenticationManager` during authentication to derive root keys
//! from presentation keys and passwords.

use bsv::primitives::hash::pbkdf2_hmac_sha512;
use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::random::random_bytes;
use bsv::primitives::symmetric_key::SymmetricKey;

use crate::WalletError;

use super::types::StateSnapshot;

/// Default number of PBKDF2 iterations matching the TS constant `PBKDF2_NUM_ROUNDS`.
pub const PBKDF2_NUM_ROUNDS: u32 = 7777;

/// Snapshot format version 1 (legacy).
const SNAPSHOT_VERSION_1: u8 = 1;

/// Snapshot format version 2 (current, includes active_profile_id).
const SNAPSHOT_VERSION_2: u8 = 2;

/// Size of the snapshot encryption key in bytes (AES-256).
const SNAPSHOT_KEY_SIZE: usize = 32;

/// Size of the active profile ID field in V2 snapshots.
const PROFILE_ID_SIZE: usize = 16;

/// Size of the root primary key in the decrypted payload.
const ROOT_KEY_SIZE: usize = 32;

/// Derive a key from a password using PBKDF2-HMAC-SHA512.
///
/// Returns a 64-byte derived key. Uses bsv-sdk's `pbkdf2_hmac_sha512`
/// implementation matching the TS `pbkdf2NativeOrJs` function.
///
/// # Arguments
/// * `password` - Raw password bytes
/// * `salt` - Salt bytes (typically from UMP token `password_salt` field)
/// * `iterations` - Number of PBKDF2 rounds (default: `PBKDF2_NUM_ROUNDS`)
pub fn derive_key_from_password(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    pbkdf2_hmac_sha512(password, salt, iterations, 64)
}

/// XOR two byte arrays of equal length.
///
/// Used for combining password-derived key with presentation key or recovery key
/// to encrypt/decrypt root key material. Panics in debug mode if lengths differ;
/// in release mode, XORs up to the length of the shorter array.
///
/// # Arguments
/// * `key1` - First byte array
/// * `key2` - Second byte array
pub fn xor_keys(key1: &[u8], key2: &[u8]) -> Vec<u8> {
    debug_assert_eq!(
        key1.len(),
        key2.len(),
        "xor_keys: arrays must have equal length ({} vs {})",
        key1.len(),
        key2.len()
    );
    key1.iter()
        .zip(key2.iter())
        .map(|(a, b)| a ^ b)
        .collect()
}

/// Derive the identity public key from root key bytes.
///
/// Constructs a `PrivateKey` from the first 32 bytes of the root key,
/// then returns the compressed public key in hex form.
///
/// # Arguments
/// * `root_key` - Root key material (at least 32 bytes)
pub fn derive_identity_key(root_key: &[u8]) -> Result<String, WalletError> {
    if root_key.len() < 32 {
        return Err(WalletError::Internal(format!(
            "Root key too short for identity derivation: {} bytes, need at least 32",
            root_key.len()
        )));
    }
    let pk = PrivateKey::from_bytes(&root_key[..32]).map_err(|e| {
        WalletError::Internal(format!("Failed to create PrivateKey from root key: {}", e))
    })?;
    let pub_key = pk.to_public_key();
    Ok(pub_key.to_der_hex())
}

/// Extract root key material from a state snapshot if available.
///
/// This higher-level helper errors if no raw encrypted bytes are available,
/// since `StateSnapshot` carries JSON-level state rather than the raw
/// encrypted blob. Use `restore_root_key_from_snapshot_bytes` for actual
/// decryption of the binary snapshot.
///
/// # Arguments
/// * `snapshot` - The state snapshot to extract from
pub fn restore_root_key_from_snapshot(snapshot: &StateSnapshot) -> Result<Vec<u8>, WalletError> {
    if snapshot.presentation_key.is_some() {
        // The presentation key alone is not sufficient to recover the root key;
        // it must be combined with password or recovery key via XOR against UMP token fields.
        return Err(WalletError::Internal(
            "Root key restoration from snapshot requires UMP token lookup and password/recovery key"
                .to_string(),
        ));
    }
    Err(WalletError::Internal(
        "No presentation key in snapshot; re-authentication required".to_string(),
    ))
}

/// Restore root key and active profile ID from raw encrypted snapshot bytes.
///
/// Supports both V1 and V2 snapshot formats matching the TypeScript
/// `CWIStyleWalletManager.loadSnapshot` behavior.
///
/// ## Snapshot format
///
/// **V1:** `[1 byte version=1] [32 byte snapshot_key] [encrypted_payload]`
///
/// **V2:** `[1 byte version=2] [32 byte snapshot_key] [16 byte active_profile_id] [encrypted_payload]`
///
/// The encrypted payload is AES-256-GCM encrypted by the snapshot_key. After
/// decryption, the payload contains:
/// - Bytes 0..31: root_primary_key (32 bytes)
/// - Remaining: varint length + serialized UMP token (currently unused)
///
/// # Returns
/// A tuple of `(root_key: Vec<u8>, active_profile_id: Vec<u8>)`.
/// For V1 snapshots, active_profile_id is 16 zero bytes.
pub fn restore_root_key_from_snapshot_bytes(
    snapshot_bytes: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), WalletError> {
    if snapshot_bytes.is_empty() {
        return Err(WalletError::Internal(
            "Snapshot bytes are empty".to_string(),
        ));
    }

    let version = snapshot_bytes[0];

    match version {
        SNAPSHOT_VERSION_1 => restore_v1(snapshot_bytes),
        SNAPSHOT_VERSION_2 => restore_v2(snapshot_bytes),
        _ => Err(WalletError::Internal(format!(
            "Unsupported snapshot version: {}",
            version
        ))),
    }
}

/// Parse and decrypt a V1 snapshot.
///
/// Layout: `[1 byte version] [32 byte key] [encrypted_payload]`
fn restore_v1(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), WalletError> {
    let min_len = 1 + SNAPSHOT_KEY_SIZE; // version + key; encrypted payload is at least 48 bytes
    if data.len() <= min_len {
        return Err(WalletError::Internal(format!(
            "V1 snapshot too short: {} bytes, need more than {}",
            data.len(),
            min_len
        )));
    }

    let snapshot_key_bytes = &data[1..1 + SNAPSHOT_KEY_SIZE];
    let encrypted_payload = &data[1 + SNAPSHOT_KEY_SIZE..];

    let sym_key = SymmetricKey::from_bytes(snapshot_key_bytes).map_err(|e| {
        WalletError::Internal(format!("Invalid snapshot key: {}", e))
    })?;

    let decrypted = sym_key.decrypt(encrypted_payload).map_err(|e| {
        WalletError::Internal(format!("Snapshot decryption failed: {}", e))
    })?;

    if decrypted.len() < ROOT_KEY_SIZE {
        return Err(WalletError::Internal(format!(
            "Decrypted payload too short for root key: {} bytes, need at least {}",
            decrypted.len(),
            ROOT_KEY_SIZE
        )));
    }

    let root_key = decrypted[..ROOT_KEY_SIZE].to_vec();
    let active_profile_id = vec![0u8; PROFILE_ID_SIZE]; // V1 has no profile ID

    Ok((root_key, active_profile_id))
}

/// Parse and decrypt a V2 snapshot.
///
/// Layout: `[1 byte version] [32 byte key] [16 byte profile_id] [encrypted_payload]`
fn restore_v2(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), WalletError> {
    let min_len = 1 + SNAPSHOT_KEY_SIZE + PROFILE_ID_SIZE;
    if data.len() <= min_len {
        return Err(WalletError::Internal(format!(
            "V2 snapshot too short: {} bytes, need more than {}",
            data.len(),
            min_len
        )));
    }

    let snapshot_key_bytes = &data[1..1 + SNAPSHOT_KEY_SIZE];
    let active_profile_id = data[1 + SNAPSHOT_KEY_SIZE..1 + SNAPSHOT_KEY_SIZE + PROFILE_ID_SIZE].to_vec();
    let encrypted_payload = &data[1 + SNAPSHOT_KEY_SIZE + PROFILE_ID_SIZE..];

    let sym_key = SymmetricKey::from_bytes(snapshot_key_bytes).map_err(|e| {
        WalletError::Internal(format!("Invalid snapshot key: {}", e))
    })?;

    let decrypted = sym_key.decrypt(encrypted_payload).map_err(|e| {
        WalletError::Internal(format!("Snapshot decryption failed: {}", e))
    })?;

    if decrypted.len() < ROOT_KEY_SIZE {
        return Err(WalletError::Internal(format!(
            "Decrypted payload too short for root key: {} bytes, need at least {}",
            decrypted.len(),
            ROOT_KEY_SIZE
        )));
    }

    let root_key = decrypted[..ROOT_KEY_SIZE].to_vec();

    Ok((root_key, active_profile_id))
}

/// Encode a varint (unsigned LEB128) into a byte vector.
fn encode_varint(mut value: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
    buf
}

/// Create a V2 encrypted snapshot from root key material.
///
/// Generates a random 32-byte snapshot key, encrypts the payload (root key +
/// varint-prefixed UMP token bytes), and assembles the full snapshot.
///
/// ## Output format
///
/// `[1 byte version=2] [32 byte snapshot_key] [16 byte active_profile_id] [encrypted_payload]`
///
/// ## Encrypted payload format
///
/// `[32 byte root_key] [varint(token_bytes.len())] [token_bytes]`
///
/// # Arguments
/// * `root_key` - The 32-byte root primary key
/// * `active_profile_id` - The 16-byte active profile identifier
/// * `token_bytes` - Serialized UMP token bytes (can be empty)
pub fn save_snapshot(
    root_key: &[u8],
    active_profile_id: &[u8],
    token_bytes: &[u8],
) -> Result<Vec<u8>, WalletError> {
    if root_key.len() != ROOT_KEY_SIZE {
        return Err(WalletError::Internal(format!(
            "Root key must be {} bytes, got {}",
            ROOT_KEY_SIZE,
            root_key.len()
        )));
    }
    if active_profile_id.len() != PROFILE_ID_SIZE {
        return Err(WalletError::Internal(format!(
            "Active profile ID must be {} bytes, got {}",
            PROFILE_ID_SIZE,
            active_profile_id.len()
        )));
    }

    // Build the plaintext payload: root_key + varint(len) + token_bytes
    let varint = encode_varint(token_bytes.len());
    let mut payload = Vec::with_capacity(ROOT_KEY_SIZE + varint.len() + token_bytes.len());
    payload.extend_from_slice(root_key);
    payload.extend_from_slice(&varint);
    payload.extend_from_slice(token_bytes);

    // Generate a random snapshot key and encrypt the payload.
    let snapshot_key_bytes = random_bytes(SNAPSHOT_KEY_SIZE);
    let sym_key = SymmetricKey::from_bytes(&snapshot_key_bytes).map_err(|e| {
        WalletError::Internal(format!("Failed to create snapshot key: {}", e))
    })?;

    let encrypted = sym_key.encrypt(&payload).map_err(|e| {
        WalletError::Internal(format!("Snapshot encryption failed: {}", e))
    })?;

    // Assemble: version(1) + snapshot_key(32) + profile_id(16) + encrypted_payload
    let mut result = Vec::with_capacity(1 + SNAPSHOT_KEY_SIZE + PROFILE_ID_SIZE + encrypted.len());
    result.push(SNAPSHOT_VERSION_2);
    result.extend_from_slice(&snapshot_key_bytes);
    result.extend_from_slice(active_profile_id);
    result.extend_from_slice(&encrypted);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_keys() {
        let a = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let b = vec![0x11, 0x22, 0x33, 0x44];
        let result = xor_keys(&a, &b);
        assert_eq!(result, vec![0xBB, 0x99, 0xFF, 0x99]);

        // XOR with self should yield all zeros
        let zeros = xor_keys(&a, &a);
        assert_eq!(zeros, vec![0x00, 0x00, 0x00, 0x00]);

        // XOR with zeros is identity
        let z = vec![0x00, 0x00, 0x00, 0x00];
        assert_eq!(xor_keys(&a, &z), a);
    }

    #[test]
    fn test_derive_key_from_password() {
        let password = b"test-password";
        let salt = b"test-salt-value";
        let result = derive_key_from_password(password, salt, 100);
        // PBKDF2-HMAC-SHA512 with 64-byte output
        assert_eq!(
            result.len(),
            64,
            "PBKDF2 output should be 64 bytes, got {}",
            result.len()
        );

        // Deterministic: same inputs produce same output
        let result2 = derive_key_from_password(password, salt, 100);
        assert_eq!(result, result2, "PBKDF2 should be deterministic");

        // Different salt produces different output
        let result3 = derive_key_from_password(password, b"different-salt", 100);
        assert_ne!(result, result3, "Different salt should produce different key");
    }

    // -----------------------------------------------------------------------
    // Snapshot save/restore round-trip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_save_restore_roundtrip_v2() {
        let root_key = vec![0xAB; 32];
        let profile_id = vec![0xCD; 16];
        let token_bytes = b"serialized-ump-token-data";

        let snapshot = save_snapshot(&root_key, &profile_id, token_bytes)
            .expect("save_snapshot should succeed");

        // Verify version byte
        assert_eq!(snapshot[0], SNAPSHOT_VERSION_2);

        let (restored_root, restored_profile) =
            restore_root_key_from_snapshot_bytes(&snapshot)
                .expect("restore should succeed");

        assert_eq!(restored_root, root_key, "root key should round-trip");
        assert_eq!(restored_profile, profile_id, "profile ID should round-trip");
    }

    #[test]
    fn test_save_restore_roundtrip_empty_token() {
        let root_key = vec![0xFF; 32];
        let profile_id = vec![0x00; 16];
        let token_bytes = b"";

        let snapshot = save_snapshot(&root_key, &profile_id, token_bytes)
            .expect("save_snapshot should succeed");

        let (restored_root, restored_profile) =
            restore_root_key_from_snapshot_bytes(&snapshot)
                .expect("restore should succeed");

        assert_eq!(restored_root, root_key);
        assert_eq!(restored_profile, profile_id);
    }

    #[test]
    fn test_save_restore_roundtrip_large_token() {
        let root_key = vec![0x42; 32];
        let profile_id = vec![0x13; 16];
        let token_bytes = vec![0x77; 1024]; // 1KB token

        let snapshot = save_snapshot(&root_key, &profile_id, &token_bytes)
            .expect("save_snapshot should succeed");

        let (restored_root, restored_profile) =
            restore_root_key_from_snapshot_bytes(&snapshot)
                .expect("restore should succeed");

        assert_eq!(restored_root, root_key);
        assert_eq!(restored_profile, profile_id);
    }

    #[test]
    fn test_v2_format_parsing() {
        // Manually construct a V2 snapshot and verify it parses correctly.
        let root_key = vec![0x11; 32];
        let profile_id = vec![0x22; 16];

        // Build payload: root_key + varint(0) (no token bytes)
        let mut payload = Vec::new();
        payload.extend_from_slice(&root_key);
        payload.push(0x00); // varint for 0 length

        let snapshot_key_bytes = random_bytes(32);
        let sym_key = SymmetricKey::from_bytes(&snapshot_key_bytes).unwrap();
        let encrypted = sym_key.encrypt(&payload).unwrap();

        let mut snapshot = Vec::new();
        snapshot.push(SNAPSHOT_VERSION_2);
        snapshot.extend_from_slice(&snapshot_key_bytes);
        snapshot.extend_from_slice(&profile_id);
        snapshot.extend_from_slice(&encrypted);

        let (restored_root, restored_profile) =
            restore_root_key_from_snapshot_bytes(&snapshot)
                .expect("V2 parsing should succeed");

        assert_eq!(restored_root, root_key);
        assert_eq!(restored_profile, profile_id);
    }

    #[test]
    fn test_v1_format_parsing() {
        // Manually construct a V1 snapshot (no profile_id).
        let root_key = vec![0x33; 32];

        // Build payload: root_key + varint(0)
        let mut payload = Vec::new();
        payload.extend_from_slice(&root_key);
        payload.push(0x00);

        let snapshot_key_bytes = random_bytes(32);
        let sym_key = SymmetricKey::from_bytes(&snapshot_key_bytes).unwrap();
        let encrypted = sym_key.encrypt(&payload).unwrap();

        let mut snapshot = Vec::new();
        snapshot.push(SNAPSHOT_VERSION_1);
        snapshot.extend_from_slice(&snapshot_key_bytes);
        snapshot.extend_from_slice(&encrypted);

        let (restored_root, restored_profile) =
            restore_root_key_from_snapshot_bytes(&snapshot)
                .expect("V1 parsing should succeed");

        assert_eq!(restored_root, root_key);
        // V1 returns zero profile ID
        assert_eq!(restored_profile, vec![0u8; 16]);
    }

    #[test]
    fn test_unsupported_version() {
        let data = vec![0xFF, 0x00, 0x00]; // version 255
        let result = restore_root_key_from_snapshot_bytes(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unsupported snapshot version"),
            "Error: {}",
            err_msg
        );
    }

    #[test]
    fn test_empty_snapshot_bytes() {
        let result = restore_root_key_from_snapshot_bytes(&[]);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("empty"), "Error: {}", err_msg);
    }

    #[test]
    fn test_truncated_v2_snapshot() {
        // Just version + partial key (no encrypted payload)
        let data = vec![SNAPSHOT_VERSION_2; 10];
        let result = restore_root_key_from_snapshot_bytes(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("too short"), "Error: {}", err_msg);
    }

    #[test]
    fn test_truncated_v1_snapshot() {
        let data = vec![SNAPSHOT_VERSION_1; 10];
        let result = restore_root_key_from_snapshot_bytes(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("too short"), "Error: {}", err_msg);
    }

    #[test]
    fn test_save_snapshot_invalid_root_key_size() {
        let result = save_snapshot(&[0u8; 16], &[0u8; 16], &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Root key must be 32 bytes"));
    }

    #[test]
    fn test_save_snapshot_invalid_profile_id_size() {
        let result = save_snapshot(&[0u8; 32], &[0u8; 8], &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Active profile ID must be 16 bytes"));
    }

    #[test]
    fn test_encode_varint_small() {
        assert_eq!(encode_varint(0), vec![0x00]);
        assert_eq!(encode_varint(1), vec![0x01]);
        assert_eq!(encode_varint(127), vec![0x7F]);
    }

    #[test]
    fn test_encode_varint_multi_byte() {
        assert_eq!(encode_varint(128), vec![0x80, 0x01]);
        assert_eq!(encode_varint(300), vec![0xAC, 0x02]);
    }

    #[test]
    fn test_two_snapshots_differ() {
        // Same input should produce different encrypted output due to random IV + key.
        let root_key = vec![0x42; 32];
        let profile_id = vec![0x00; 16];
        let s1 = save_snapshot(&root_key, &profile_id, &[]).unwrap();
        let s2 = save_snapshot(&root_key, &profile_id, &[]).unwrap();
        assert_ne!(s1, s2, "Two snapshots should differ due to random keys");

        // But both should restore to the same root key.
        let (r1, _) = restore_root_key_from_snapshot_bytes(&s1).unwrap();
        let (r2, _) = restore_root_key_from_snapshot_bytes(&s2).unwrap();
        assert_eq!(r1, root_key);
        assert_eq!(r2, root_key);
    }
}
