//! StorageActionProvider trait for storage-level action operations.
//!
//! Extends StorageProvider with action methods (create_action, process_action,
//! internalize_action). Provides default implementations that delegate to
//! the storage method functions.

use async_trait::async_trait;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionResult, StorageInternalizeActionArgs,
    StorageInternalizeActionResult, StorageProcessActionArgs, StorageProcessActionResult,
};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

/// Trait extending StorageProvider with action methods.
///
/// These methods are called by the signer pipeline to create, process,
/// and internalize transactions at the storage level.
#[async_trait]
pub trait StorageActionProvider: StorageProvider {
    /// Create a new transaction in storage.
    ///
    /// Allocates UTXOs, creates transaction/output/label records, and returns
    /// the data needed to build the actual Bitcoin transaction.
    async fn create_action(
        &self,
        auth: &str,
        args: &StorageCreateActionArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<StorageCreateActionResult> {
        let (user, _) = self.find_or_insert_user(auth, trx).await?;
        crate::storage::methods::create_action::storage_create_action(
            self, user.user_id, args, trx,
        )
        .await
    }

    /// Process (commit) a signed transaction to storage.
    ///
    /// Stores the signed transaction, creates a ProvenTxReq record,
    /// and optionally triggers broadcasting.
    async fn process_action(
        &self,
        auth: &str,
        args: &StorageProcessActionArgs,
        trx: Option<&TrxToken>,
    ) -> WalletResult<StorageProcessActionResult> {
        let (user, _) = self.find_or_insert_user(auth, trx).await?;
        crate::storage::methods::process_action::storage_process_action(
            self, user.user_id, args, trx,
        )
        .await
    }

    /// Internalize outputs from an external transaction.
    ///
    /// Takes ownership of specified outputs, adding them to the wallet's
    /// tracked outputs as wallet payments or basket insertions.
    /// Requires WalletServices for chain tracker access (merkle proof validation).
    async fn internalize_action(
        &self,
        auth: &str,
        args: &StorageInternalizeActionArgs,
        services: &dyn WalletServices,
        trx: Option<&TrxToken>,
    ) -> WalletResult<StorageInternalizeActionResult> {
        let (user, _) = self.find_or_insert_user(auth, trx).await?;
        crate::storage::methods::internalize_action::storage_internalize_action(
            self, services, user.user_id, args, trx,
        )
        .await
    }
}

/// Blanket implementation: every StorageProvider automatically implements
/// StorageActionProvider with the default stubs.
impl<T: StorageProvider> StorageActionProvider for T {}
