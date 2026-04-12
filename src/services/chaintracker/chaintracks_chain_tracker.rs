//! ChaintracksChainTracker implementing bsv-sdk ChainTracker trait.
//!
//! Ported from wallet-toolbox/src/services/chaintracker/ChaintracksChainTracker.ts.
//! Caches merkle roots by block height to avoid redundant API calls.
//!
//! Supports two backends:
//! - Remote: delegates to a `ChaintracksServiceClient` over HTTP.
//! - Local: delegates to an in-process `Chaintracks` instance.

use std::collections::HashMap;
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
pub struct ChaintracksChainTracker {
    backend: ChaintracksBackend,
    root_cache: Mutex<HashMap<u32, String>>,
}

impl ChaintracksChainTracker {
    /// Create a new ChaintracksChainTracker wrapping the given remote service client.
    ///
    /// This preserves the existing constructor signature; callers are unaffected.
    pub fn new(service_client: ChaintracksServiceClient) -> Self {
        Self {
            backend: ChaintracksBackend::Remote(service_client),
            root_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Create a ChaintracksChainTracker backed by a local Chaintracks instance.
    pub fn with_local(chaintracks: Arc<Chaintracks>) -> Self {
        Self {
            backend: ChaintracksBackend::Local(chaintracks),
            root_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Delegate to the backend: get a block header by hash.
    pub async fn hash_to_header(&self, hash: &str) -> WalletResult<BlockHeader> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => client
                .get_header_for_block_hash(hash)
                .await?
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for block hash {}", hash))
                }),
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_block_hash(hash)
                .await?
                .map(BlockHeader::from)
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for block hash {}", hash))
                }),
        }
    }

    /// Delegate to the backend: get a block header by height.
    pub async fn get_header_for_height(&self, height: u32) -> WalletResult<BlockHeader> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => {
                client.get_header_for_height(height).await?.ok_or_else(|| {
                    WalletError::Internal(format!("No header found for height {}", height))
                })
            }
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_height(height)
                .await?
                .map(BlockHeader::from)
                .ok_or_else(|| {
                    WalletError::Internal(format!("No header found for height {}", height))
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
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e))),
            ChaintracksBackend::Local(ct) => ct
                .find_header_for_height(height)
                .await
                .map(|opt| opt.map(BlockHeader::from))
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e))),
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
            if let Some(cached_root) = cache.get(&height) {
                return Ok(cached_root == root);
            }
        }

        // Cache miss -- fetch from backend
        let header = self.fetch_header_for_height(height).await?;

        match header {
            None => Ok(false),
            Some(h) => {
                // Store in cache
                let mut cache = self.root_cache.lock().await;
                cache.insert(height, h.merkle_root.clone());
                Ok(h.merkle_root == root)
            }
        }
    }

    async fn current_height(&self) -> Result<u32, TransactionError> {
        match &self.backend {
            ChaintracksBackend::Remote(client) => client
                .get_present_height()
                .await
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e))),
            ChaintracksBackend::Local(ct) => ct
                .current_height()
                .await
                .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e))),
        }
    }
}
