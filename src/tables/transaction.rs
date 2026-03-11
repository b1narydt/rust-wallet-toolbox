//! Transaction table struct.
//!
//! Maps to TS `TableTransaction` in `wallet-toolbox/src/storage/schema/tables/TableTransaction.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::status::TransactionStatus;

/// A wallet transaction record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Transaction {
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
    pub transaction_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Associated proven transaction, if the tx is on chain.
    pub proven_tx_id: Option<i64>,
    /// Current processing status.
    pub status: TransactionStatus,
    /// Unique reference string for idempotency.
    pub reference: String,
    /// Whether this transaction sends funds out of the wallet.
    pub is_outgoing: bool,
    /// Net satoshi value (positive for incoming, negative for outgoing).
    pub satoshis: i64,
    /// Human-readable description.
    pub description: String,
    /// Transaction version number.
    pub version: Option<i32>,
    /// Transaction locktime value.
    pub lock_time: Option<i32>,
    /// Transaction ID hex string.
    pub txid: Option<String>,
    /// Input BEEF data for transaction signing context.
    #[serde(rename = "inputBEEF")]
    #[sqlx(rename = "inputBEEF")]
    pub input_beef: Option<Vec<u8>>,
    /// Raw serialized transaction bytes.
    pub raw_tx: Option<Vec<u8>>,
}
