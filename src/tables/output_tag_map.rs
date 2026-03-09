//! OutputTagMap table struct.
//!
//! Maps to TS `TableOutputTagMap` in `wallet-toolbox/src/storage/schema/tables/TableOutputTagMap.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Maps output tags to outputs (many-to-many relationship).
///
/// Note: The TS table name is `output_tags_map` in the Knex migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct OutputTagMap {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Tag foreign key.
    pub output_tag_id: i64,
    /// Output foreign key.
    pub output_id: i64,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
