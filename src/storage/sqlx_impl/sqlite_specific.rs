//! SQLite-specific storage setup with dual-pool WAL mode.
//!
//! Provides `create_sqlite_pools` which creates a single-writer pool
//! and a multi-reader pool, both configured with WAL journal mode.

#[cfg(feature = "sqlite")]
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
#[cfg(feature = "sqlite")]
use std::str::FromStr;

#[cfg(feature = "sqlite")]
use crate::error::WalletResult;
#[cfg(feature = "sqlite")]
use crate::storage::StorageConfig;

/// Create dual SQLite pools: one writer (max 1 connection) and one reader
/// (max N connections) both in WAL journal mode.
///
/// The writer pool uses `max_connections(1)` to serialize writes.
/// The reader pool uses `max_connections(config.sqlite_read_connections)` for
/// concurrent reads. Both pools set `busy_timeout` to 30 seconds and
/// `create_if_missing(true)`.
#[cfg(feature = "sqlite")]
pub async fn create_sqlite_pools(
    config: &StorageConfig,
) -> WalletResult<(SqlitePool, SqlitePool)> {
    let base_opts = SqliteConnectOptions::from_str(&config.url)?
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30))
        .create_if_missing(true);

    // Writer pool: exactly 1 connection for serialized writes
    let writer_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(config.min_connections.min(1))
        .idle_timeout(config.idle_timeout)
        .acquire_timeout(config.connect_timeout)
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

    // Ensure WAL mode is active on the writer
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&writer_pool)
        .await?;
    sqlx::query("PRAGMA synchronous=NORMAL")
        .execute(&writer_pool)
        .await?;

    Ok((writer_pool, reader_pool))
}
