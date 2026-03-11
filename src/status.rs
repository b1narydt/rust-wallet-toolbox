//! Status enums for wallet entities.
//!
//! Each enum serializes to camelCase strings matching the TypeScript values exactly.
//! All enums derive serde for JSON serialization and strum for Display/FromStr conversions.
//! Database encoding/decoding is handled by the `impl_sqlx_string_enum!` macro in
//! the sqlx_string_enum module, which treats each enum as a VARCHAR/TEXT string column.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Status of a wallet transaction.
///
/// Maps to TS `TransactionStatus` in `wallet-toolbox/src/sdk/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum TransactionStatus {
    /// Transaction completed successfully and is proven on chain.
    Completed,
    /// Transaction failed to broadcast or was rejected.
    Failed,
    /// Transaction has not yet been processed.
    Unprocessed,
    /// Transaction is currently being broadcast to the network.
    Sending,
    /// Transaction was broadcast but is not yet proven in a block.
    Unproven,
    /// Transaction is awaiting signatures before it can be broadcast.
    Unsigned,
    /// Transaction was created with noSend flag and will not be broadcast.
    Nosend,
    /// Transaction contains nLockTime or nSequence constraints and is not yet final.
    Nonfinal,
    /// Transaction is being unfailed (moved from failed back to processing).
    Unfail,
}

/// Status of a proven transaction request.
///
/// Maps to TS `ProvenTxReqStatus` in `wallet-toolbox/src/sdk/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ProvenTxReqStatus {
    /// Request is currently being sent to miners.
    Sending,
    /// Request has not yet been sent.
    Unsent,
    /// Request was created with noSend flag and will not be sent.
    Nosend,
    /// Request status is unknown or uninitialized.
    Unknown,
    /// Request contains a non-final transaction.
    Nonfinal,
    /// Request has not yet been processed.
    Unprocessed,
    /// Transaction was broadcast but is not yet mined.
    Unmined,
    /// Waiting for a callback from the broadcast service.
    Callback,
    /// Transaction is in a block but not yet sufficiently confirmed.
    Unconfirmed,
    /// Proof request completed successfully.
    Completed,
    /// Transaction was determined to be invalid.
    Invalid,
    /// Transaction was detected as a double spend.
    DoubleSpend,
    /// Request is being unfailed (moved from failed back to processing).
    Unfail,
}

/// Status of wallet synchronization.
///
/// Maps to TS `SyncStatus` in `wallet-toolbox/src/sdk/WalletStorage.interfaces.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum SyncStatus {
    /// Synchronization completed successfully.
    Success,
    /// Synchronization encountered an error.
    Error,
    /// Remote storage has been identified but not yet synced.
    Identified,
    /// Records have been updated from the remote storage.
    Updated,
    /// Sync status has not been determined.
    Unknown,
}

/// Status of a transaction output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum OutputStatus {
    /// The output has not been spent and is available for use.
    Unspent,
    /// The output has been consumed as an input in another transaction.
    Spent,
}

#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(TransactionStatus);
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(ProvenTxReqStatus);
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(SyncStatus);
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
impl_sqlx_string_enum!(OutputStatus);
