//! Remote storage client module.
//!
//! Provides `StorageClient<W>`, a JSON-RPC client that implements
//! `WalletStorageProvider` by forwarding all calls to a remote TS storage server.

pub mod storage_client;
pub use storage_client::StorageClient;
