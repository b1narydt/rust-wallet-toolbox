//! ChaintracksChainTracker implementing bsv-sdk ChainTracker trait.
//!
//! Ported from wallet-toolbox/src/services/chaintracker/ChaintracksChainTracker.ts.
//! Caches merkle roots by block height to avoid redundant API calls.
//!
//! Supports two backends:
//! - Remote: delegates to a `ChaintracksServiceClient` over HTTP.
//! - Local: delegates to an in-process `Chaintracks` instance.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::error::TransactionError;
use tokio::sync::Mutex;

use crate::chaintracks::{Chaintracks, ChaintracksClient as LocalChaintracksClient};
use crate::error::{WalletError, WalletResult};
use crate::services::types::BlockHeader;

use super::chaintracks_service_client::ChaintracksServiceClient;

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum ChaintracksBackend {
    Remote(ChaintracksServiceClient),
    Local(Arc<Chaintracks>),
}

// ---------------------------------------------------------------------------
// BlockHeader conversion
// ---------------------------------------------------------------------------

/// Convert the chaintracks-layer BlockHeader into the services-layer BlockHeader.
///
/// Both structs have identical fields; this is a field-by-field copy.
impl From<crate::chaintracks::BlockHeader> for BlockHeader {
    fn from(h: crate::chaintracks::BlockHeader) -> Self {
        BlockHeader {
            version: h.version,
            previous_hash: h.previous_hash,
            merkle_root: h.merkle_root,
            time: h.time,
            bits: h.bits,
            nonce: h.nonce,
            height: h.height,
            hash: h.hash,
        }
    }
}

// ---------------------------------------------------------------------------
// ChaintracksChainTracker
// ---------------------------------------------------------------------------

/// Chain tracker backed by the Chaintracks service (remote) or a local instance.
///
/// Implements the bsv-sdk `ChainTracker` trait for merkle root validation.
/// Caches previously looked-up merkle roots by block height to avoid
/// redundant network or storage calls.
/// Cloning shares the root cache: every clone reads and fills the same map.
/// `Services::get_chain_tracker` hands out a clone per call, and a cache that
/// did not survive that call would re-fetch every header root on every
/// `internalize_action` — the cache exists precisely because one action
/// validates many bumps at the same heights.
#[derive(Clone)]
pub struct ChaintracksChainTracker {
    backend: ChaintracksBackend,
    root_cache: Arc<Mutex<RootCache>>,
}

/// Chaintracks' default storage processes the newest 400 blocks with full
/// reorg support. Roots in that window are read from Chaintracks on every
/// validation so a replacement active-chain header is observed immediately.
const REORG_SAFE_DEPTH: u32 = 400;

/// A few thousand settled roots cover repeated wallet proof validation while
/// keeping the process-wide cache footprint independent of chain age.
const ROOT_CACHE_CAPACITY: usize = 4096;

#[derive(Default)]
struct RootCache {
    roots: HashMap<u32, String>,
    insertion_order: VecDeque<u32>,
}

impl RootCache {
    fn get(&self, height: u32) -> Option<&str> {
        self.roots.get(&height).map(String::as_str)
    }

    fn insert(&mut self, height: u32, merkle_root: String) {
        if let Some(existing) = self.roots.get_mut(&height) {
            *existing = merkle_root;
            return;
        }

        while self.roots.len() >= ROOT_CACHE_CAPACITY {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.roots.remove(&oldest);
            }
        }

        self.roots.insert(height, merkle_root);
        self.insertion_order.push_back(height);
    }
}

impl ChaintracksChainTracker {
    /// Create a new ChaintracksChainTracker wrapping the given remote service client.
    ///
    /// This preserves the existing constructor signature; callers are unaffected.
    pub fn new(service_client: ChaintracksServiceClient) -> Self {
        Self {
            backend: ChaintracksBackend::Remote(service_client),
            root_cache: Arc::new(Mutex::new(RootCache::default())),
        }
    }

    /// Create a ChaintracksChainTracker backed by a local Chaintracks instance.
    pub fn with_local(chaintracks: Arc<Chaintracks>) -> Self {
        Self {
            backend: ChaintracksBackend::Local(chaintracks),
            root_cache: Arc::new(Mutex::new(RootCache::default())),
        }
    }

    /// Delegate to the backend: get a block header by hash.
    pub async fn hash_to_header(&self, hash: &str) -> WalletResult<BlockHeader> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => client
                .get_header_for_block_hash(hash)
                .await?
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for block hash {hash}"))
                }),
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_block_hash(hash)
                .await?
                .map(BlockHeader::from)
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for block hash {hash}"))
                }),
        }
    }

    /// Delegate to the backend: get a block header by height.
    pub async fn get_header_for_height(&self, height: u32) -> WalletResult<BlockHeader> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => {
                client.get_header_for_height(height).await?.ok_or_else(|| {
                    WalletError::Internal(format!("No header found for height {height}"))
                })
            }
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_height(height)
                .await?
                .map(BlockHeader::from)
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for height {height}"))
                }),
        }
    }

    /// Fetch the header at `height` from whichever backend is configured.
    ///
    /// Returns `None` if the header is not found; propagates other errors.
    async fn fetch_header_for_height(
        &self,
        height: u32,
    ) -> Result<Option<BlockHeader>, TransactionError> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => client
                .get_header_for_height(height)
                .await
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {e}"))),
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_height(height)
                .await
                .map(|opt| opt.map(BlockHeader::from))
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {e}"))),
        }
    }

    /// Insert a value directly into the merkle root cache.
    ///
    /// Primarily for testing purposes.
    pub async fn insert_cache(&self, height: u32, merkle_root: String) {
        let mut cache = self.root_cache.lock().await;
        cache.insert(height, merkle_root);
    }
}

// ---------------------------------------------------------------------------
// ChainTracker trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ChainTracker for ChaintracksChainTracker {
    async fn is_valid_root_for_height(
        &self,
        root: &str,
        height: u32,
    ) -> Result<bool, TransactionError> {
        // Check cache first
        {
            let cache = self.root_cache.lock().await;
            if let Some(cached_root) = cache.get(height) {
                return Ok(cached_root == root);
            }
        }

        // Cache miss -- fetch from backend
        let header = self.fetch_header_for_height(height).await?;

        match header {
            None => Ok(false),
            Some(h) => {
                // Recent active-chain roots remain reorgable in Chaintracks.
                // Settled roots can be shared safely across tracker clones.
                let tip_height = self.current_height().await?;
                if height < tip_height.saturating_sub(REORG_SAFE_DEPTH) {
                    let mut cache = self.root_cache.lock().await;
                    cache.insert(height, h.merkle_root.clone());
                }
                Ok(h.merkle_root == root)
            }
        }
    }

    async fn current_height(&self) -> Result<u32, TransactionError> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => client
                .get_present_height()
                .await
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {e}"))),
            ChaintracksBackend::Local(ct) => ct
                .current_height()
                .await
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn root_cache_is_bounded() {
        let mut cache = RootCache::default();
        for height in 0..=ROOT_CACHE_CAPACITY as u32 {
            cache.insert(height, format!("root-{height}"));
        }

        assert_eq!(cache.roots.len(), ROOT_CACHE_CAPACITY);
        assert!(cache.get(0).is_none());
        assert_eq!(cache.get(ROOT_CACHE_CAPACITY as u32), Some("root-4096"));
    }

    #[tokio::test]
    async fn recent_root_is_refetched_after_reorg() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let reorged = Arc::new(AtomicBool::new(false));
        let header_requests = Arc::new(AtomicUsize::new(0));
        let server_reorged = Arc::clone(&reorged);
        let server_requests = Arc::clone(&header_requests);

        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 2048];
                let length = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..length]);
                let body = if request.contains("/getPresentHeight") {
                    r#"{"status":"success","value":1000}"#.to_string()
                } else {
                    server_requests.fetch_add(1, Ordering::SeqCst);
                    let root = if server_reorged.load(Ordering::SeqCst) {
                        "root-b"
                    } else {
                        "root-a"
                    };
                    format!(
                        r#"{{"status":"success","value":{{"version":1,"previousHash":"prev","merkleRoot":"{root}","time":1,"bits":1,"nonce":1,"height":999,"hash":"hash"}}}}"#
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = ChaintracksServiceClient::new(
            crate::types::Chain::Main,
            Some(&format!("http://{address}")),
            reqwest::Client::new(),
        );
        let tracker = ChaintracksChainTracker::new(client);
        assert!(tracker
            .is_valid_root_for_height("root-a", 999)
            .await
            .unwrap());

        reorged.store(true, Ordering::SeqCst);
        let later_caller = tracker.clone();
        assert!(later_caller
            .is_valid_root_for_height("root-b", 999)
            .await
            .unwrap());
        assert!(!later_caller
            .is_valid_root_for_height("root-a", 999)
            .await
            .unwrap());
        assert_eq!(header_requests.load(Ordering::SeqCst), 3);

        server.abort();
    }
}
