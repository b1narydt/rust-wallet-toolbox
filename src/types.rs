//! Shared types used across wallet toolbox subsystems.
//!
//! Contains small enum and struct types that are referenced by multiple modules.
//! Kept minimal -- only types needed by status.rs and table structs.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// The BSV network chain type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum Chain {
    /// BSV mainnet
    Main,
    /// BSV testnet
    Test,
}

/// Indicates who provides the storage backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StorageProvidedBy {
    /// Storage is provided by the service
    Storage,
    /// Storage is provided by the caller
    You,
    /// Both the caller and storage contributed (e.g., user specifies inputs, storage allocates change)
    #[serde(rename = "you-and-storage")]
    #[strum(serialize = "you-and-storage")]
    YouAndStorage,
}

#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(Chain);
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(StorageProvidedBy);
