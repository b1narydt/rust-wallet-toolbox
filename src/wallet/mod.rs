//! Wallet module: core wallet struct and supporting types.
//!
//! Contains the Wallet struct (Plan 03), all supporting types, traits,
//! validation helpers, settings manager, beef helpers, and error helpers.

pub mod beef_helpers;
pub mod certificates;
pub mod discovery;
pub mod error_helpers;
pub mod privileged;
pub mod settings;
pub mod setup;
pub mod types;
pub mod validation;
pub mod wallet;

// Re-export key public types for convenience.
pub use privileged::PrivilegedKeyManager;
pub use settings::WalletSettingsManager;
pub use setup::{SetupWallet, WalletBuilder};
pub use types::{AuthId, KeyPair, StorageIdentity, WalletArgs};
pub use wallet::Wallet;
