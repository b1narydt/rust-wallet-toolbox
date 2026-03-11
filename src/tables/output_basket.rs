//! OutputBasket table struct.
//!
//! Maps to TS `TableOutputBasket` in `wallet-toolbox/src/storage/schema/tables/TableOutputBasket.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A basket grouping for transaction outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct OutputBasket {
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
    pub basket_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Basket name (e.g., "default", "change").
    pub name: String,
    /// Target number of UTXOs to maintain in this basket.
    #[serde(rename = "numberOfDesiredUTXOs")]
    #[sqlx(rename = "numberOfDesiredUTXOs")]
    pub number_of_desired_utxos: i32,
    /// Minimum satoshi value for each UTXO in this basket.
    #[serde(rename = "minimumDesiredUTXOValue")]
    #[sqlx(rename = "minimumDesiredUTXOValue")]
    pub minimum_desired_utxo_value: i32,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
