//! Wire-format args for getSyncChunk and processSyncChunk.
//!
//! `RequestSyncChunkArgs` matches the TypeScript `RequestSyncChunkArgs` interface
//! exactly, including camelCase JSON field names. This is distinct from the internal
//! `GetSyncChunkArgs<'a>` which uses borrowed data and the `SyncMap` type.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// Wire-format arguments for getSyncChunk / processSyncChunk requests.
///
/// Matches TypeScript `RequestSyncChunkArgs` from WalletStorage.interfaces.ts.
/// Used in the `WalletStorageProvider` trait and by `StorageClient` (Phase 3)
/// for remote calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSyncChunkArgs {
    /// Identity key of the storage being read from.
    pub from_storage_identity_key: String,
    /// Identity key of the storage being synced to.
    pub to_storage_identity_key: String,
    /// The user's identity key.
    pub identity_key: String,
    /// Only return records updated after this timestamp.
    #[serde(
        default,
        with = "crate::serde_datetime::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub since: Option<NaiveDateTime>,
    /// Maximum total rough byte size of returned data.
    pub max_rough_size: i64,
    /// Maximum number of items per entity type.
    pub max_items: i64,
    /// Per-entity row offsets for pagination within a sync window.
    pub offsets: Vec<SyncChunkOffset>,
}

/// A single per-entity offset for sync chunk pagination.
///
/// The `name` field uses camelCase entity names matching the TypeScript
/// wire format (e.g., `"provenTx"`, `"outputBasket"`, `"transaction"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChunkOffset {
    /// Entity type name in camelCase (e.g., "provenTx", "transaction").
    pub name: String,
    /// Row offset to resume pagination from.
    pub offset: i64,
}
