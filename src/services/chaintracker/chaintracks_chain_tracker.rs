//! ChaintracksChainTracker implementing bsv-sdk ChainTracker trait.
//!
//! Ported from wallet-toolbox/src/services/chaintracker/ChaintracksChainTracker.ts.
//! Caches merkle roots by block height to avoid redundant API calls.

use std::collections::HashMap;

use async_trait::async_trait;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::error::TransactionError;
use tokio::sync::Mutex;

use crate::error::{WalletError, WalletResult};
use crate::services::types::BlockHeader;

use super::chaintracks_service_client::ChaintracksServiceClient;

/// Chain tracker backed by the Chaintracks service.
///
/// Implements the bsv-sdk `ChainTracker` trait for merkle root validation.
/// Caches previously looked-up merkle roots by block height to avoid
/// redundant HTTP calls.
pub struct ChaintracksChainTracker {
    service_client: ChaintracksServiceClient,
    root_cache: Mutex<HashMap<u32, String>>,
}

impl ChaintracksChainTracker {
    /// Create a new ChaintracksChainTracker wrapping the given service client.
    pub fn new(service_client: ChaintracksServiceClient) -> Self {
        Self {
            service_client,
            root_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Delegate to service client: get a block header by hash.
    pub async fn hash_to_header(&self, hash: &str) -> WalletResult<BlockHeader> {
        self.service_client
            .get_header_for_block_hash(hash)
            .await?
            .ok_or_else(|| {
                WalletError::Internal(format!("No header found for block hash {}", hash))
            })
    }

    /// Delegate to service client: get a block header by height.
    pub async fn get_header_for_height(&self, height: u32) -> WalletResult<BlockHeader> {
        self.service_client
            .get_header_for_height(height)
            .await?
            .ok_or_else(|| WalletError::Internal(format!("No header found for height {}", height)))
    }

    /// Insert a value directly into the merkle root cache.
    ///
    /// Primarily for testing purposes.
    pub async fn insert_cache(&self, height: u32, merkle_root: String) {
        let mut cache = self.root_cache.lock().await;
        cache.insert(height, merkle_root);
    }
}

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

        // Cache miss -- fetch from service
        let header = self
            .service_client
            .get_header_for_height(height)
            .await
            .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e)))?;

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
        self.service_client
            .get_present_height()
            .await
            .map_err(|e| TransactionError::InvalidFormat(format!("ChainTracker error: {}", e)))
    }
}
