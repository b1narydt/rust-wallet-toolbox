//! WhatsOnChain Bulk Header Ingestor
//!
//! Uses WhatsOnChain API as a fallback for bulk header fetching.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::chaintracks::{
    BlockHeader, BulkIngestor, BulkSyncResult, ChaintracksStorageIngest, HeightRange,
};
use crate::error::{WalletError, WalletResult};
use crate::types::Chain;

use super::{WocChainInfo, WocHeaderByteFileLinks, WocHeaderResponse};
use super::{WOC_API_URL_MAIN, WOC_API_URL_TEST};

/// Parsed file link info
#[derive(Debug, Clone)]
pub struct FileLink {
    pub url: String,
    pub file_name: String,
    pub range: Option<HeightRange>,
    pub is_latest: bool,
}

/// Options for WhatsOnChain bulk ingestor
#[derive(Debug, Clone)]
pub struct BulkWocOptions {
    /// Chain to ingest
    pub chain: Chain,
    /// API key (optional, enables higher rate limits)
    pub api_key: Option<String>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// User agent for requests
    pub user_agent: String,
    /// Enable caching
    pub enable_cache: bool,
    /// How long chain info is valid (milliseconds)
    pub chain_info_ttl_ms: u64,
    /// Idle wait time between requests (milliseconds)
    pub idle_wait_ms: u64,
}

impl Default for BulkWocOptions {
    fn default() -> Self {
        BulkWocOptions {
            chain: Chain::Main,
            api_key: None,
            timeout_secs: 30,
            user_agent: "BsvWalletToolbox/1.0".to_string(),
            enable_cache: true,
            chain_info_ttl_ms: 5000,
            idle_wait_ms: 5000,
        }
    }
}

impl BulkWocOptions {
    /// Create options for mainnet
    pub fn mainnet() -> Self {
        Self::default()
    }

    /// Create options for testnet
    pub fn testnet() -> Self {
        BulkWocOptions {
            chain: Chain::Test,
            ..Default::default()
        }
    }

    /// Set API key
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
}

/// WhatsOnChain-based bulk header ingestor
///
/// Uses the WhatsOnChain API to fetch historical headers.
/// Slower than CDN but provides a reliable fallback.
pub struct BulkWocIngestor {
    options: BulkWocOptions,
    client: reqwest::Client,
    storage: RwLock<Option<Arc<dyn ChaintracksStorageIngest>>>,
    /// Cached chain info
    chain_info: RwLock<Option<(WocChainInfo, std::time::Instant)>>,
}

impl BulkWocIngestor {
    /// Create a new WhatsOnChain bulk ingestor
    pub fn new(options: BulkWocOptions) -> WalletResult<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(options.timeout_secs))
            .user_agent(&options.user_agent);

        if let Some(ref key) = options.api_key {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "woc-api-key",
                reqwest::header::HeaderValue::from_str(key).map_err(|_| {
                    WalletError::InvalidParameter {
                        parameter: "api_key".to_string(),
                        must_be: "a valid HTTP header value".to_string(),
                    }
                })?,
            );
            builder = builder.default_headers(headers);
        }

        let client = builder
            .build()
            .map_err(|e| WalletError::Internal(e.to_string()))?;

        Ok(BulkWocIngestor {
            options,
            client,
            storage: RwLock::new(None),
            chain_info: RwLock::new(None),
        })
    }

    /// Create a default mainnet ingestor
    pub fn mainnet() -> WalletResult<Self> {
        Self::new(BulkWocOptions::mainnet())
    }

    /// Create a default testnet ingestor
    pub fn testnet() -> WalletResult<Self> {
        Self::new(BulkWocOptions::testnet())
    }

    /// Get base API URL for the configured chain
    pub fn api_url(&self) -> &'static str {
        match self.options.chain {
            Chain::Main => WOC_API_URL_MAIN,
            Chain::Test => WOC_API_URL_TEST,
        }
    }

    /// Fetch chain info (with caching)
    pub async fn get_chain_info(&self) -> WalletResult<WocChainInfo> {
        // Check cache
        {
            let cache = self.chain_info.read().await;
            if let Some((ref info, ref timestamp)) = *cache {
                let elapsed = timestamp.elapsed().as_millis() as u64;
                if elapsed < self.options.chain_info_ttl_ms {
                    return Ok(info.clone());
                }
            }
        }

        // Fetch fresh
        let url = format!("{}/chain/info", self.api_url());
        debug!("Fetching chain info from: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WalletError::NetworkChain(format!(
                "WOC chain/info returned status {}",
                response.status()
            )));
        }

        let info: WocChainInfo = response
            .json()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        // Update cache
        {
            let mut cache = self.chain_info.write().await;
            *cache = Some((info.clone(), std::time::Instant::now()));
        }

        Ok(info)
    }

    /// Get current chain tip height
    pub async fn get_chain_tip_height(&self) -> WalletResult<u32> {
        let info = self.get_chain_info().await?;
        Ok(info.blocks)
    }

    /// Get current chain tip hash
    pub async fn get_chain_tip_hash(&self) -> WalletResult<String> {
        let info = self.get_chain_info().await?;
        Ok(info.best_block_hash)
    }

    /// Fetch header by hash
    pub async fn get_header_by_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>> {
        let url = format!("{}/block/{}/header", self.api_url(), hash);
        debug!("Fetching header by hash: {}", hash);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(WalletError::NetworkChain(format!(
                "WOC header lookup returned status {}",
                response.status()
            )));
        }

        let woc_header: WocHeaderResponse = response
            .json()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;
        Ok(Some(self.convert_woc_header(&woc_header)))
    }

    /// Fetch recent headers (last ~10 blocks)
    pub async fn get_recent_headers(&self) -> WalletResult<Vec<BlockHeader>> {
        let url = format!("{}/block/headers", self.api_url());
        debug!("Fetching recent headers");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WalletError::NetworkChain(format!(
                "WOC block/headers returned status {}",
                response.status()
            )));
        }

        let woc_headers: Vec<WocHeaderResponse> = response
            .json()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;
        let headers = woc_headers
            .iter()
            .map(|h| self.convert_woc_header(h))
            .collect();

        Ok(headers)
    }

    /// Fetch header byte file links
    pub async fn get_header_byte_file_links(&self) -> WalletResult<Vec<FileLink>> {
        let url = format!("{}/block/headers/resources", self.api_url());
        debug!("Fetching header file links");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WalletError::NetworkChain(format!(
                "WOC headers/resources returned status {}",
                response.status()
            )));
        }

        let links: WocHeaderByteFileLinks = response
            .json()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        let parsed: Vec<FileLink> = links
            .files
            .iter()
            .filter_map(|url| self.parse_file_link(url))
            .collect();

        Ok(parsed)
    }

    /// Parse a file link URL
    pub fn parse_file_link(&self, url: &str) -> Option<FileLink> {
        // Use reqwest::Url (always available) instead of the optional url crate
        let parsed = reqwest::Url::parse(url).ok()?;
        let file_name = parsed.path_segments()?.next_back()?.to_string();

        // Check if this is the "latest" file
        if file_name == "latest" {
            return Some(FileLink {
                url: url.to_string(),
                file_name,
                range: None,
                is_latest: true,
            });
        }

        // Parse range from filename: "0_999999_headers.bin" format
        let parts: Vec<&str> = file_name.split('_').collect();
        if parts.len() >= 2 {
            let from_height: u32 = parts[0].parse().ok()?;
            let to_height: u32 = parts[1].parse().ok()?;

            return Some(FileLink {
                url: url.to_string(),
                file_name,
                range: Some(HeightRange::new(from_height, to_height)),
                is_latest: false,
            });
        }

        None
    }

    /// Download binary header file
    pub async fn download_header_file(&self, link: &FileLink) -> WalletResult<Vec<u8>> {
        debug!("Downloading header file: {}", link.file_name);

        let response = self
            .client
            .get(&link.url)
            .send()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WalletError::NetworkChain(format!(
                "Failed to download {}: status {}",
                link.url,
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| WalletError::NetworkChain(e.to_string()))?;
        Ok(bytes.to_vec())
    }

    /// Parse headers from binary data
    pub fn parse_headers(&self, data: &[u8], start_height: u32) -> Vec<BlockHeader> {
        let mut headers = Vec::with_capacity(data.len() / 80);

        for (i, chunk) in data.chunks(80).enumerate() {
            if chunk.len() != 80 {
                warn!("Incomplete header chunk at index {}", i);
                break;
            }

            let header = self.deserialize_header(chunk, start_height + i as u32);
            headers.push(header);
        }

        headers
    }

    /// Deserialize a single 80-byte header
    pub fn deserialize_header(&self, data: &[u8], height: u32) -> BlockHeader {
        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);

        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(&data[4..36]);
        prev_hash.reverse();
        let previous_hash = hex::encode(prev_hash);

        let mut merkle = [0u8; 32];
        merkle.copy_from_slice(&data[36..68]);
        merkle.reverse();
        let merkle_root = hex::encode(merkle);

        let time = u32::from_le_bytes([data[68], data[69], data[70], data[71]]);
        let bits = u32::from_le_bytes([data[72], data[73], data[74], data[75]]);
        let nonce = u32::from_le_bytes([data[76], data[77], data[78], data[79]]);

        let hash = self.compute_block_hash(data);

        BlockHeader {
            version,
            previous_hash,
            merkle_root,
            time,
            bits,
            nonce,
            height,
            hash,
        }
    }

    /// Compute double SHA256 hash of header bytes
    pub fn compute_block_hash(&self, header_bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(header_bytes);
        let first_hash = hasher.finalize();

        let mut hasher = Sha256::new();
        hasher.update(first_hash);
        let second_hash = hasher.finalize();

        let mut reversed = second_hash.to_vec();
        reversed.reverse();

        hex::encode(reversed)
    }

    /// Convert WOC header response to BlockHeader
    pub fn convert_woc_header(&self, woc: &WocHeaderResponse) -> BlockHeader {
        // Parse bits from hex string
        let bits = u32::from_str_radix(&woc.bits, 16).unwrap_or(0);

        let previous_hash = woc
            .previousblockhash
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
}

#[async_trait]
impl BulkIngestor for BulkWocIngestor {
    async fn get_present_height(&self) -> WalletResult<u32> {
        let height = self.get_chain_tip_height().await?;
        Ok(height)
    }

    async fn synchronize(&self) -> WalletResult<BulkSyncResult> {
        info!("WOC bulk sync: fetching headers");

        let links = self.get_header_byte_file_links().await?;

        let mut all_headers = Vec::new();
        let mut last_height: Option<u32> = None;

        for link in &links {
            // Skip if no range info and not latest
            if link.range.is_none() && !link.is_latest {
                continue;
            }

            let data = self.download_header_file(link).await?;

            let start_height = link
                .range
                .as_ref()
                .map(|r| r.low)
                .or(last_height.map(|h| h + 1))
                .unwrap_or(0);

            let headers = self.parse_headers(&data, start_height);

            for header in headers {
                last_height = Some(header.height);
                all_headers.push(header);
            }
        }

        // Sort by height
        all_headers.sort_by_key(|h| h.height);

        let done = true;

        info!("WOC bulk sync fetched {} headers", all_headers.len());

        Ok(BulkSyncResult {
            live_headers: all_headers,
            done,
        })
    }

    async fn fetch_headers(&self) -> WalletResult<BulkSyncResult> {
        info!("Fetching headers from WOC");

        // Get file links
        let links = self.get_header_byte_file_links().await?;

        let mut all_headers = Vec::new();
        let mut last_height: Option<u32> = None;

        // Process each relevant file link
        for link in &links {
            // Skip if no range info and not latest
            if link.range.is_none() && !link.is_latest {
                continue;
            }

            // Download and parse
            let data = self.download_header_file(link).await?;

            let start_height = link
                .range
                .as_ref()
                .map(|r| r.low)
                .or(last_height.map(|h| h + 1))
                .unwrap_or(0);

            let headers = self.parse_headers(&data, start_height);

            for header in headers {
                last_height = Some(header.height);
                all_headers.push(header);
            }
        }

        // Sort by height
        all_headers.sort_by_key(|h| h.height);

        info!("Fetched {} headers from WOC", all_headers.len());

        Ok(BulkSyncResult {
            live_headers: all_headers,
            done: true,
        })
    }

    async fn set_storage(&self, storage: Arc<dyn ChaintracksStorageIngest>) -> WalletResult<()> {
        let mut guard = self.storage.write().await;
        *guard = Some(storage);
        Ok(())
    }

    async fn shutdown(&self) -> WalletResult<()> {
        info!("WOC bulk ingestor shutting down");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_options_creation() {
        let mainnet = BulkWocOptions::mainnet();
        assert_eq!(mainnet.chain, Chain::Main);

        let testnet = BulkWocOptions::testnet();
        assert_eq!(testnet.chain, Chain::Test);

        let with_key = BulkWocOptions::mainnet().with_api_key("test-key");
        assert_eq!(with_key.api_key, Some("test-key".to_string()));
    }

    #[test]
    fn test_api_url() {
        let mainnet = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();
        assert!(mainnet.api_url().contains("main"));

        let testnet = BulkWocIngestor::new(BulkWocOptions::testnet()).unwrap();
        assert!(testnet.api_url().contains("test"));
    }

    #[test]
    fn test_parse_file_link() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        // Test normal file
        let link = ingestor.parse_file_link("https://example.com/headers/0_99999_headers.bin");
        assert!(link.is_some());
        let link = link.unwrap();
        assert!(!link.is_latest);
        assert!(link.range.is_some());
        assert_eq!(link.range.as_ref().unwrap().low, 0);
        assert_eq!(link.range.as_ref().unwrap().high, 99999);

        // Test latest file
        let latest = ingestor.parse_file_link("https://example.com/headers/latest");
        assert!(latest.is_some());
        assert!(latest.unwrap().is_latest);
    }

    #[test]
    fn test_convert_woc_header() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        let woc = WocHeaderResponse {
            hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            confirmations: 1000,
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
            previousblockhash: None,
            nextblockhash: Some("some_hash".to_string()),
        };

        let header = ingestor.convert_woc_header(&woc);
        assert_eq!(header.height, 0);
        assert_eq!(header.nonce, 2083236893);
        assert_eq!(header.previous_hash, "0".repeat(64));
    }

    #[test]
    fn test_woc_api_url_constants() {
        assert!(WOC_API_URL_MAIN.contains("whatsonchain.com"));
        assert!(WOC_API_URL_MAIN.contains("main"));
        assert!(WOC_API_URL_TEST.contains("whatsonchain.com"));
        assert!(WOC_API_URL_TEST.contains("test"));
    }

    #[test]
    fn test_options_defaults() {
        let opts = BulkWocOptions::default();
        assert_eq!(opts.chain, Chain::Main);
        assert_eq!(opts.timeout_secs, 30);
        assert!(opts.enable_cache);
        assert_eq!(opts.chain_info_ttl_ms, 5000);
        assert_eq!(opts.idle_wait_ms, 5000);
        assert!(opts.api_key.is_none());
    }

    #[test]
    fn test_woc_chain_info_deserialization() {
        let json = r#"{
            "chain": "main",
            "blocks": 800000,
            "headers": 800000,
            "bestblockhash": "000000000000000001234567890abcdef",
            "difficulty": 1234567890.5,
            "mediantime": 1700000000
        }"#;

        let info: WocChainInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.chain, "main");
        assert_eq!(info.blocks, 800000);
        assert_eq!(info.best_block_hash, "000000000000000001234567890abcdef");
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
            "previousblockhash": null,
            "nextblockhash": "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048"
        }"#;

        let response: WocHeaderResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.height, 0);
        assert_eq!(response.nonce, 2083236893);
        assert!(response.previousblockhash.is_none());
        assert!(response.nextblockhash.is_some());
    }

    #[test]
    fn test_file_link_range_parsing() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        // Valid range format
        let link = ingestor.parse_file_link("https://cdn.example.com/100000_199999_headers.bin");
        assert!(link.is_some());
        let link = link.unwrap();
        assert_eq!(link.range.as_ref().unwrap().low, 100000);
        assert_eq!(link.range.as_ref().unwrap().high, 199999);

        // Invalid URL
        let invalid = ingestor.parse_file_link("not a valid url");
        assert!(invalid.is_none());
    }

    #[test]
    fn test_deserialize_header() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        // Genesis block header bytes (80 bytes)
        let genesis_hex = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
        let genesis_bytes = hex::decode(genesis_hex).unwrap();

        let header = ingestor.deserialize_header(&genesis_bytes, 0);

        assert_eq!(header.height, 0);
        assert_eq!(header.version, 1);
        assert_eq!(header.nonce, 2083236893);
        assert_eq!(header.bits, 0x1d00ffff);
    }

    #[test]
    fn test_compute_block_hash() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        // Genesis block header
        let genesis_hex = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
        let genesis_bytes = hex::decode(genesis_hex).unwrap();

        let hash = ingestor.compute_block_hash(&genesis_bytes);

        // Genesis block hash (reversed for display)
        assert_eq!(
            hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }

    #[tokio::test]
    async fn test_ingestor_creation() {
        let mainnet = BulkWocIngestor::mainnet();
        assert!(mainnet.is_ok());

        let testnet = BulkWocIngestor::testnet();
        assert!(testnet.is_ok());
    }

    #[test]
    fn test_convert_woc_header_with_previous() {
        let ingestor = BulkWocIngestor::new(BulkWocOptions::mainnet()).unwrap();

        let woc = WocHeaderResponse {
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
            previousblockhash: Some(
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
            ),
            nextblockhash: Some(
                "000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd".to_string(),
            ),
        };

        let header = ingestor.convert_woc_header(&woc);
        assert_eq!(header.height, 1);
        assert_eq!(
            header.previous_hash,
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
    }
}
