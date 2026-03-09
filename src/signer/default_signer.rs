//! DefaultWalletSigner -- the production implementation of WalletSigner.
//!
//! Coordinates between the storage layer, BSV SDK signing, and wallet services
//! to implement the full createAction/signAction/internalizeAction/abortAction
//! lifecycle.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::AbortActionResult;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::traits::WalletSigner;
use crate::signer::types::{
    PendingSignAction, SignerCreateActionResult, SignerInternalizeActionResult,
    SignerSignActionResult, ValidAbortActionArgs, ValidCreateActionArgs,
    ValidInternalizeActionArgs, ValidSignActionArgs,
};
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;

/// Production implementation of the WalletSigner trait.
///
/// Holds references to storage, services, and key material. Manages
/// in-memory PendingSignAction state for the delayed signing flow.
pub struct DefaultWalletSigner {
    /// The storage manager for database operations.
    pub storage: Arc<WalletStorageManager>,
    /// The wallet services for network operations (broadcast, chain tracker, etc).
    pub services: Arc<dyn WalletServices>,
    /// The cached key deriver for BRC-42/BRC-29 key derivation.
    /// Uses interior mutability (RwLock with HashMap) so &self suffices.
    pub key_deriver: Arc<CachedKeyDeriver>,
    /// The chain being operated on.
    pub chain: Chain,
    /// The wallet's identity public key.
    pub identity_key: PublicKey,
    /// In-memory store for pending sign actions (delayed signing).
    /// Uses tokio::sync::Mutex for async access.
    pending_sign_actions: tokio::sync::Mutex<HashMap<String, PendingSignAction>>,
}

impl DefaultWalletSigner {
    /// Create a new DefaultWalletSigner.
    pub fn new(
        storage: Arc<WalletStorageManager>,
        services: Arc<dyn WalletServices>,
        key_deriver: Arc<CachedKeyDeriver>,
        chain: Chain,
        identity_key: PublicKey,
    ) -> Self {
        Self {
            storage,
            services,
            key_deriver,
            chain,
            identity_key,
            pending_sign_actions: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The auth string used for storage operations (identity key hex).
    fn auth(&self) -> String {
        self.identity_key.to_der_hex()
    }
}

#[async_trait]
impl WalletSigner for DefaultWalletSigner {
    async fn create_action(
        &self,
        args: ValidCreateActionArgs,
    ) -> WalletResult<SignerCreateActionResult> {
        let (result, pending) = crate::signer::methods::create_action::signer_create_action(
            self.storage.as_ref(),
            self.services.as_ref(),
            &self.key_deriver,
            &self.identity_key,
            &self.auth(),
            &args,
        )
        .await?;

        // If delayed signing, store the pending sign action
        if let Some(p) = pending {
            let mut map = self.pending_sign_actions.lock().await;
            map.insert(p.reference.clone(), p);
        }

        Ok(result)
    }

    async fn sign_action(
        &self,
        args: ValidSignActionArgs,
    ) -> WalletResult<SignerSignActionResult> {
        // Look up the pending sign action
        let pending = {
            let mut map = self.pending_sign_actions.lock().await;
            map.remove(&args.reference).ok_or_else(|| {
                WalletError::NotImplemented(
                    "recovery of out-of-session signAction reference data is not yet implemented."
                        .to_string(),
                )
            })?
        };

        let result = crate::signer::methods::sign_action::signer_sign_action(
            self.storage.as_ref(),
            self.services.as_ref(),
            &self.key_deriver,
            &self.identity_key,
            &self.auth(),
            &args,
            &pending,
        )
        .await?;

        Ok(result)
    }

    async fn internalize_action(
        &self,
        args: ValidInternalizeActionArgs,
    ) -> WalletResult<SignerInternalizeActionResult> {
        crate::signer::methods::internalize_action::signer_internalize_action(
            self.storage.as_ref(),
            self.services.as_ref(),
            &self.auth(),
            &args,
        )
        .await
    }

    async fn abort_action(
        &self,
        args: ValidAbortActionArgs,
    ) -> WalletResult<AbortActionResult> {
        // Remove from in-memory pending map if present
        {
            let mut map = self.pending_sign_actions.lock().await;
            map.remove(&args.reference);
        }

        crate::signer::methods::abort_action::signer_abort_action(
            self.storage.as_ref(),
            &self.auth(),
            &args,
        )
        .await
    }
}
