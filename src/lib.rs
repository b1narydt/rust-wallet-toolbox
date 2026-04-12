// Note: `missing_docs` is intentionally NOT warned/denied at crate level.
// CI runs clippy with `-A missing_docs` to allow private/incomplete doc coverage
// on struct fields, variants, and methods. The crate-level attribute would
// otherwise override the command-line `-A` (rustc lint precedence).
#![allow(missing_docs)]
// `WalletError::ReviewActions` and `::InvalidMerkleRoot` carry rich contextual
// data required by the TypeScript-parity wire format (matching WERR_* payloads).
// Boxing the variant would force an API-breaking change to a stable public
// error type used pervasively as `Result<_, WalletError>`. The size is
// intentional for API stability and wire-format parity.
#![allow(clippy::result_large_err)]
//! BSV Wallet Toolbox - Production-ready BSV wallet implementation in Rust.
//!
//! This crate provides a complete, production-ready BSV wallet with persistent
//! storage, automatic transaction broadcasting, proof collection, and chain
//! monitoring. It translates the TypeScript wallet-toolbox into idiomatic Rust.
//!
//! # Modules
//!
//! - [`error`] - Unified error types with WERR codes
//! - [`status`] - Status enums for transactions, proofs, and sync
//! - [`types`] - Shared types (`Chain`, `StorageProvidedBy`)
//! - [`tables`] - Database table structs for all 16 entity types
//! - [`storage`] - Storage traits, manager, and SQLx implementation
//! - [`services`] - Network service providers (ARC, WhatsOnChain, etc.)
//! - [`signer`] - Transaction signing with BRC-29 key derivation
//! - [`wallet`] - High-level Wallet struct implementing WalletInterface
//! - [`monitor`] - Background task runner for proofs, sync, and chain monitoring
//! - [`utility`] - Cryptographic helpers and BRC-29 script templates
//! - [`logging`] - Structured logging initialization via tracing

/// Authentication manager for WAB-based wallet authentication flows.
pub mod auth_manager;
/// Error types for wallet operations.
pub mod error;
/// Structured logging initialization.
pub mod logging;
/// Background monitoring tasks for proofs and chain state.
pub mod monitor;
/// Permission management for wallet operations.
pub mod permissions;
/// Network service providers and traits.
pub mod services;
/// Transaction signing and key derivation.
pub mod signer;
/// Macro for implementing sqlx traits on enums as string columns.
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
#[macro_use]
mod sqlx_string_enum;

/// Block header tracking, storage, and chain reorganization detection.
pub mod chaintracks;
/// Lenient NaiveDateTime serde helpers (handles trailing "Z" from TS).
pub mod serde_datetime;
/// Lenient serde helpers for TS interop (integer-as-bool, etc.).
pub mod serde_helpers;
/// Status enums for wallet entities.
pub mod status;
/// Storage layer: traits, manager, and implementations.
pub mod storage;
/// Database table structs mapping to SQL schema.
pub mod tables;
/// Shared types used across subsystems.
pub mod types;
/// Cryptographic utilities and BRC-29 script templates.
pub mod utility;
/// Wallet Authentication Backend (WAB) client for identity verification.
pub mod wab_client;
/// High-level wallet implementation.
pub mod wallet;

/// Database migration helpers (feature-gated by database backend).
#[cfg(any(feature = "sqlite", feature = "mysql", feature = "postgres"))]
pub mod migrations;

/// BSV transaction type from the SDK.
pub use bsv::transaction::Transaction;
/// The WalletInterface trait defining all 29 wallet operations.
pub use bsv::wallet::interfaces::WalletInterface;
/// Key derivation for BRC-42/BRC-43 protocols.
pub use bsv::wallet::key_deriver::KeyDeriver;
/// Protocol wallet providing default WalletInterface implementations.
pub use bsv::wallet::proto_wallet::ProtoWallet;

/// Unified wallet error type.
pub use error::WalletError;
/// Convenience Result alias for wallet operations.
pub use error::WalletResult;
/// Status of a proven transaction request.
pub use status::ProvenTxReqStatus;
/// Status of wallet synchronization.
pub use status::SyncStatus;
/// Status of a wallet transaction.
pub use status::TransactionStatus;

/// Trait for wallet setup and configuration.
pub use wallet::setup::SetupWallet;
/// Builder for constructing configured Wallet instances.
pub use wallet::setup::WalletBuilder;
