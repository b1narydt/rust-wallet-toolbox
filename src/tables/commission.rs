//! Commission table struct.
//!
//! Maps to TS `TableCommission` in `wallet-toolbox/src/storage/schema/tables/TableCommission.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A commission record for a transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Commission {
    /// When this record was created.
    #[sqlx(rename = "created_at")]
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Primary key.
    pub commission_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Parent transaction foreign key.
    pub transaction_id: i64,
    /// Commission amount in satoshis.
    pub satoshis: i64,
    /// Key offset used for deriving the commission output key.
    pub key_offset: String,
    /// Whether the commission has been redeemed (spent).
    pub is_redeemed: bool,
    /// The locking script for the commission output.
    pub locking_script: Vec<u8>,
}
