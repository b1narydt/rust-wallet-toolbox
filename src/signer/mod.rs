//! Signer pipeline module.
//!
//! Contains the WalletSigner trait, DefaultWalletSigner implementation,
//! and all types used by the signing pipeline.

pub mod broadcast_outcome;
pub mod build_signable;
pub mod complete_signed;
pub mod default_signer;
pub mod methods;
pub mod traits;
pub mod types;
pub mod verify_unlock_scripts;

// Re-export key items
pub use build_signable::build_signable_transaction;
pub use complete_signed::complete_signed_transaction;
pub use default_signer::DefaultWalletSigner;
pub use traits::WalletSigner;
pub use types::{
    PendingSignAction, PendingStorageInput, SignerCreateActionResult,
    SignerInternalizeActionResult, SignerSignActionResult, ValidAbortActionArgs,
    ValidCreateActionArgs, ValidInternalizeActionArgs, ValidSignActionArgs,
    DEFAULT_FEE_RATE_SAT_PER_KB,
};
