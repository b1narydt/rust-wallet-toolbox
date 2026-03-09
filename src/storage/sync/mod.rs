//! Sync protocol implementation for incremental state synchronization between storage providers.
//!
//! This module implements the getSyncChunk / processSyncChunk protocol that enables
//! active/backup storage synchronization. It includes:
//! - `SyncChunk`, `SyncMap`, `EntitySyncMap` types
//! - Entity merge logic for convergent merging of two copies of the same entity
//! - `get_sync_chunk` for extracting changed entities since a given timestamp
//! - `process_sync_chunk` for applying incoming entities with ID remapping

pub mod get_sync_chunk;
pub mod merge;
pub mod process_sync_chunk;
pub mod sync_map;

pub use get_sync_chunk::*;
pub use merge::*;
pub use process_sync_chunk::*;
pub use sync_map::*;
