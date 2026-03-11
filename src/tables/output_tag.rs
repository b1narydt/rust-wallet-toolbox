//! OutputTag table struct.
//!
//! Maps to TS `TableOutputTag` in `wallet-toolbox/src/storage/schema/tables/TableOutputTag.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A tag that can be applied to transaction outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct OutputTag {
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
    pub output_tag_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Tag text.
    pub tag: String,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
