//! Polling-based Live Header Ingestor
//!
//! Polls WhatsOnChain API for new block headers at regular intervals.
//! Based on TypeScript: `wallet-toolbox/src/services/chaintracker/chaintracks/Ingest/LiveIngestorWhatsOnChainPoll.ts`

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

use super::{woc_header_to_block_header, WocGetHeadersHeader, WOC_API_URL_MAIN, WOC_API_URL_TEST};
use crate::chaintracks::{BlockHeader, ChaintracksStorageIngest, LiveBlockHeader, LiveIngestor};
use crate::error::{WalletError, WalletResult};
use crate::types::Chain;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options for polling live ingestor
#[derive(Debug, Clone)]
pub struct LivePollingOptions {
    /// Chain to monitor
    pub chain: Chain,
    /// API key (optional)
    pub api_key: Option<String>,
    /// Poll interval in seconds
    pub poll_interval_secs: u64,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// User agent for requests
    pub user_agent: String,
    /// Idle wait time before considering connection stale (ms)
    pub idle_wait_ms: u64,
}

impl Default for LivePollingOptions {
    fn default() -> Self {
        LivePollingOptions {
            chain: Chain::Main,
            api_key: None,
            poll_interval_secs: 60, // Check every minute
            timeout_secs: 30,
            user_agent: "BsvWalletToolbox/1.0".to_string(),
            idle_wait_ms: 100_000,
        }
    }
}

impl LivePollingOptions {
    /// Create options for mainnet
    pub fn mainnet() -> Self {
        Self::default()
    }

    /// Create options for testnet
    pub fn testnet() -> Self {
        LivePollingOptions {
            chain: Chain::Test,
            ..Default::default()
        }
    }

    /// Set poll interval in seconds
    pub fn with_poll_interval(mut self, secs: u64) -> Self {
        self.poll_interval_secs = secs;
        self
    }

    /// Set API key
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Ingestor struct
// ---------------------------------------------------------------------------

/// Polling-based live header ingestor
///
/// Periodically polls the WhatsOnChain API to detect new blocks.
/// Simple and reliable, suitable for most use cases.
pub struct LivePollingIngestor {
    options: LivePollingOptions,
    client: reqwest::Client,
    storage: Arc<RwLock<Option<Arc<dyn ChaintracksStorageIngest>>>>,

    /// Whether the ingestor is currently running
    running: Arc<AtomicBool>,

    /// Broadcast channel for new headers
    sender: broadcast::Sender<LiveBlockHeader>,

    /// Last seen headers (for detecting new ones)
    last_headers: Arc<RwLock<Vec<WocGetHeadersHeader>>>,
}

impl LivePollingIngestor {
    /// Create a new polling live ingestor
    pub fn new(options: LivePollingOptions) -> WalletResult<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(options.timeout_secs))
            .user_agent(&options.user_agent);

        if let Some(ref key) = options.api_key {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "woc-api-key",
                reqwest::header::HeaderValue::from_str(key)
                    .map_err(|_| WalletError::Internal("Invalid API key".to_string()))?,
            );
            builder = builder.default_headers(headers);
        }

        let client = builder
            .build()
            .map_err(|e| WalletError::Internal(e.to_string()))?;
        let (sender, _) = broadcast::channel(100);

        Ok(LivePollingIngestor {
            options,
            client,
            storage: Arc::new(RwLock::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            sender,
            last_headers: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Create a default mainnet ingestor
    pub fn mainnet() -> WalletResult<Self> {
        Self::new(LivePollingOptions::mainnet())
    }

    /// Create a default testnet ingestor
    pub fn testnet() -> WalletResult<Self> {
        Self::new(LivePollingOptions::testnet())
    }

    /// Get base API URL for the configured chain
    fn api_url(&self) -> &'static str {
        match self.options.chain {
            Chain::Main => WOC_API_URL_MAIN,
            Chain::Test => WOC_API_URL_TEST,
        }
    }

    /// Fetch recent headers from WOC (last ~10 blocks)
    async fn fetch_recent_headers(&self) -> WalletResult<Vec<WocGetHeadersHeader>> {
        let url = format!("{}/block/headers", self.api_url());
        debug!("Polling for recent headers: {}", url);

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

        let headers: Vec<WocGetHeadersHeader> = response
            .json()
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;
        Ok(headers)
    }

    /// Fetch a specific header by hash
    async fn fetch_header_by_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>> {
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

        let woc_header: WocGetHeadersHeader = response
            .json()
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;
        Ok(Some(woc_header_to_block_header(&woc_header)))
    }

    /// Run the polling loop
    async fn polling_loop(self: Arc<Self>, live_headers: Arc<RwLock<Vec<BlockHeader>>>) {
        info!(
            "Starting polling loop with interval {} seconds",
            self.options.poll_interval_secs
        );

        while self.running.load(Ordering::SeqCst) {
            match self.poll_once(&live_headers).await {
                Ok(count) => {
                    if count > 0 {
                        debug!("Poll found {} new headers", count);
                    }
                }
                Err(e) => {
                    warn!("Poll error: {}", e);
                }
            }

            // Wait before next poll, checking stop condition periodically
            let wait_secs = self.options.poll_interval_secs;
            for _ in 0..wait_secs {
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }

        info!("Polling loop stopped");
    }

    /// Perform a single poll
    async fn poll_once(&self, live_headers: &Arc<RwLock<Vec<BlockHeader>>>) -> WalletResult<usize> {
        let headers = self.fetch_recent_headers().await?;

        // Find new headers not in last batch
        let last = self.last_headers.read().await;
        let new_headers: Vec<WocGetHeadersHeader> = headers
            .iter()
            .filter(|h| !last.iter().any(|lh| lh.hash == h.hash))
            .cloned()
            .collect();
        drop(last);

        let count = new_headers.len();

        if count > 0 {
            let mut live = live_headers.write().await;

            for woc_header in &new_headers {
                let header = woc_header_to_block_header(woc_header);
                info!(
                    "New block detected: height={}, hash={}",
                    header.height,
                    &header.hash[..16]
                );

                // Add to live headers (newest first)
                live.insert(0, header.clone());

                // Broadcast to subscribers
                let live_header = block_header_to_live_header(header);
                let _ = self.sender.send(live_header);
            }
        }

        // Update last headers cache
        *self.last_headers.write().await = headers;

        Ok(count)
    }

    /// Subscribe to new header notifications
    pub fn subscribe(&self) -> broadcast::Receiver<LiveBlockHeader> {
        self.sender.subscribe()
    }

    /// Check if currently running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// LiveIngestor trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl LiveIngestor for LivePollingIngestor {
    async fn get_header_by_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>> {
        self.fetch_header_by_hash(hash).await
    }

    async fn start_listening(&self, live_headers: &mut Vec<BlockHeader>) -> WalletResult<()> {
        if self.running.load(Ordering::SeqCst) {
            warn!("Polling ingestor already running");
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);

        // Wrap the existing headers in Arc<RwLock<>> for the polling loop
        let headers_arc = Arc::new(RwLock::new(live_headers.clone()));

        // Clone self into Arc for the spawned task
        let self_arc = Arc::new(LivePollingIngestor {
            options: self.options.clone(),
            client: self.client.clone(),
            storage: self.storage.clone(),
            running: self.running.clone(),
            sender: self.sender.clone(),
            last_headers: self.last_headers.clone(),
        });

        let headers_clone = headers_arc.clone();

        // Spawn the polling loop
        tokio::spawn(async move {
            self_arc.polling_loop(headers_clone).await;
        });

        // Wait for initial poll
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Copy any new headers back
        let updated = headers_arc.read().await;
        live_headers.clear();
        live_headers.extend(updated.iter().cloned());

        Ok(())
    }

    async fn stop_listening(&self) -> WalletResult<()> {
        info!("Stopping polling ingestor");
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn set_storage(&self, storage: Arc<dyn ChaintracksStorageIngest>) -> WalletResult<()> {
        *self.storage.write().await = Some(storage);
        Ok(())
    }

    async fn shutdown(&self) -> WalletResult<()> {
        self.stop_listening().await
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Convert BlockHeader to LiveBlockHeader
fn block_header_to_live_header(header: BlockHeader) -> LiveBlockHeader {
    LiveBlockHeader {
        version: header.version,
        previous_hash: header.previous_hash,
        merkle_root: header.merkle_root,
        time: header.time,
        bits: header.bits,
        nonce: header.nonce,
        height: header.height,
        hash: header.hash,
        chain_work: "0".repeat(64),
        is_chain_tip: true,
        is_active: true,
        header_id: None,
        previous_header_id: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_options_creation() {
        let mainnet = LivePollingOptions::mainnet();
        assert_eq!(mainnet.chain, Chain::Main);
        assert_eq!(mainnet.poll_interval_secs, 60);

        let testnet = LivePollingOptions::testnet();
        assert_eq!(testnet.chain, Chain::Test);

        let custom = LivePollingOptions::mainnet()
            .with_poll_interval(30)
            .with_api_key("test-key");
        assert_eq!(custom.poll_interval_secs, 30);
        assert_eq!(custom.api_key, Some("test-key".to_string()));
    }

    #[test]
    fn test_woc_header_conversion() {
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
            next_block_hash: Some(
                "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048".to_string(),
            ),
            n_tx: 1,
            num_tx: 1,
        };

        let header = woc_header_to_block_header(&woc);
        assert_eq!(header.height, 0);
        assert_eq!(header.nonce, 2083236893);
        assert_eq!(header.previous_hash, "0".repeat(64));
        assert_eq!(header.bits, 0x1d00ffff);
    }

    #[test]
    fn test_api_url() {
        let mainnet = LivePollingIngestor::new(LivePollingOptions::mainnet()).unwrap();
        assert!(mainnet.api_url().contains("main"));

        let testnet = LivePollingIngestor::new(LivePollingOptions::testnet()).unwrap();
        assert!(testnet.api_url().contains("test"));
    }

    #[tokio::test]
    async fn test_ingestor_lifecycle() {
        let ingestor = LivePollingIngestor::new(LivePollingOptions::mainnet()).unwrap();

        assert!(!ingestor.is_running());

        // Don't actually start listening in unit tests (would make network calls)
        // Just verify the state management works
        ingestor.stop_listening().await.unwrap();
        assert!(!ingestor.is_running());
    }

    #[test]
    fn test_options_defaults() {
        let opts = LivePollingOptions::default();
        assert_eq!(opts.chain, Chain::Main);
        assert_eq!(opts.poll_interval_secs, 60);
        assert_eq!(opts.timeout_secs, 30);
        assert_eq!(opts.idle_wait_ms, 100_000);
        assert!(opts.api_key.is_none());
    }

    #[test]
    fn test_woc_api_url_constants() {
        assert!(WOC_API_URL_MAIN.contains("whatsonchain.com"));
        assert!(WOC_API_URL_MAIN.contains("/main"));
        assert!(WOC_API_URL_TEST.contains("whatsonchain.com"));
        assert!(WOC_API_URL_TEST.contains("/test"));
    }

    #[test]
    fn test_woc_header_with_previous() {
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
    fn test_woc_header_deserialization() {
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

        let header: WocGetHeadersHeader = serde_json::from_str(json).unwrap();
        assert_eq!(header.height, 0);
        assert_eq!(header.nonce, 2083236893);
        assert!(header.previous_block_hash.is_none());
    }

    #[test]
    fn test_block_header_to_live_header() {
        let header = BlockHeader {
            version: 1,
            previous_hash: "0".repeat(64),
            merkle_root: "abc".repeat(21) + "a",
            time: 1231006505,
            bits: 0x1d00ffff,
            nonce: 2083236893,
            height: 0,
            hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
        };

        let live = block_header_to_live_header(header.clone());
        assert_eq!(live.height, header.height);
        assert_eq!(live.hash, header.hash);
        assert!(live.is_chain_tip);
        assert!(live.is_active);
        assert!(live.header_id.is_none());
        assert!(live.previous_header_id.is_none());
    }

    #[tokio::test]
    async fn test_ingestor_subscribe() {
        let ingestor = LivePollingIngestor::new(LivePollingOptions::mainnet()).unwrap();
        let _receiver = ingestor.subscribe();
        // Verify subscription works without panicking
    }

    #[test]
    fn test_mainnet_testnet_creation() {
        let mainnet = LivePollingIngestor::mainnet();
        assert!(mainnet.is_ok());

        let testnet = LivePollingIngestor::testnet();
        assert!(testnet.is_ok());
    }
}
