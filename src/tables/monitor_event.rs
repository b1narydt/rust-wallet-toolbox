//! MonitorEvent table struct.
//!
//! Maps to TS `TableMonitorEvent` in `wallet-toolbox/src/storage/schema/tables/TableMonitorEvent.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// An event recorded by the chain monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct MonitorEvent {
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
    pub id: i64,
    /// Event type identifier string.
    pub event: String,
    /// Optional JSON details for the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}
