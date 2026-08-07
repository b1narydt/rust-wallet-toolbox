//! SQLite-specific storage setup with dual-pool WAL mode.
//!
//! Provides `create_sqlite_pools` which creates a single-writer pool
//! and a multi-reader pool, both configured with WAL journal mode.

#[cfg(feature = "sqlite")]
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
#[cfg(feature = "sqlite")]
use std::str::FromStr;

#[cfg(feature = "sqlite")]
use crate::error::WalletResult;
#[cfg(feature = "sqlite")]
use crate::storage::{SqliteSyncMode, StorageConfig};

/// Create dual SQLite pools: one writer (max 1 connection) and one reader
/// (max N connections) both in WAL journal mode.
///
/// The writer pool uses `max_connections(1)` to serialize writes, and waits
/// up to `config.write_acquire_timeout` for the connection — all writes
/// queue through it, so its acquire timeout is a write-queue bound, not a
/// connect bound. The reader pool uses
/// `max_connections(config.sqlite_read_connections)` for concurrent reads
/// with the shorter `config.connect_timeout`.
///
/// All pragmas are set in the connect options so every connection the pools
/// ever open — including replacements for recycled connections — gets the
/// same journal mode, busy timeout, and synchronous level.
#[cfg(feature = "sqlite")]
pub async fn create_sqlite_pools(config: &StorageConfig) -> WalletResult<(SqlitePool, SqlitePool)> {
    let synchronous = match config.sqlite_synchronous {
        SqliteSyncMode::Full => SqliteSynchronous::Full,
        SqliteSyncMode::Normal => SqliteSynchronous::Normal,
    };
    let base_opts = SqliteConnectOptions::from_str(&config.url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(synchronous)
        .busy_timeout(std::time::Duration::from_secs(30))
        .create_if_missing(true);

    // Writer pool: exactly 1 connection for serialized writes
    let writer_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(config.min_connections.min(1))
        .idle_timeout(config.idle_timeout)
        .acquire_timeout(config.write_acquire_timeout)
        .connect_with(base_opts.clone())
        .await?;

    // Reader pool: multiple connections for concurrent reads
    let reader_pool = SqlitePoolOptions::new()
        .max_connections(config.sqlite_read_connections)
        .min_connections(config.min_connections)
        .idle_timeout(config.idle_timeout)
        .acquire_timeout(config.connect_timeout)
        .connect_with(base_opts.read_only(true))
        .await?;

    Ok((writer_pool, reader_pool))
}
