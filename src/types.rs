//! Shared types used across wallet toolbox subsystems.
//!
//! Contains small enum and struct types that are referenced by multiple modules.
//! Kept minimal -- only types needed by status.rs and table structs.

use serde::{Deserialize, Serialize};
use sqlx::Type;
use strum::{Display, EnumString};

/// The BSV network chain type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Display, EnumString)]
#[sqlx(type_name = "TEXT", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum Chain {
    /// BSV mainnet
    Main,
    /// BSV testnet
    Test,
}

/// Indicates who provides the storage backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Display, EnumString)]
#[sqlx(type_name = "TEXT", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StorageProvidedBy {
    /// Storage is provided by the service
    Storage,
    /// Storage is provided by the caller
    You,
}
