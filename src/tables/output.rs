//! Output table struct.
//!
//! Maps to TS `TableOutput` in `wallet-toolbox/src/storage/schema/tables/TableOutput.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::types::StorageProvidedBy;

/// A transaction output record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Output {
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
    pub output_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Parent transaction foreign key.
    pub transaction_id: i64,
    /// Optional output basket foreign key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basket_id: Option<i64>,
    /// Whether this output is spendable by the wallet.
    pub spendable: bool,
    /// Whether this output is a change output.
    pub change: bool,
    /// Human-readable description of the output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_description: Option<String>,
    /// Output index within the transaction.
    pub vout: i32,
    /// Value in satoshis.
    pub satoshis: i64,
    /// Who provided the storage for this output.
    pub provided_by: StorageProvidedBy,
    /// Purpose string (e.g., "change", "payment").
    pub purpose: String,
    /// Output type descriptor (e.g., "P2PKH").
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub output_type: String,
    /// Transaction ID hex string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// Identity key of the sender (for received outputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_identity_key: Option<String>,
    /// BRC-42 derivation prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_prefix: Option<String>,
    /// BRC-42 derivation suffix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_suffix: Option<String>,
    /// Custom spending instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Transaction ID that spent this output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent_by: Option<i64>,
    /// nSequence number for non-final transactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<i32>,
    /// Human-readable description of the spending transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spending_description: Option<String>,
    /// Length of the locking script in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_length: Option<i64>,
    /// Byte offset of the locking script within the transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_offset: Option<i64>,
    /// The raw locking script bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locking_script: Option<Vec<u8>>,
}
