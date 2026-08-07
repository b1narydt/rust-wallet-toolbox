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
pub mod portable;
pub mod remoting;
pub mod sqlx_impl;
pub mod sync;
pub mod traits;

// Re-export key types for convenience.
pub use find_args::*;
pub use manager::WalletStorageManager;
pub use remoting::StorageClient;
pub use sqlx_impl::trx_token::TrxToken;
pub use traits::{StorageProvider, StorageReader, StorageReaderWriter, WalletStorageProvider};

use crate::error::{WalletError, WalletResult};
use std::time::Duration;

/// SQLite `PRAGMA synchronous` level for the writer pool (WAL mode).
///
/// `Full` fsyncs the WAL on every commit: a committed transaction survives
/// power loss. `Normal` skips the per-commit fsync: commits since the last
/// WAL checkpoint can be lost on power loss (never on application crash,
/// and the database never corrupts either way). `Normal` is meaningfully
/// faster; `Full` is the default because a wallet losing a committed spend
/// record double-spends later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqliteSyncMode {
    /// Fsync per commit — committed means durable across power loss.
    #[default]
    Full,
    /// Fsync at checkpoint only — faster, may lose recent commits on power loss.
    Normal,
}

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
    /// Maximum time to wait when acquiring a read connection from the pool.
    pub connect_timeout: Duration,
    /// Maximum time a write may queue for the single SQLite writer
    /// connection before failing. All writes serialize through one
    /// connection, so under a burst of large transactions this is queue
    /// depth × transaction time; a short timeout converts backpressure
    /// into failed operations. (SQLite only.)
    pub write_acquire_timeout: Duration,
    /// Writer `PRAGMA synchronous` level (SQLite only).
    pub sqlite_synchronous: SqliteSyncMode,
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
            write_acquire_timeout: Duration::from_secs(60),
            sqlite_synchronous: SqliteSyncMode::Full,
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
