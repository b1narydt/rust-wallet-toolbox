//! StorageProvider trait -- the top of the storage trait hierarchy.
//!
//! Extends StorageReaderWriter with high-level operations like migration,
//! settings management, and lifecycle control. This is the trait that
//! WalletStorageManager and Wallet hold via `Arc<dyn StorageProvider>`.

use async_trait::async_trait;

use crate::error::WalletResult;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::TrxToken;
use crate::tables::Settings;
use crate::types::Chain;

/// Top-level storage provider interface for the wallet.
///
/// Adds high-level operations (migration, settings, lifecycle) on top of
/// the full CRUD interface provided by StorageReaderWriter.
///
/// Designed for dynamic dispatch: `Arc<dyn StorageProvider>`.
#[async_trait]
pub trait StorageProvider: StorageReaderWriter {
    /// Run database migrations to bring the schema up to date.
    async fn migrate_database(&self) -> WalletResult<String>;

    /// Get the current settings record.
    /// Typically reads from a cached value after `make_available()`.
    async fn get_settings(&self, trx: Option<&TrxToken>) -> WalletResult<Settings>;

    /// Ensure the storage is ready for use. Reads settings and caches them.
    async fn make_available(&self) -> WalletResult<Settings>;

    /// Destroy the storage, cleaning up all resources.
    async fn destroy(&self) -> WalletResult<()>;

    /// Drop all data from all tables. Used for testing.
    async fn drop_all_data(&self) -> WalletResult<()>;

    /// Get the storage identity key.
    fn get_storage_identity_key(&self) -> WalletResult<String>;

    /// Check whether the storage is currently available.
    fn is_available(&self) -> bool;

    /// Get the chain this storage is configured for.
    fn get_chain(&self) -> Chain;

    /// Set the active state of this storage provider.
    fn set_active(&self, active: bool);

    /// Check whether this is an active storage provider.
    fn is_active(&self) -> bool;
}
