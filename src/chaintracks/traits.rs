//! Trait definitions for chaintracks storage, client, and ingestor interfaces.

use async_trait::async_trait;
use std::sync::Arc;

use super::{
    BaseBlockHeader, BlockHeader, ChaintracksInfo, HeightRange, InsertHeaderResult, LiveBlockHeader,
};
use crate::error::WalletResult;
use crate::types::Chain;

// ---------------------------------------------------------------------------
// Callback types
// ---------------------------------------------------------------------------

pub type HeaderCallback = Box<dyn Fn(BlockHeader) + Send + Sync>;
pub type ReorgCallback = Box<dyn Fn(ReorgEvent) + Send + Sync>;

// ---------------------------------------------------------------------------
// Event / result types
// ---------------------------------------------------------------------------

/// Emitted when a blockchain reorganization is detected.
#[derive(Debug, Clone)]
pub struct ReorgEvent {
    /// Depth of the reorganization in blocks.
    pub depth: u32,
    /// The old chain tip before the reorg.
    pub old_tip: BlockHeader,
    /// The new chain tip after the reorg.
    pub new_tip: BlockHeader,
    /// Headers that were deactivated by the reorg.
    pub deactivated_headers: Vec<BlockHeader>,
}

/// Result returned by bulk synchronization operations.
#[derive(Debug, Clone)]
pub struct BulkSyncResult {
    /// Live headers fetched during this sync step.
    pub live_headers: Vec<BlockHeader>,
    /// Whether synchronization is complete.
    pub done: bool,
}

// ---------------------------------------------------------------------------
// ChaintracksOptions
// ---------------------------------------------------------------------------

/// Configuration options for a Chaintracks instance.
#[derive(Debug, Clone)]
pub struct ChaintracksOptions {
    /// The BSV chain (mainnet or testnet).
    pub chain: Chain,
    /// Height threshold below the tip below which headers are considered "live".
    pub live_height_threshold: u32,
    /// Maximum depth of reorg to detect.
    pub reorg_height_threshold: u32,
    /// Recursion limit when adding live headers.
    pub add_live_recursion_limit: u32,
    /// Maximum number of headers to insert in a single batch.
    pub batch_insert_limit: u32,
    /// Number of headers moved per bulk migration chunk.
    pub bulk_migration_chunk_size: u32,
    /// Whether at least one ingestor is required.
    pub require_ingestors: bool,
    /// If true, storage mutations are disallowed.
    pub readonly: bool,
}

impl ChaintracksOptions {
    /// Returns the default configuration for BSV mainnet.
    pub fn default_mainnet() -> Self {
        Self {
            chain: Chain::Main,
            live_height_threshold: 2000,
            reorg_height_threshold: 400,
            add_live_recursion_limit: 36,
            batch_insert_limit: 400,
            bulk_migration_chunk_size: 500,
            require_ingestors: false,
            readonly: false,
        }
    }

    /// Returns the default configuration for BSV testnet.
    pub fn default_testnet() -> Self {
        Self {
            chain: Chain::Test,
            live_height_threshold: 2000,
            reorg_height_threshold: 400,
            add_live_recursion_limit: 36,
            batch_insert_limit: 400,
            bulk_migration_chunk_size: 500,
            require_ingestors: false,
            readonly: false,
        }
    }
}

impl Default for ChaintracksOptions {
    fn default() -> Self {
        Self::default_mainnet()
    }
}

// ---------------------------------------------------------------------------
// Storage traits
// ---------------------------------------------------------------------------

/// Read-only query interface for chaintracks block header storage.
#[async_trait]
pub trait ChaintracksStorageQuery: Send + Sync {
    /// Returns the chain this storage tracks.
    fn chain(&self) -> Chain;

    /// Returns the live height threshold configuration value.
    fn live_height_threshold(&self) -> u32;

    /// Returns the reorg height threshold configuration value.
    fn reorg_height_threshold(&self) -> u32;

    /// Returns the current chain tip as a live header, if available.
    async fn find_chain_tip_header(&self) -> WalletResult<Option<LiveBlockHeader>>;

    /// Returns the block hash of the current chain tip, if available.
    async fn find_chain_tip_hash(&self) -> WalletResult<Option<String>>;

    /// Returns the block header at the given height, if available.
    async fn find_header_for_height(&self, height: u32) -> WalletResult<Option<BlockHeader>>;

    /// Returns the live block header matching the given block hash, if available.
    async fn find_live_header_for_block_hash(
        &self,
        hash: &str,
    ) -> WalletResult<Option<LiveBlockHeader>>;

    /// Returns the live block header matching the given merkle root, if available.
    async fn find_live_header_for_merkle_root(
        &self,
        merkle_root: &str,
    ) -> WalletResult<Option<LiveBlockHeader>>;

    /// Returns raw 80-byte header data for `count` headers starting at `height`.
    async fn get_headers_bytes(&self, height: u32, count: u32) -> WalletResult<Vec<u8>>;

    /// Returns all live block headers.
    async fn get_live_headers(&self) -> WalletResult<Vec<LiveBlockHeader>>;

    /// Returns all available height ranges in storage.
    async fn get_available_height_ranges(&self) -> WalletResult<Vec<HeightRange>>;

    /// Returns the height range currently covered by live headers, if any.
    async fn find_live_height_range(&self) -> WalletResult<Option<HeightRange>>;

    /// Returns the most recent common ancestor of two live headers, if any.
    async fn find_common_ancestor(
        &self,
        h1: &LiveBlockHeader,
        h2: &LiveBlockHeader,
    ) -> WalletResult<Option<LiveBlockHeader>>;

    /// Returns the reorg depth required to make `new_header` the active tip.
    async fn find_reorg_depth(&self, new_header: &LiveBlockHeader) -> WalletResult<u32>;
}

/// Write interface for ingesting block headers into storage.
#[async_trait]
pub trait ChaintracksStorageIngest: ChaintracksStorageQuery {
    /// Inserts a live block header, returning metadata about the result.
    async fn insert_header(&self, header: LiveBlockHeader) -> WalletResult<InsertHeaderResult>;

    /// Removes live block headers that are older than the active tip height allows.
    async fn prune_live_block_headers(&self, active_tip_height: u32) -> WalletResult<u32>;

    /// Migrates the oldest `count` live headers to bulk (archived) storage.
    async fn migrate_live_to_bulk(&self, count: u32) -> WalletResult<u32>;

    /// Deletes live block headers at or below `max_height`.
    async fn delete_older_live_block_headers(&self, max_height: u32) -> WalletResult<u32>;

    /// Marks the storage as available for use.
    async fn make_available(&self) -> WalletResult<()>;

    /// Applies any pending schema or data migrations.
    async fn migrate_latest(&self) -> WalletResult<()>;

    /// Drops all data from the storage, leaving the schema intact.
    async fn drop_all_data(&self) -> WalletResult<()>;

    /// Destroys the storage, releasing all resources.
    async fn destroy(&self) -> WalletResult<()>;
}

/// Full storage interface combining query, ingest, and lifecycle operations.
#[async_trait]
pub trait ChaintracksStorage: ChaintracksStorageIngest {
    /// Returns a human-readable identifier for the storage backend type.
    fn storage_type(&self) -> &str;

    /// Returns true if the storage is currently available for operations.
    async fn is_available(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Client / Management traits
// ---------------------------------------------------------------------------

/// Client interface for interacting with a chaintracks instance.
#[async_trait]
pub trait ChaintracksClient: Send + Sync {
    /// Returns the chain this client tracks.
    async fn get_chain(&self) -> Chain;

    /// Returns summary information about this chaintracks instance.
    async fn get_info(&self) -> WalletResult<ChaintracksInfo>;

    /// Returns the height currently being processed by ingestors, if known.
    async fn get_present_height(&self) -> WalletResult<u32>;

    /// Returns true if this client is actively listening for new blocks.
    async fn is_listening(&self) -> bool;

    /// Returns true if this client is synchronized with the network tip.
    async fn is_synchronized(&self) -> bool;

    /// Returns the current verified chain tip height.
    async fn current_height(&self) -> WalletResult<u32>;

    /// Returns the block header at the given height, if available.
    async fn find_header_for_height(&self, height: u32) -> WalletResult<Option<BlockHeader>>;

    /// Returns the block header matching the given block hash, if available.
    async fn find_header_for_block_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>>;

    /// Returns the current chain tip header, if available.
    async fn find_chain_tip_header(&self) -> WalletResult<Option<BlockHeader>>;

    /// Returns the block hash of the current chain tip, if available.
    async fn find_chain_tip_hash(&self) -> WalletResult<Option<String>>;

    /// Returns true if `root` is a valid merkle root for the block at `height`.
    async fn is_valid_root_for_height(&self, root: &str, height: u32) -> WalletResult<bool>;

    /// Returns serialized header bytes as a hex string for `count` headers starting at `height`.
    async fn get_headers(&self, height: u32, count: u32) -> WalletResult<String>;

    /// Submits a new block header for ingestion.
    async fn add_header(&self, header: BaseBlockHeader) -> WalletResult<()>;

    /// Starts listening for new blocks from live ingestors.
    async fn start_listening(&self) -> WalletResult<()>;

    /// Waits until the client is listening, or returns an error if not supported.
    async fn listening(&self) -> WalletResult<()>;

    /// Subscribes to new block header events; returns a subscription ID.
    async fn subscribe_headers(&self, callback: HeaderCallback) -> WalletResult<String>;

    /// Subscribes to reorganization events; returns a subscription ID.
    async fn subscribe_reorgs(&self, callback: ReorgCallback) -> WalletResult<String>;

    /// Unsubscribes the given subscription ID; returns true if it was found.
    async fn unsubscribe(&self, subscription_id: &str) -> WalletResult<bool>;
}

/// Management interface extending the client with administrative operations.
#[async_trait]
pub trait ChaintracksManagement: ChaintracksClient {
    /// Destroys the chaintracks instance and all underlying resources.
    async fn destroy(&self) -> WalletResult<()>;

    /// Validates the integrity of stored chain data.
    async fn validate(&self) -> WalletResult<bool>;

    /// Exports bulk headers to files in the given folder.
    ///
    /// - `folder` — destination directory path.
    /// - `headers_per_file` — maximum headers per output file (default chosen by implementation).
    /// - `max_height` — if provided, only export headers at or below this height.
    async fn export_bulk_headers(
        &self,
        folder: &str,
        headers_per_file: Option<u32>,
        max_height: Option<u32>,
    ) -> WalletResult<()>;
}

// ---------------------------------------------------------------------------
// Ingestor traits
// ---------------------------------------------------------------------------

/// Bulk (historical) ingestor that fetches headers from a remote data source.
#[async_trait]
pub trait BulkIngestor: Send + Sync {
    /// Returns the highest block height available from the remote source.
    async fn get_present_height(&self) -> WalletResult<u32>;

    /// Performs a full synchronization pass, returning a summary of what was synced.
    async fn synchronize(&self) -> WalletResult<BulkSyncResult>;

    /// Fetches the next batch of headers from the remote source.
    async fn fetch_headers(&self) -> WalletResult<BulkSyncResult>;

    /// Attaches a storage backend to this ingestor.
    async fn set_storage(&self, storage: Arc<dyn ChaintracksStorageIngest>) -> WalletResult<()>;

    /// Shuts down the ingestor, releasing any held resources.
    async fn shutdown(&self) -> WalletResult<()>;
}

/// Live ingestor that subscribes to new block headers as they are produced.
#[async_trait]
pub trait LiveIngestor: Send + Sync {
    /// Returns the block header matching the given hash from the live source, if available.
    async fn get_header_by_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>>;

    /// Starts listening for new live block headers.
    ///
    /// `live_headers` is a mutable snapshot of the currently known live headers,
    /// which the ingestor may use for initial state.
    async fn start_listening(&self, live_headers: &mut Vec<BlockHeader>) -> WalletResult<()>;

    /// Stops listening for new live block headers.
    async fn stop_listening(&self) -> WalletResult<()>;

    /// Attaches a storage backend to this ingestor.
    async fn set_storage(&self, storage: Arc<dyn ChaintracksStorageIngest>) -> WalletResult<()>;

    /// Shuts down the ingestor, releasing any held resources.
    async fn shutdown(&self) -> WalletResult<()>;
}
