//! DefaultWalletSigner -- the production implementation of WalletSigner.
//!
//! Coordinates between the storage layer, BSV SDK signing, and wallet services
//! to implement the full createAction/signAction/internalizeAction/abortAction
//! lifecycle.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use async_trait::async_trait;

use bsv::primitives::public_key::PublicKey;
use bsv::transaction::transaction::Transaction;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::AbortActionResult;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::traits::WalletSigner;
use crate::signer::types::{
    PendingSignAction, PendingStorageInput, SignerCreateActionResult,
    SignerInternalizeActionResult, SignerSignActionResult, ValidAbortActionArgs,
    ValidCreateActionArgs, ValidInternalizeActionArgs, ValidSignActionArgs,
};
use crate::status::TransactionStatus;
use crate::storage::action_types::StorageCreateActionResult;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
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

    /// Recover a PendingSignAction from storage for out-of-session signAction calls.
    ///
    /// When the in-memory HashMap does not contain the reference (e.g. after a
    /// process restart), this method reconstructs the PendingSignAction by:
    /// 1. Looking up the Transaction record by reference.
    /// 2. Verifying the transaction is still awaiting signatures.
    /// 3. Parsing the stored unsigned raw_tx to get input outpoints.
    /// 4. Looking up the Output records for each spent input to get derivation
    ///    data needed for BRC-29 signing.
    /// 5. Assembling a PendingSignAction with all fields required by signer_sign_action.
    async fn recover_pending_sign_action(
        &self,
        reference: &str,
    ) -> WalletResult<PendingSignAction> {
        let auth = self.auth();

        // --- Step 1: Find the user ---
        let user = self
            .storage
            .find_user_by_identity_key(&auth)
            .await?
            .ok_or_else(|| WalletError::Unauthorized("User not found".to_string()))?;

        // --- Step 2: Find the transaction by reference ---
        let txs = self
            .storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(user.user_id),
                        reference: Some(reference.to_string()),
                        ..Default::default()
                    },
                    no_raw_tx: false,
                    ..Default::default()
                },
            )
            .await?;

        let tx_record = txs
            .into_iter()
            .next()
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "reference".to_string(),
                must_be: "a reference for an existing unsigned transaction".to_string(),
            })?;

        // --- Step 3: Verify the transaction is in a signable state ---
        let signable_statuses = [TransactionStatus::Unsigned, TransactionStatus::Nosend];
        if !signable_statuses.contains(&tx_record.status) {
            return Err(WalletError::InvalidOperation(format!(
                "Transaction '{}' has status '{}' and cannot be signed",
                reference, tx_record.status
            )));
        }

        // --- Step 4: Get raw_tx and input_beef from the transaction ---
        let raw_tx_bytes = tx_record.raw_tx.ok_or_else(|| {
            WalletError::Internal(format!(
                "Transaction '{}' has no raw_tx stored for signing recovery",
                reference
            ))
        })?;
        let input_beef = tx_record.input_beef;

        // --- Step 5: Parse the unsigned transaction to get input outpoints ---
        let mut cursor = Cursor::new(&raw_tx_bytes);
        let unsigned_tx = Transaction::from_binary(&mut cursor).map_err(|e| {
            WalletError::Internal(format!(
                "Failed to parse stored unsigned transaction for reference '{}': {}",
                reference, e
            ))
        })?;

        // --- Step 6: Find all Output records spent by this transaction ---
        let spent_outputs = self
            .storage
            .find_outputs_storage(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        spent_by: Some(tx_record.transaction_id),
                        ..Default::default()
                    },
                    no_script: false,
                    ..Default::default()
                },
            )
            .await?;

        // --- Step 7: Build PendingStorageInput entries ---
        // For each input in the unsigned transaction, find the matching spent output
        // (matched by txid + vout) to get the derivation data needed for signing.
        let mut pdi: Vec<PendingStorageInput> = Vec::new();

        for (vin, input) in unsigned_tx.inputs.iter().enumerate() {
            let source_txid = input.source_txid.as_deref().unwrap_or("");
            let source_vout = input.source_output_index;

            // Find the matching spent output record
            let spent_output = spent_outputs
                .iter()
                .find(|o| o.txid.as_deref() == Some(source_txid) && o.vout == source_vout as i32);

            if let Some(output) = spent_output {
                // Only storage-provided (BRC-29/P2PKH) inputs need signing derivation data.
                // User-provided inputs with pre-supplied unlocking scripts don't need pdi entries
                // (they will be overridden by spends in ValidSignActionArgs).
                let derivation_prefix = output.derivation_prefix.clone().unwrap_or_default();
                let derivation_suffix = output.derivation_suffix.clone().unwrap_or_default();

                // Only add to pdi if this is a storage-provided input with derivation data
                if !derivation_prefix.is_empty() || !derivation_suffix.is_empty() {
                    // Get locking_script bytes, converting to hex for PendingStorageInput
                    let locking_script_hex = output
                        .locking_script
                        .as_ref()
                        .map(|b| {
                            b.iter()
                                .map(|byte| format!("{:02x}", byte))
                                .collect::<String>()
                        })
                        .unwrap_or_default();

                    pdi.push(PendingStorageInput {
                        vin: vin as u32,
                        derivation_prefix,
                        derivation_suffix,
                        unlocker_pub_key: output.sender_identity_key.clone(),
                        source_satoshis: output.satoshis as u64,
                        locking_script: locking_script_hex,
                    });
                }
            }
        }

        // --- Step 8: Build a minimal StorageCreateActionResult ---
        // Only input_beef is used by signer_sign_action (for build_beef_bytes).
        // Other fields are set to minimal defaults.
        let dcr = StorageCreateActionResult {
            reference: reference.to_string(),
            version: tx_record.version.unwrap_or(1) as u32,
            lock_time: tx_record.lock_time.unwrap_or(0) as u32,
            inputs: vec![],
            outputs: vec![],
            derivation_prefix: String::new(),
            input_beef,
            no_send_change_output_vouts: None,
        };

        // --- Step 9: Build a minimal ValidCreateActionArgs stub ---
        // args is not used by signer_sign_action, so defaults are fine.
        use bsv::wallet::interfaces::CreateActionOptions;
        let args_stub = ValidCreateActionArgs {
            description: tx_record.description,
            inputs: vec![],
            outputs: vec![],
            lock_time: dcr.lock_time,
            version: dcr.version,
            labels: vec![],
            options: CreateActionOptions::default(),
            input_beef: None,
            is_new_tx: true,
            is_sign_action: true,
            is_no_send: tx_record.status == TransactionStatus::Nosend,
            is_delayed: false,
            is_send_with: false,
        };

        Ok(PendingSignAction {
            reference: reference.to_string(),
            dcr,
            args: args_stub,
            tx: raw_tx_bytes,
            amount: 0, // not used by signer_sign_action
            pdi,
        })
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

    async fn sign_action(&self, args: ValidSignActionArgs) -> WalletResult<SignerSignActionResult> {
        // Look up the pending sign action from in-memory state.
        // If not found (e.g. after a process restart), recover from storage.
        let pending = {
            let mut map = self.pending_sign_actions.lock().await;
            map.remove(&args.reference)
        };

        let pending = match pending {
            Some(p) => p,
            None => self.recover_pending_sign_action(&args.reference).await?,
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

    async fn abort_action(&self, args: ValidAbortActionArgs) -> WalletResult<AbortActionResult> {
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
