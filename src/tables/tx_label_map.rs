//! TxLabelMap table struct.
//!
//! Maps to TS `TableTxLabelMap` in `wallet-toolbox/src/storage/schema/tables/TableTxLabelMap.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Maps transaction labels to transactions (many-to-many relationship).
///
/// Note: The TS table name is `tx_labels_map` in the Knex migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct TxLabelMap {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Label foreign key.
    pub tx_label_id: i64,
    /// Transaction foreign key.
    pub transaction_id: i64,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
