//! Services layer for BSV wallet network communication.
//!
//! Provides the WalletServices trait, provider traits, result/config types,
//! and the generic ServiceCollection failover pattern.

pub mod chaintracker;
pub mod providers;
pub mod service_collection;
pub mod services;
pub mod traits;
pub mod types;

// Re-export key types for convenience
pub use service_collection::ServiceCollection;
pub use services::Services;
pub use traits::*;
pub use types::*;
