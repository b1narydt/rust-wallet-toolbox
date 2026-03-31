//! Chaintracks ingestors
//!
//! This module provides ingestor implementations for fetching blockchain headers
//! from various sources:
//!
//! ## Bulk Ingestors (Historical Data)
//! - `BulkCdnIngestor` - Downloads headers from Babbage CDN (fast, preferred)
//! - `BulkWocIngestor` - Uses WhatsOnChain API (slower, reliable fallback)
//!
//! ## Live Ingestors (Real-time Updates)
//! - `LivePollingIngestor` - Polls WOC API at intervals (simple, reliable)
//! - `LiveWebSocketIngestor` - Uses WOC WebSocket for instant notifications (low latency)

mod bulk_cdn;
mod bulk_woc;
mod live_polling;
#[cfg(feature = "chaintracks-ws")]
mod live_websocket;

pub use bulk_cdn::*;
pub use bulk_woc::*;
pub use live_polling::*;
#[cfg(feature = "chaintracks-ws")]
pub use live_websocket::*;

use serde::{Deserialize, Serialize};

use crate::chaintracks::BlockHeader;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default CDN URL for block headers (Babbage Systems)
pub const DEFAULT_CDN_URL: &str = "https://bsv-headers.babbage.systems/";

/// Legacy CDN URL (project babbage)
pub const LEGACY_CDN_URL: &str = "https://cdn.projectbabbage.com/blockheaders/";

/// WhatsOnChain API base URL for mainnet
pub const WOC_API_URL_MAIN: &str = "https://api.whatsonchain.com/v1/bsv/main";

/// WhatsOnChain API base URL for testnet
pub const WOC_API_URL_TEST: &str = "https://api.whatsonchain.com/v1/bsv/test";

/// WhatsOnChain WebSocket URL for mainnet
pub const WOC_WS_URL_MAIN: &str = "wss://socket-v2.whatsonchain.com/websocket/blockHeaders";

/// WhatsOnChain WebSocket URL for testnet
pub const WOC_WS_URL_TEST: &str =
    "wss://socket-v2-testnet.whatsonchain.com/websocket/blockHeaders";

// ---------------------------------------------------------------------------
// WoC response types
// ---------------------------------------------------------------------------

/// WhatsOnChain chain info response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocChainInfo {
    pub chain: String,
    pub blocks: u32,
    pub headers: u32,
    #[serde(rename = "bestblockhash")]
    pub best_block_hash: String,
    pub difficulty: f64,
    #[serde(rename = "mediantime")]
    pub median_time: u64,
}

/// WhatsOnChain header response from `/block/{hash}/header`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocHeaderResponse {
    pub hash: String,
    pub confirmations: u32,
    pub size: u64,
    pub height: u32,
    pub version: u32,
    #[serde(rename = "versionHex")]
    pub version_hex: String,
    pub merkleroot: String,
    pub time: u32,
    #[serde(rename = "mediantime")]
    pub median_time: u32,
    pub nonce: u32,
    /// Hex-encoded bits value (e.g. `"1d00ffff"`).
    pub bits: String,
    pub difficulty: f64,
    pub chainwork: String,
    pub previousblockhash: Option<String>,
    pub nextblockhash: Option<String>,
}

/// WhatsOnChain header byte file links response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocHeaderByteFileLinks {
    pub files: Vec<String>,
}

/// WhatsOnChain header from the `/block/headers` polling endpoint.
///
/// Note: `bits` is a hex string here, unlike the WebSocket variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocGetHeadersHeader {
    pub hash: String,
    pub confirmations: u32,
    pub size: u64,
    pub height: u32,
    pub version: u32,
    #[serde(rename = "versionHex")]
    pub version_hex: String,
    pub merkleroot: String,
    pub time: u32,
    #[serde(rename = "mediantime")]
    pub median_time: u32,
    pub nonce: u32,
    /// Hex-encoded bits value (e.g. `"1d00ffff"`).
    pub bits: String,
    pub difficulty: f64,
    pub chainwork: String,
    #[serde(rename = "previousblockhash")]
    pub previous_block_hash: Option<String>,
    #[serde(rename = "nextblockhash")]
    pub next_block_hash: Option<String>,
    #[serde(rename = "nTx", default)]
    pub n_tx: u32,
    #[serde(default)]
    pub num_tx: u32,
}

/// WebSocket message types from WOC.
///
/// **Variant order matters** for `#[serde(untagged)]`: more constrained variants
/// (with required fields) must appear before variants with all-optional fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum WocWsMessage {
    /// Connection info (has required `connect` field).
    Connect {
        connect: String,
    },
    /// Typed message with required `type` field (subscribe, unsubscribe, etc.).
    TypedMessage {
        #[serde(rename = "type")]
        msg_type: u32,
        channel: Option<String>,
        data: Option<serde_json::Value>,
    },
    /// Header data message (all fields optional, so tried after more specific variants).
    HeaderData {
        channel: Option<String>,
        #[serde(rename = "pub")]
        pub_data: Option<WocPubData>,
        data: Option<WocPubData>,
    },
    /// Empty ping response (matches `{}`; must be last since it has no required fields).
    Empty {},
}

/// Published header data wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocPubData {
    pub data: Option<WocWsBlockHeader>,
}

/// Block header from WebSocket.
///
/// Note: `bits` is a numeric `u32` here, unlike the polling variant where it is hex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WocWsBlockHeader {
    pub hash: String,
    pub height: u32,
    pub version: u32,
    #[serde(rename = "previousblockhash")]
    pub previous_block_hash: Option<String>,
    pub merkleroot: String,
    pub time: u32,
    /// Numeric bits value (already decoded, not hex).
    pub bits: u32,
    pub nonce: u32,
    #[serde(default)]
    pub confirmations: u32,
    #[serde(default)]
    pub size: u64,
}

// ---------------------------------------------------------------------------
// CDN types
// ---------------------------------------------------------------------------

/// CDN bulk header file metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkHeaderFileInfo {
    /// File name.
    pub file_name: String,
    /// Starting height.
    pub from_height: u32,
    /// Ending height.
    pub to_height: u32,
    /// Number of headers in file.
    pub count: u32,
    /// SHA256 hash of file contents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<String>,
    /// Chain this file is for.
    pub chain: String,
    /// Source URL override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// CDN file listing response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkHeaderFilesInfo {
    /// List of available files.
    pub files: Vec<BulkHeaderFileInfo>,
    /// Default headers per file.
    pub headers_per_file: u32,
    /// Last updated timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a [`WocGetHeadersHeader`] (from the polling REST API) into a [`BlockHeader`].
///
/// Parses the hex `bits` field into a `u32`.
pub fn woc_header_to_block_header(woc: &WocGetHeadersHeader) -> BlockHeader {
    let bits = u32::from_str_radix(&woc.bits, 16).unwrap_or(0);
    let previous_hash = woc
        .previous_block_hash
        .clone()
        .unwrap_or_else(|| "0".repeat(64));
    BlockHeader {
        version: woc.version,
        previous_hash,
        merkle_root: woc.merkleroot.clone(),
        time: woc.time,
        bits,
        nonce: woc.nonce,
        height: woc.height,
        hash: woc.hash.clone(),
    }
}

/// Convert a [`WocWsBlockHeader`] (from the WebSocket stream) into a [`BlockHeader`].
///
/// `bits` is already a numeric `u32`, so no hex parsing is needed.
pub fn ws_header_to_block_header(woc: &WocWsBlockHeader) -> BlockHeader {
    let previous_hash = woc
        .previous_block_hash
        .clone()
        .unwrap_or_else(|| "0".repeat(64));
    BlockHeader {
        version: woc.version,
        previous_hash,
        merkle_root: woc.merkleroot.clone(),
        time: woc.time,
        bits: woc.bits,
        nonce: woc.nonce,
        height: woc.height,
        hash: woc.hash.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdn_url_constants() {
        assert!(DEFAULT_CDN_URL.starts_with("https://"));
        assert!(DEFAULT_CDN_URL.contains("bsv-headers"));
        assert!(LEGACY_CDN_URL.starts_with("https://"));
        assert!(LEGACY_CDN_URL.contains("projectbabbage"));
    }

    #[test]
    fn test_woc_url_constants() {
        assert!(WOC_API_URL_MAIN.contains("whatsonchain"));
        assert!(WOC_API_URL_MAIN.contains("main"));
        assert!(WOC_API_URL_TEST.contains("whatsonchain"));
        assert!(WOC_API_URL_TEST.contains("test"));
        assert!(WOC_WS_URL_MAIN.contains("whatsonchain"));
        assert!(WOC_WS_URL_TEST.contains("whatsonchain"));
    }

    #[test]
    fn test_woc_header_to_block_header() {
        let woc = WocGetHeadersHeader {
            hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            confirmations: 800000,
            size: 285,
            height: 0,
            version: 1,
            version_hex: "00000001".to_string(),
            merkleroot: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                .to_string(),
            time: 1231006505,
            median_time: 1231006505,
            nonce: 2083236893,
            bits: "1d00ffff".to_string(),
            difficulty: 1.0,
            chainwork: "0".repeat(64),
            previous_block_hash: None,
            next_block_hash: None,
            n_tx: 1,
            num_tx: 1,
        };

        let header = woc_header_to_block_header(&woc);
        assert_eq!(header.height, 0);
        assert_eq!(header.nonce, 2083236893);
        assert_eq!(header.bits, 0x1d00ffff);
        assert_eq!(header.previous_hash, "0".repeat(64));
        assert_eq!(
            header.hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
        assert_eq!(
            header.merkle_root,
            "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        );
    }

    #[test]
    fn test_woc_header_to_block_header_with_previous() {
        let woc = WocGetHeadersHeader {
            hash: "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048".to_string(),
            confirmations: 799999,
            size: 215,
            height: 1,
            version: 1,
            version_hex: "00000001".to_string(),
            merkleroot: "0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098"
                .to_string(),
            time: 1231469665,
            median_time: 1231469665,
            nonce: 2573394689,
            bits: "1d00ffff".to_string(),
            difficulty: 1.0,
            chainwork: "0".repeat(64),
            previous_block_hash: Some(
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            ),
            next_block_hash: Some(
                "000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd".to_string(),
            ),
            n_tx: 1,
            num_tx: 1,
        };

        let header = woc_header_to_block_header(&woc);
        assert_eq!(header.height, 1);
        assert_eq!(
            header.previous_hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_ws_header_to_block_header() {
        let ws = WocWsBlockHeader {
            hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            height: 0,
            version: 1,
            previous_block_hash: None,
            merkleroot: "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
                .to_string(),
            time: 1231006505,
            bits: 486604799, // 0x1d00ffff in decimal
            nonce: 2083236893,
            confirmations: 800000,
            size: 285,
        };

        let header = ws_header_to_block_header(&ws);
        assert_eq!(header.height, 0);
        assert_eq!(header.nonce, 2083236893);
        assert_eq!(header.bits, 486604799);
        assert_eq!(header.previous_hash, "0".repeat(64));
        assert_eq!(
            header.hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_ws_header_to_block_header_with_previous() {
        let ws = WocWsBlockHeader {
            hash: "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048".to_string(),
            height: 1,
            version: 1,
            previous_block_hash: Some(
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            ),
            merkleroot: "0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098"
                .to_string(),
            time: 1231469665,
            bits: 486604799,
            nonce: 2573394689,
            confirmations: 799999,
            size: 215,
        };

        let header = ws_header_to_block_header(&ws);
        assert_eq!(header.height, 1);
        assert_eq!(
            header.previous_hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_woc_chain_info_deserialization() {
        let json = r#"{
            "chain": "main",
            "blocks": 800000,
            "headers": 800000,
            "bestblockhash": "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
            "difficulty": 1.0,
            "mediantime": 1231006505
        }"#;
        let info: WocChainInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.chain, "main");
        assert_eq!(info.blocks, 800000);
        assert_eq!(
            info.best_block_hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[test]
    fn test_woc_header_response_deserialization() {
        let json = r#"{
            "hash": "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
            "confirmations": 800000,
            "size": 285,
            "height": 0,
            "version": 1,
            "versionHex": "00000001",
            "merkleroot": "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
            "time": 1231006505,
            "mediantime": 1231006505,
            "nonce": 2083236893,
            "bits": "1d00ffff",
            "difficulty": 1.0,
            "chainwork": "0000000000000000000000000000000000000000000000000000000100010001",
            "nextblockhash": "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048"
        }"#;
        let resp: WocHeaderResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.height, 0);
        assert_eq!(resp.bits, "1d00ffff");
        assert!(resp.previousblockhash.is_none());
        assert!(resp.nextblockhash.is_some());
    }

    #[test]
    fn test_bulk_header_file_info_deserialization() {
        let json = r#"{
            "fileName": "0_99999_headers.bin",
            "fromHeight": 0,
            "toHeight": 99999,
            "count": 100000,
            "fileHash": "abc123",
            "chain": "main",
            "sourceUrl": "https://example.com/headers.bin"
        }"#;
        let info: BulkHeaderFileInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.file_name, "0_99999_headers.bin");
        assert_eq!(info.from_height, 0);
        assert_eq!(info.to_height, 99999);
        assert_eq!(info.count, 100000);
        assert_eq!(info.chain, "main");
    }

    #[test]
    fn test_bulk_header_files_info_deserialization() {
        let json = r#"{
            "files": [
                {
                    "fileName": "0_99999_headers.bin",
                    "fromHeight": 0,
                    "toHeight": 99999,
                    "count": 100000,
                    "chain": "main"
                }
            ],
            "headersPerFile": 100000,
            "lastUpdated": "2024-01-01T00:00:00Z"
        }"#;
        let info: BulkHeaderFilesInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.files.len(), 1);
        assert_eq!(info.headers_per_file, 100000);
        assert_eq!(info.last_updated.as_deref(), Some("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn test_woc_ws_block_header_deserialization() {
        let json = r#"{
            "hash": "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
            "height": 0,
            "version": 1,
            "previousblockhash": null,
            "merkleroot": "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
            "time": 1231006505,
            "bits": 486604799,
            "nonce": 2083236893
        }"#;
        let header: WocWsBlockHeader = serde_json::from_str(json).unwrap();
        assert_eq!(header.height, 0);
        assert_eq!(header.bits, 486604799);
        assert!(header.previous_block_hash.is_none());
    }

    #[test]
    fn test_woc_ws_message_connect() {
        let json = r#"{"connect": "hello"}"#;
        let msg: WocWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            WocWsMessage::Connect { connect } => assert_eq!(connect, "hello"),
            _ => panic!("expected Connect variant"),
        }
    }

    #[test]
    fn test_woc_ws_message_typed() {
        let json = r#"{"type": 5, "channel": "blockHeaders"}"#;
        let msg: WocWsMessage = serde_json::from_str(json).unwrap();
        match msg {
            WocWsMessage::TypedMessage {
                msg_type, channel, ..
            } => {
                assert_eq!(msg_type, 5);
                assert_eq!(channel.as_deref(), Some("blockHeaders"));
            }
            _ => panic!("expected TypedMessage variant"),
        }
    }
}
