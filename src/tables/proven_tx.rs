//! ProvenTx table struct.
//!
//! Maps to TS `TableProvenTx` in `wallet-toolbox/src/storage/schema/tables/TableProvenTx.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A proven transaction with merkle proof data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct ProvenTx {
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
    pub proven_tx_id: i64,
    /// Transaction ID hex string.
    pub txid: String,
    /// Block height where the transaction was mined.
    pub height: i32,
    /// Index of the transaction within the block.
    #[sqlx(rename = "index")]
    pub index: i32,
    /// Serialized merkle path proof.
    pub merkle_path: Vec<u8>,
    /// Raw serialized transaction bytes.
    pub raw_tx: Vec<u8>,
    /// Hash of the block containing the transaction.
    pub block_hash: String,
    /// Merkle root of the block.
    pub merkle_root: String,
}
