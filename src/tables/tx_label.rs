//! TxLabel table struct.
//!
//! Maps to TS `TableTxLabel` in `wallet-toolbox/src/storage/schema/tables/TableTxLabel.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A label that can be applied to transactions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct TxLabel {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Primary key.
    pub tx_label_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Label text.
    pub label: String,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
