//! ProvenTxReq table struct.
//!
//! Maps to TS `TableProvenTxReq` in `wallet-toolbox/src/storage/schema/tables/TableProvenTxReq.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::status::ProvenTxReqStatus;

/// A request to prove a transaction on-chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct ProvenTxReq {
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
    pub proven_tx_req_id: i64,
    /// Associated proven transaction, once proof is acquired.
    pub proven_tx_id: Option<i64>,
    /// Current processing status.
    pub status: ProvenTxReqStatus,
    /// Number of broadcast/proof-fetch attempts made.
    pub attempts: i32,
    /// Whether the caller has been notified of completion.
    pub notified: bool,
    /// Transaction ID hex string.
    pub txid: String,
    /// Batch identifier for grouped processing.
    pub batch: Option<String>,
    /// JSON-encoded history of processing attempts.
    pub history: String,
    /// JSON-encoded notification configuration.
    pub notify: String,
    /// Raw serialized transaction bytes.
    pub raw_tx: Vec<u8>,
    /// Input BEEF data for transaction context.
    #[serde(rename = "inputBEEF")]
    #[sqlx(rename = "inputBEEF")]
    pub input_beef: Option<Vec<u8>>,
}
