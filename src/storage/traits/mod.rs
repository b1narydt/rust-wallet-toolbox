//! Storage trait hierarchy.
//!
//! Three-level trait hierarchy mirroring the TypeScript storage abstraction:
//! - `StorageReader`: Read-only operations (find/count)
//! - `StorageReaderWriter`: Read-write operations (insert/update/transaction)
//! - `StorageProvider`: High-level operations (migrate/settings/lifecycle)

pub mod provider;
pub mod reader;
pub mod reader_writer;
pub mod scope;
pub mod wallet_provider;

pub use provider::StorageProvider;
pub use reader::StorageReader;
pub use reader_writer::StorageReaderWriter;
pub use scope::{resolve_user_scope, UserScope, UserScopedPartial};
pub use wallet_provider::WalletStorageProvider;
