//! WalletSigner trait definition.
//!
//! Defines the async trait for the signer pipeline, which handles
//! create_action, sign_action, internalize_action, and abort_action.

use async_trait::async_trait;

use bsv::wallet::interfaces::AbortActionResult;

use crate::error::WalletResult;
use crate::signer::types::{
    SignerCreateActionResult, SignerInternalizeActionResult, SignerSignActionResult,
    ValidAbortActionArgs, ValidCreateActionArgs, ValidInternalizeActionArgs, ValidSignActionArgs,
};

/// The signer pipeline trait.
///
/// Handles transaction creation, signing, internalization, and abortion.
/// Implementations coordinate between key derivation, transaction building,
/// and storage to produce signed transactions.
///
/// All methods are async to support database and network operations.
#[async_trait]
pub trait WalletSigner: Send + Sync {
    /// Create a new transaction or prepare one for delayed signing.
    ///
    /// If `args.is_sign_action` is true, returns a `SignableTransaction` reference
    /// instead of signing immediately. The caller must later call `sign_action`.
    async fn create_action(
        &self,
        args: ValidCreateActionArgs,
    ) -> WalletResult<SignerCreateActionResult>;

    /// Sign a previously created transaction (delayed signing flow).
    ///
    /// The `args.reference` must match a pending sign action from a prior
    /// `create_action` call with `is_sign_action=true`.
    async fn sign_action(
        &self,
        args: ValidSignActionArgs,
    ) -> WalletResult<SignerSignActionResult>;

    /// Internalize outputs from an external transaction.
    ///
    /// Takes ownership of outputs in a pre-existing transaction, adding them
    /// to the wallet's tracked outputs (as wallet payments or basket insertions).
    async fn internalize_action(
        &self,
        args: ValidInternalizeActionArgs,
    ) -> WalletResult<SignerInternalizeActionResult>;

    /// Abort a pending transaction.
    ///
    /// Releases any locked UTXOs and removes pending state.
    async fn abort_action(
        &self,
        args: ValidAbortActionArgs,
    ) -> WalletResult<AbortActionResult>;
}
