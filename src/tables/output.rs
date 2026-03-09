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
    pub created_at: NaiveDateTime,
    /// When this record was last updated.
    #[sqlx(rename = "updated_at")]
    pub updated_at: NaiveDateTime,
    /// Primary key.
    pub output_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Parent transaction foreign key.
    pub transaction_id: i64,
    /// Optional output basket foreign key.
    pub basket_id: Option<i64>,
    /// Whether this output is spendable by the wallet.
    pub spendable: bool,
    /// Whether this output is a change output.
    pub change: bool,
    /// Human-readable description of the output.
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
    pub txid: Option<String>,
    /// Identity key of the sender (for received outputs).
    pub sender_identity_key: Option<String>,
    /// BRC-42 derivation prefix.
    pub derivation_prefix: Option<String>,
    /// BRC-42 derivation suffix.
    pub derivation_suffix: Option<String>,
    /// Custom spending instructions.
    pub custom_instructions: Option<String>,
    /// Transaction ID that spent this output.
    pub spent_by: Option<i64>,
    /// nSequence number for non-final transactions.
    pub sequence_number: Option<i32>,
    /// Human-readable description of the spending transaction.
    pub spending_description: Option<String>,
    /// Length of the locking script in bytes.
    pub script_length: Option<i64>,
    /// Byte offset of the locking script within the transaction.
    pub script_offset: Option<i64>,
    /// The raw locking script bytes.
    pub locking_script: Option<Vec<u8>>,
}
