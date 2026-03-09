//! SQL-backed storage implementation using sqlx.
//!
//! Contains the generic [`StorageSqlx`] struct, type aliases for each
//! backend, and database-specific modules.

/// SQL dialect helpers for building dynamic WHERE clauses.
pub mod dialect;
/// Find query generation (SELECT with dynamic WHERE).
pub mod find;
/// Insert query generation.
pub mod insert;
/// StorageReader/StorageReaderWriter/StorageProvider trait implementations.
pub mod trait_impls;
/// Transaction token type for transactional operations.
pub mod trx_token;
/// Update query generation.
pub mod update;

#[cfg(feature = "sqlite")]
pub mod sqlite_specific;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{WalletError, WalletResult};
use crate::storage::StorageConfig;
use crate::tables::Settings;
use crate::types::Chain;

/// Generic SQL-backed storage implementation parameterized by database backend.
///
/// For SQLite, `read_pool` is `Some(pool)` providing a separate read pool.
/// For MySQL/PostgreSQL, `read_pool` is `None` and the single `write_pool` is used.
pub struct StorageSqlx<DB: sqlx::Database> {
    /// Connection pool for write operations (single connection for SQLite).
    pub write_pool: sqlx::Pool<DB>,
    /// Optional dedicated read pool (SQLite WAL mode optimization).
    pub read_pool: Option<sqlx::Pool<DB>>,
    /// Storage configuration (e.g., max script size).
    pub config: StorageConfig,
    /// BSV chain this storage is configured for.
    pub chain: Chain,
    /// Whether this storage provider is currently active.
    pub active: AtomicBool,
    /// Identity key identifying this storage instance.
    pub storage_identity_key: String,
    /// Cached settings loaded from the settings table.
    pub settings: tokio::sync::RwLock<Option<Settings>>,
}

impl<DB: sqlx::Database> StorageSqlx<DB> {
    /// Get the read pool. Returns the dedicated read pool if available,
    /// otherwise falls back to the write pool.
    pub fn read_pool(&self) -> &sqlx::Pool<DB> {
        self.read_pool.as_ref().unwrap_or(&self.write_pool)
    }

    /// Check if this storage provider is currently marked as active.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Set the active state of this storage provider.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// SQLite-specific constructor and type alias
// ---------------------------------------------------------------------------

/// Type alias for SQLite-backed storage.
#[cfg(feature = "sqlite")]
pub type SqliteStorage = StorageSqlx<sqlx::Sqlite>;

#[cfg(feature = "sqlite")]
impl SqliteStorage {
    /// Create a new SQLite storage with dual-pool WAL mode.
    pub async fn new_sqlite(config: StorageConfig, chain: Chain) -> WalletResult<Self> {
        let (write_pool, read_pool) = sqlite_specific::create_sqlite_pools(&config).await?;
        Ok(Self {
            write_pool,
            read_pool: Some(read_pool),
            config,
            chain,
            active: AtomicBool::new(false),
            storage_identity_key: String::new(),
            settings: tokio::sync::RwLock::new(None),
        })
    }

    /// Begin a SQLite transaction. Returns a TrxToken wrapping the transaction.
    pub async fn begin_sqlite_transaction(&self) -> WalletResult<trx_token::TrxToken> {
        let tx = self.write_pool.begin().await?;
        let inner: SqliteTrxInner = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));
        Ok(trx_token::TrxToken::new(inner))
    }

    /// Extract the SQLite transaction inner handle from a TrxToken.
    pub fn extract_sqlite_trx(trx: &trx_token::TrxToken) -> WalletResult<&SqliteTrxInner> {
        trx.downcast_ref::<SqliteTrxInner>().ok_or_else(|| {
            WalletError::Internal("TrxToken does not contain a SQLite transaction".to_string())
        })
    }
}

/// The concrete inner type for SQLite transactions.
/// Wrapped in `Arc<Mutex<Option<...>>>` to allow shared ownership and
/// "take" semantics for commit/rollback.
#[cfg(feature = "sqlite")]
pub type SqliteTrxInner =
    std::sync::Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, sqlx::Sqlite>>>>;

// ---------------------------------------------------------------------------
// MySQL-specific constructor and type alias
// ---------------------------------------------------------------------------

#[cfg(feature = "mysql")]
pub type MysqlStorage = StorageSqlx<sqlx::MySql>;

/// The concrete inner type for MySQL transactions.
#[cfg(feature = "mysql")]
pub type MysqlTrxInner =
    std::sync::Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, sqlx::MySql>>>>;

#[cfg(feature = "mysql")]
impl MysqlStorage {
    /// Create a new MySQL storage with a single connection pool.
    pub async fn new_mysql(config: StorageConfig, chain: Chain) -> WalletResult<Self> {
        use sqlx::mysql::MySqlPoolOptions;

        let pool = MySqlPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .idle_timeout(config.idle_timeout)
            .acquire_timeout(config.connect_timeout)
            .connect(&config.url)
            .await?;

        Ok(Self {
            write_pool: pool,
            read_pool: None,
            config,
            chain,
            active: AtomicBool::new(false),
            storage_identity_key: String::new(),
            settings: tokio::sync::RwLock::new(None),
        })
    }

    /// Begin a MySQL transaction. Returns a TrxToken wrapping the transaction.
    pub async fn begin_mysql_transaction(&self) -> WalletResult<trx_token::TrxToken> {
        let tx = self.write_pool.begin().await?;
        let inner: MysqlTrxInner = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));
        Ok(trx_token::TrxToken::new(inner))
    }

    /// Extract the MySQL transaction inner handle from a TrxToken.
    pub fn extract_mysql_trx(trx: &trx_token::TrxToken) -> WalletResult<&MysqlTrxInner> {
        trx.downcast_ref::<MysqlTrxInner>().ok_or_else(|| {
            WalletError::Internal("TrxToken does not contain a MySQL transaction".to_string())
        })
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL-specific constructor and type alias
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub type PgStorage = StorageSqlx<sqlx::Postgres>;

/// The concrete inner type for PostgreSQL transactions.
#[cfg(feature = "postgres")]
pub type PgTrxInner =
    std::sync::Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, sqlx::Postgres>>>>;

#[cfg(feature = "postgres")]
impl PgStorage {
    /// Create a new PostgreSQL storage with a single connection pool.
    pub async fn new_postgres(config: StorageConfig, chain: Chain) -> WalletResult<Self> {
        use sqlx::postgres::PgPoolOptions;

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .idle_timeout(config.idle_timeout)
            .acquire_timeout(config.connect_timeout)
            .connect(&config.url)
            .await?;

        Ok(Self {
            write_pool: pool,
            read_pool: None,
            config,
            chain,
            active: AtomicBool::new(false),
            storage_identity_key: String::new(),
            settings: tokio::sync::RwLock::new(None),
        })
    }

    /// Begin a PostgreSQL transaction. Returns a TrxToken wrapping the transaction.
    pub async fn begin_pg_transaction(&self) -> WalletResult<trx_token::TrxToken> {
        let tx = self.write_pool.begin().await?;
        let inner: PgTrxInner = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));
        Ok(trx_token::TrxToken::new(inner))
    }

    /// Extract the PostgreSQL transaction inner handle from a TrxToken.
    pub fn extract_pg_trx(trx: &trx_token::TrxToken) -> WalletResult<&PgTrxInner> {
        trx.downcast_ref::<PgTrxInner>().ok_or_else(|| {
            WalletError::Internal("TrxToken does not contain a PostgreSQL transaction".to_string())
        })
    }
}
