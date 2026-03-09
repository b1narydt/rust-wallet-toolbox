//! Settings table struct.
//!
//! Maps to TS `TableSettings` in `wallet-toolbox/src/storage/schema/tables/TableSettings.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::types::Chain;

/// Storage configuration settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Settings {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Identity key identifying this storage instance.
    pub storage_identity_key: String,
    /// Human-readable name for this storage.
    pub storage_name: String,
    /// BSV network chain (main or test).
    pub chain: Chain,
    /// Database type identifier (e.g., "SQLite").
    pub dbtype: String,
    /// Maximum locking script size to store inline (bytes).
    pub max_output_script: i32,
}
