//! Feature-gated database migration helpers.
//!
//! Each database backend has its own migration directory and feature-gated
//! helper functions. The `sqlx::migrate!()` macro embeds the SQL files at
//! compile time.

use tracing::{error, info, instrument};

use crate::WalletResult;
use crate::error::WalletError;

/// Returns the SQLite migrator with embedded migration files.
#[cfg(feature = "sqlite")]
pub fn sqlite_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("migrations/sqlite")
}

/// Returns the MySQL migrator with embedded migration files.
#[cfg(feature = "mysql")]
pub fn mysql_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("migrations/mysql")
}

/// Returns the PostgreSQL migrator with embedded migration files.
#[cfg(feature = "postgres")]
pub fn postgres_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("migrations/postgres")
}

/// Run SQLite migrations against the provided pool.
#[cfg(feature = "sqlite")]
#[instrument(skip(pool))]
pub async fn run_sqlite_migrations(pool: &sqlx::SqlitePool) -> WalletResult<()> {
    info!("Running SQLite migrations");
    let migrator = sqlite_migrator();
    migrator.run(pool).await.map_err(|e| {
        error!(error = %e, "SQLite migration failed");
        WalletError::Internal(format!("SQLite migration failed: {e}"))
    })?;
    info!("SQLite migrations completed successfully");
    Ok(())
}

/// Run MySQL migrations against the provided pool.
#[cfg(feature = "mysql")]
#[instrument(skip(pool))]
pub async fn run_mysql_migrations(pool: &sqlx::MySqlPool) -> WalletResult<()> {
    info!("Running MySQL migrations");
    let migrator = mysql_migrator();
    migrator.run(pool).await.map_err(|e| {
        error!(error = %e, "MySQL migration failed");
        WalletError::Internal(format!("MySQL migration failed: {e}"))
    })?;
    info!("MySQL migrations completed successfully");
    Ok(())
}

/// Run PostgreSQL migrations against the provided pool.
#[cfg(feature = "postgres")]
#[instrument(skip(pool))]
pub async fn run_postgres_migrations(pool: &sqlx::PgPool) -> WalletResult<()> {
    info!("Running PostgreSQL migrations");
    let migrator = postgres_migrator();
    migrator.run(pool).await.map_err(|e| {
        error!(error = %e, "PostgreSQL migration failed");
        WalletError::Internal(format!("PostgreSQL migration failed: {e}"))
    })?;
    info!("PostgreSQL migrations completed successfully");
    Ok(())
}
