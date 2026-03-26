//! Remote storage client module.
//!
//! Provides `StorageClient<W>`, a JSON-RPC client that implements
//! `WalletStorageProvider` by forwarding all calls to a remote TS storage server.
//!
//! Also provides `WalletArc<W>`, a `Clone`-able `Arc` wrapper for any
//! `WalletInterface` implementor, enabling use of types like `ProtoWallet`
//! (which does not implement `Clone`) with `StorageClient`.

pub mod storage_client;
pub mod wallet_arc;

pub use storage_client::StorageClient;
pub use wallet_arc::WalletArc;
