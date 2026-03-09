//! User table struct.
//!
//! Maps to TS `TableUser` in `wallet-toolbox/src/storage/schema/tables/TableUser.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A wallet user identified by a public identity key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct User {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Primary key.
    pub user_id: i64,
    /// Hex-encoded public identity key.
    pub identity_key: String,
    /// Name of the currently active storage for this user.
    pub active_storage: String,
}
