//! SyncState table struct.
//!
//! Maps to TS `TableSyncState` in `wallet-toolbox/src/storage/schema/tables/TableSyncState.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::status::SyncStatus;

/// Synchronization state between wallet storage instances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct SyncState {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    #[serde(
        rename = "created_at",
        alias = "createdAt",
        with = "crate::serde_datetime"
    )]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    #[serde(
        rename = "updated_at",
        alias = "updatedAt",
        with = "crate::serde_datetime"
    )]
    pub updated_at: NaiveDateTime,
    /// Primary key.
    pub sync_state_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Identity key of the remote storage.
    pub storage_identity_key: String,
    /// Name of the remote storage.
    pub storage_name: String,
    /// Current sync status.
    pub status: SyncStatus,
    /// Whether initial sync has been performed.
    pub init: bool,
    /// Reference number for sync protocol.
    pub ref_num: String,
    /// JSON-encoded sync map tracking per-entity sync positions.
    pub sync_map: String,
    /// Timestamp of last successful sync.
    #[serde(default, with = "crate::serde_datetime::option")]
    pub when: Option<NaiveDateTime>,
    /// Net satoshi delta from last sync.
    pub satoshis: Option<i64>,
    /// Error message from the local side, if any.
    pub error_local: Option<String>,
    /// Error message from the remote side, if any.
    pub error_other: Option<String>,
}
