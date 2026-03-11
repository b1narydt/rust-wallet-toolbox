//! CertificateField table struct.
//!
//! Maps to TS `TableCertificateField` in `wallet-toolbox/src/storage/schema/tables/TableCertificateField.ts`.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A field within a certificate, storing encrypted values and master keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct CertificateField {
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
    /// Owning user foreign key.
    pub user_id: i64,
    /// Parent certificate foreign key.
    pub certificate_id: i64,
    /// Name of the certificate field.
    pub field_name: String,
    /// Encrypted field value.
    pub field_value: String,
    /// Master key used for field encryption.
    pub master_key: String,
}
