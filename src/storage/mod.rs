//! Storage module for the BSV wallet.
//!
//! Provides the trait hierarchy, type-erased transaction tokens,
//! find argument types, storage configuration, and SQL-backed implementations.

pub mod action_traits;
pub mod action_types;
pub mod beef;
pub mod find_args;
pub mod manager;
pub mod methods;
pub mod sqlx_impl;
pub mod sync;
pub mod traits;

// Re-export key types for convenience.
pub use find_args::*;
pub use manager::WalletStorageManager;
pub use sqlx_impl::trx_token::TrxToken;
pub use traits::{StorageProvider, StorageReader, StorageReaderWriter, WalletStorageProvider};

use crate::error::{WalletError, WalletResult};
use std::time::Duration;

/// Configuration for storage pool connections.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Database connection URL (e.g., "sqlite::memory:" or "sqlite:wallet.db").
    pub url: String,
    /// Number of read connections for SQLite dual-pool (ignored for MySQL/PostgreSQL).
    pub sqlite_read_connections: u32,
    /// Minimum number of connections in the pool.
    pub min_connections: u32,
    /// Maximum number of connections in the pool.
    pub max_connections: u32,
    /// How long an idle connection can remain in the pool before being closed.
    pub idle_timeout: Duration,
    /// Maximum time to wait when acquiring a connection from the pool.
    pub connect_timeout: Duration,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            url: String::from("sqlite::memory:"),
            sqlite_read_connections: 4,
            min_connections: 2,
            max_connections: 50,
            idle_timeout: Duration::from_secs(600), // 10 minutes
            connect_timeout: Duration::from_secs(5),
        }
    }
}

/// Verify that a result set contains at most one element.
/// Returns `Ok(Some(item))` if exactly one, `Ok(None)` if empty,
/// or `Err` if more than one.
pub fn verify_one_or_none<T>(mut results: Vec<T>) -> WalletResult<Option<T>> {
    if results.len() > 1 {
        return Err(WalletError::Internal(format!(
            "Expected at most one result, got {}",
            results.len()
        )));
    }
    Ok(results.pop())
}

/// Verify that a result set contains exactly one element.
/// Returns `Ok(item)` if exactly one, or `Err` otherwise.
pub fn verify_one<T>(mut results: Vec<T>) -> WalletResult<T> {
    if results.len() != 1 {
        return Err(WalletError::Internal(format!(
            "Expected exactly one result, got {}",
            results.len()
        )));
    }
    Ok(results.pop().unwrap())
}
