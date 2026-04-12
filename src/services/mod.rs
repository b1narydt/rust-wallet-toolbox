//! Services layer for BSV wallet network communication.
//!
//! Provides the WalletServices trait, provider traits, result/config types,
//! and the generic ServiceCollection failover pattern.

pub mod chaintracker;
pub mod providers;
pub mod service_collection;
// The `services` submodule name mirrors the TypeScript wallet-toolbox layout
// (`services/services.ts`) where the concrete `Services` struct lives in its
// own file. Renaming would be a breaking change to the public module path.
#[allow(clippy::module_inception)]
pub mod services;
pub mod traits;
pub mod types;

// Re-export key types for convenience
pub use service_collection::ServiceCollection;
pub use services::Services;
pub use traits::*;
pub use types::*;
