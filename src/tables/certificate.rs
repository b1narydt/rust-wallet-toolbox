//! Certificate table struct.
//!
//! Maps to TS `TableCertificate` in `wallet-toolbox/src/storage/schema/tables/TableCertificate.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A certificate issued to a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Certificate {
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
    pub certificate_id: i64,
    /// Owning user foreign key.
    pub user_id: i64,
    /// Certificate type identifier string.
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub cert_type: String,
    /// Unique serial number.
    pub serial_number: String,
    /// Identity key of the certifier.
    pub certifier: String,
    /// Identity key of the subject.
    pub subject: String,
    /// Identity key of an optional verifier.
    pub verifier: Option<String>,
    /// On-chain outpoint for certificate revocation (txid.vout).
    pub revocation_outpoint: String,
    /// Digital signature of the certificate.
    pub signature: String,
    /// Soft-delete flag.
    pub is_deleted: bool,
}
