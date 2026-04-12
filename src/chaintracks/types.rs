//! Core data structures for chaintracks block header management.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::Chain;

// ---------------------------------------------------------------------------
// HeightRange
// ---------------------------------------------------------------------------

/// An inclusive range of block heights [low, high].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeightRange {
    pub low: u32,
    pub high: u32,
}

impl HeightRange {
    pub fn new(low: u32, high: u32) -> Self {
        Self { low, high }
    }

    /// Number of heights in the range (inclusive on both ends).
    pub fn count(&self) -> u32 {
        self.high - self.low + 1
    }

    /// Returns true if `height` is within [low, high].
    pub fn contains(&self, height: u32) -> bool {
        height >= self.low && height <= self.high
    }

    /// Returns true if `other` overlaps this range.
    pub fn overlaps(&self, other: &HeightRange) -> bool {
        self.low <= other.high && other.low <= self.high
    }

    /// Merge two ranges if they overlap or are adjacent (gap <= 1).
    /// Returns None if the gap is larger than 1.
    pub fn merge(&self, other: &HeightRange) -> Option<HeightRange> {
        // Adjacent: self.high + 1 >= other.low AND other.high + 1 >= self.low
        let self_adj = self.high.saturating_add(1);
        let other_adj = other.high.saturating_add(1);
        if self_adj >= other.low && other_adj >= self.low {
            Some(HeightRange::new(
                self.low.min(other.low),
                self.high.max(other.high),
            ))
        } else {
            None
        }
    }

    /// Subtract `other` from `self`, returning the remaining pieces.
    pub fn subtract(&self, other: &HeightRange) -> Vec<HeightRange> {
        // If no overlap, return self unchanged.
        if !self.overlaps(other) {
            return vec![self.clone()];
        }

        let mut result = Vec::new();

        // Left piece: [self.low, other.low - 1]
        if other.low > self.low {
            result.push(HeightRange::new(self.low, other.low - 1));
        }

        // Right piece: [other.high + 1, self.high]
        if other.high < self.high {
            result.push(HeightRange::new(other.high + 1, self.high));
        }

        result
    }
}

// ---------------------------------------------------------------------------
// BaseBlockHeader
// ---------------------------------------------------------------------------

/// The six raw fields of a Bitcoin block header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseBlockHeader {
    pub version: u32,
    /// Previous block hash in display (reversed) hex.
    pub previous_hash: String,
    /// Merkle root in display (reversed) hex.
    pub merkle_root: String,
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
}

/// Decode a display-order (big-endian) hex hash string to internal byte order
/// (little-endian / reversed) as a 32-byte array.
fn display_hex_to_internal(hex_str: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let decoded = hex::decode(hex_str).unwrap_or_else(|_| vec![0u8; 32]);
    let len = decoded.len().min(32);
    bytes[..len].copy_from_slice(&decoded[..len]);
    bytes.reverse();
    bytes
}

impl BaseBlockHeader {
    /// Serialize to the canonical 80-byte Bitcoin header format (all fields LE).
    pub fn to_bytes(&self) -> [u8; 80] {
        let mut buf = [0u8; 80];
        let mut offset = 0;

        // version: 4 bytes LE
        buf[offset..offset + 4].copy_from_slice(&self.version.to_le_bytes());
        offset += 4;

        // previous_hash: 32 bytes internal order
        let prev = display_hex_to_internal(&self.previous_hash);
        buf[offset..offset + 32].copy_from_slice(&prev);
        offset += 32;

        // merkle_root: 32 bytes internal order
        let merkle = display_hex_to_internal(&self.merkle_root);
        buf[offset..offset + 32].copy_from_slice(&merkle);
        offset += 32;

        // time: 4 bytes LE
        buf[offset..offset + 4].copy_from_slice(&self.time.to_le_bytes());
        offset += 4;

        // bits: 4 bytes LE
        buf[offset..offset + 4].copy_from_slice(&self.bits.to_le_bytes());
        offset += 4;

        // nonce: 4 bytes LE
        buf[offset..offset + 4].copy_from_slice(&self.nonce.to_le_bytes());

        buf
    }

    /// Compute the block hash and wrap into a `BlockHeader` at the given height.
    pub fn to_block_header_at_height(&self, height: u32) -> BlockHeader {
        let bytes = self.to_bytes();
        let hash = compute_block_hash(&bytes);
        BlockHeader {
            version: self.version,
            previous_hash: self.previous_hash.clone(),
            merkle_root: self.merkle_root.clone(),
            time: self.time,
            bits: self.bits,
            nonce: self.nonce,
            height,
            hash,
        }
    }
}

// ---------------------------------------------------------------------------
// BlockHeader
// ---------------------------------------------------------------------------

/// A `BaseBlockHeader` extended with height and computed hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockHeader {
    pub version: u32,
    pub previous_hash: String,
    pub merkle_root: String,
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
    pub height: u32,
    pub hash: String,
}

// ---------------------------------------------------------------------------
// LiveBlockHeader
// ---------------------------------------------------------------------------

/// A `BlockHeader` with additional chain-tracking metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBlockHeader {
    pub version: u32,
    pub previous_hash: String,
    pub merkle_root: String,
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
    pub height: u32,
    pub hash: String,
    pub chain_work: String,
    pub is_chain_tip: bool,
    pub is_active: bool,
    pub header_id: Option<u64>,
    pub previous_header_id: Option<u64>,
}

impl From<LiveBlockHeader> for BlockHeader {
    fn from(live: LiveBlockHeader) -> Self {
        BlockHeader {
            version: live.version,
            previous_hash: live.previous_hash,
            merkle_root: live.merkle_root,
            time: live.time,
            bits: live.bits,
            nonce: live.nonce,
            height: live.height,
            hash: live.hash,
        }
    }
}

// ---------------------------------------------------------------------------
// InsertHeaderResult
// ---------------------------------------------------------------------------

/// Result returned when inserting a header into the chain store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertHeaderResult {
    pub added: bool,
    pub dupe: bool,
    pub is_active_tip: bool,
    pub reorg_depth: u32,
    pub prior_tip: Option<BlockHeader>,
    pub deactivated_headers: Vec<BlockHeader>,
    pub no_prev: bool,
    pub bad_prev: bool,
    pub no_active_ancestor: bool,
    pub no_tip: bool,
}

// ---------------------------------------------------------------------------
// ChaintracksInfo
// ---------------------------------------------------------------------------

/// Summary information about a chaintracks instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaintracksInfo {
    pub chain: Option<Chain>,
    pub storage_type: String,
    pub ingestor_count: u32,
    pub tip_height: Option<u32>,
    pub live_range: Option<HeightRange>,
    pub is_listening: bool,
    pub is_synchronized: bool,
}

// ---------------------------------------------------------------------------
// Hash functions
// ---------------------------------------------------------------------------

/// Double-SHA256 of `header_bytes`, then reverse and hex-encode.
/// Produces the standard Bitcoin block hash in display (big-endian) order.
pub fn compute_block_hash(header_bytes: &[u8; 80]) -> String {
    let first = Sha256::digest(header_bytes);
    let second = Sha256::digest(&first);
    let mut hash_bytes: [u8; 32] = second.into();
    hash_bytes.reverse();
    hex::encode(hash_bytes)
}

/// Calculate the work represented by a compact `bits` difficulty value.
///
/// Work = (~target) / (target + 1) + 1, where target is the 256-bit value
/// encoded in compact form. Returns a 64-character hex string.
pub fn calculate_work(bits: u32) -> String {
    // Decode compact target into a 32-byte big-endian integer.
    let exponent = ((bits >> 24) & 0xff) as usize;
    let mantissa = bits & 0x007fffff;

    let mut target = [0u8; 32];
    if exponent > 0 && exponent <= 32 {
        let start = 32usize.saturating_sub(exponent);
        if start + 2 < 32 {
            target[start] = ((mantissa >> 16) & 0xff) as u8;
            target[start + 1] = ((mantissa >> 8) & 0xff) as u8;
            target[start + 2] = (mantissa & 0xff) as u8;
        }
    }

    // Compute work = (~target) / (target + 1) + 1 using big-endian u128 pairs.
    let (t_hi, t_lo) = bytes32_to_u128pair(&target);

    // ~target
    let (not_hi, not_lo) = (!t_hi, !t_lo);

    // target + 1
    let (t1_lo, carry) = t_lo.overflowing_add(1u128);
    let t1_hi = t_hi.wrapping_add(carry as u128);

    // (~target) / (target + 1) — 256-bit division
    let (div_hi, div_lo) = div256(not_hi, not_lo, t1_hi, t1_lo);

    // + 1
    let (result_lo, carry2) = div_lo.overflowing_add(1u128);
    let result_hi = div_hi.wrapping_add(carry2 as u128);

    // Encode as 64-char hex
    format!(
        "{:016x}{:016x}{:016x}{:016x}",
        (result_hi >> 64) as u64,
        result_hi as u64,
        (result_lo >> 64) as u64,
        result_lo as u64,
    )
}

fn bytes32_to_u128pair(b: &[u8; 32]) -> (u128, u128) {
    let hi = u128::from_be_bytes(b[0..16].try_into().unwrap());
    let lo = u128::from_be_bytes(b[16..32].try_into().unwrap());
    (hi, lo)
}

/// Unsigned 256-bit division: (a_hi:a_lo) / (b_hi:b_lo).
/// Returns (quotient_hi, quotient_lo). Panics if divisor is zero.
fn div256(a_hi: u128, a_lo: u128, b_hi: u128, b_lo: u128) -> (u128, u128) {
    if b_hi == 0 && b_lo == 0 {
        panic!("division by zero");
    }

    // Fast path: divisor fits in u128
    if b_hi == 0 {
        let (q_hi, r) = div128_by_u128(a_hi, 0, b_lo);
        let (q_lo, _) = div128_by_u128(a_lo, r, b_lo);
        return (q_hi, q_lo);
    }

    // General case: bit-by-bit long division on 32-byte arrays.
    let a = u128pair_to_bytes32(a_hi, a_lo);
    let b = u128pair_to_bytes32(b_hi, b_lo);
    let (q, _r) = bytes32_divmod(&a, &b);
    bytes32_to_u128pair(&q)
}

fn u128pair_to_bytes32(hi: u128, lo: u128) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0..16].copy_from_slice(&hi.to_be_bytes());
    b[16..32].copy_from_slice(&lo.to_be_bytes());
    b
}

/// Bit-by-bit long division of two 256-bit big-endian byte arrays.
/// Returns (quotient, remainder).
fn bytes32_divmod(a: &[u8; 32], b: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut q = [0u8; 32];
    let mut r = [0u8; 32];

    for i in 0..256usize {
        // Shift r left by 1
        shl1_bytes32(&mut r);
        // Set LSB of r to bit (255 - i) of a (MSB first)
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        let bit = (a[byte_idx] >> bit_idx) & 1;
        r[31] |= bit;

        // If r >= b, subtract b from r and set the corresponding quotient bit
        if cmp_bytes32(&r, b) >= 0 {
            sub_bytes32(&mut r, b);
            let q_byte = i / 8;
            let q_bit = 7 - (i % 8);
            q[q_byte] |= 1 << q_bit;
        }
    }

    (q, r)
}

fn shl1_bytes32(b: &mut [u8; 32]) {
    let mut carry = 0u8;
    for i in (0..32).rev() {
        let new_carry = b[i] >> 7;
        b[i] = (b[i] << 1) | carry;
        carry = new_carry;
    }
}

fn cmp_bytes32(a: &[u8; 32], b: &[u8; 32]) -> i32 {
    for i in 0..32 {
        if a[i] < b[i] {
            return -1;
        }
        if a[i] > b[i] {
            return 1;
        }
    }
    0
}

fn sub_bytes32(a: &mut [u8; 32], b: &[u8; 32]) {
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let diff = a[i] as i16 - b[i] as i16 - borrow;
        if diff < 0 {
            a[i] = (diff + 256) as u8;
            borrow = 1;
        } else {
            a[i] = diff as u8;
            borrow = 0;
        }
    }
}

/// Divide a 256-bit value (hi:lo) by a 128-bit divisor `d`.
/// Returns (quotient_lo_128, remainder) where the quotient fits in 128 bits.
fn div128_by_u128(hi: u128, lo: u128, d: u128) -> (u128, u128) {
    // Split into four 64-bit limbs and use multi-precision long division.
    let limbs = [(hi >> 64) as u64, hi as u64, (lo >> 64) as u64, lo as u64];

    let mut r: u128 = 0;
    let mut q_limbs = [0u64; 4];
    for (i, &limb) in limbs.iter().enumerate() {
        let cur = (r << 64) | limb as u128;
        q_limbs[i] = (cur / d) as u64;
        r = cur % d;
    }

    let q = ((q_limbs[2] as u128) << 64) | q_limbs[3] as u128;
    (q, r)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_height_range_count() {
        let r = HeightRange::new(10, 20);
        assert_eq!(r.count(), 11);
        let r = HeightRange::new(5, 5);
        assert_eq!(r.count(), 1);
    }

    #[test]
    fn test_height_range_contains() {
        let r = HeightRange::new(10, 20);
        assert!(r.contains(10));
        assert!(r.contains(15));
        assert!(r.contains(20));
        assert!(!r.contains(9));
        assert!(!r.contains(21));
    }

    #[test]
    fn test_height_range_merge() {
        let a = HeightRange::new(10, 20);
        let b = HeightRange::new(15, 30);
        assert_eq!(a.merge(&b), Some(HeightRange::new(10, 30)));
        let a = HeightRange::new(10, 20);
        let b = HeightRange::new(21, 30);
        assert_eq!(a.merge(&b), Some(HeightRange::new(10, 30)));
        let a = HeightRange::new(10, 20);
        let b = HeightRange::new(25, 30);
        assert_eq!(a.merge(&b), None);
    }

    #[test]
    fn test_height_range_subtract() {
        let a = HeightRange::new(10, 30);
        let b = HeightRange::new(15, 20);
        assert_eq!(
            a.subtract(&b),
            vec![HeightRange::new(10, 14), HeightRange::new(21, 30)]
        );
        let a = HeightRange::new(10, 30);
        let b = HeightRange::new(10, 20);
        assert_eq!(a.subtract(&b), vec![HeightRange::new(21, 30)]);
        let a = HeightRange::new(10, 20);
        let b = HeightRange::new(25, 30);
        assert_eq!(a.subtract(&b), vec![HeightRange::new(10, 20)]);
    }

    #[test]
    fn test_base_block_header_to_bytes() {
        let genesis = BaseBlockHeader {
            version: 1,
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            merkle_root: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                .to_string(),
            time: 1231006505,
            bits: 0x1d00ffff,
            nonce: 2083236893,
        };
        let bytes = genesis.to_bytes();
        assert_eq!(bytes.len(), 80);
    }

    #[test]
    fn test_compute_block_hash_genesis() {
        let genesis = BaseBlockHeader {
            version: 1,
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            merkle_root: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                .to_string(),
            time: 1231006505,
            bits: 0x1d00ffff,
            nonce: 2083236893,
        };
        let bytes = genesis.to_bytes();
        let hash = compute_block_hash(&bytes);
        assert_eq!(
            hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_to_block_header_at_height() {
        let base = BaseBlockHeader {
            version: 1,
            previous_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            merkle_root: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                .to_string(),
            time: 1231006505,
            bits: 0x1d00ffff,
            nonce: 2083236893,
        };
        let header = base.to_block_header_at_height(0);
        assert_eq!(header.height, 0);
        assert_eq!(
            header.hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_calculate_work() {
        let work = calculate_work(0x1d00ffff);
        assert!(!work.is_empty());
        assert_eq!(work.len(), 64);
    }

    #[test]
    fn test_block_header_serialization_roundtrip() {
        let header = BlockHeader {
            version: 1,
            previous_hash: "abc".to_string(),
            merkle_root: "def".to_string(),
            time: 12345,
            bits: 0x1d00ffff,
            nonce: 99,
            height: 10,
            hash: "ghi".to_string(),
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"previousHash\""));
        assert!(json.contains("\"merkleRoot\""));
        let deserialized: BlockHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.height, 10);
    }
}
