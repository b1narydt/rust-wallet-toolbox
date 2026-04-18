//! Provider-based createAction — uses [`SigningProvider`] for async signing backends.
//!
//! This is the async counterpart to [`signer_create_action`] that accepts a
//! [`SigningProvider`] instead of a `CachedKeyDeriver` + `PublicKey` pair.
//! The flow is identical — storage create, build unsigned tx, sign or defer,
//! process action, broadcast — but all key derivation and signing goes through
//! the provider trait, enabling any backend to participate.
//!
//! [`signer_create_action`]: super::create_action::signer_create_action
//! [`SigningProvider`]: crate::signer::signing_provider::SigningProvider

use std::collections::HashMap;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::provider_signing::{
    build_signable_transaction_with_provider, complete_signed_transaction_with_provider,
};
use crate::signer::signing_provider::SigningProvider;
use crate::signer::types::{
    PendingSignAction, SignableTransactionRef, SignerCreateActionResult, ValidCreateActionArgs,
};
use crate::storage::action_types::StorageProcessActionArgs;
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;

// Re-use helpers from the original create_action module.
use super::create_action::{
    build_beef, build_beef_bytes, merge_input_beef_signer, serialize_beef_atomic, to_storage_args,
};

/// Execute the signer-level createAction flow using a [`SigningProvider`].
///
/// Same orchestration as [`signer_create_action`] but delegates key derivation
/// and signing to `provider`, enabling async signing backends (remote signers,
/// threshold protocols, etc.) to participate in the standard wallet pipeline.
///
/// [`signer_create_action`]: super::create_action::signer_create_action
pub async fn signer_create_action_with_provider(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    provider: &dyn SigningProvider,
    auth: &str,
    args: &ValidCreateActionArgs,
) -> WalletResult<(SignerCreateActionResult, Option<PendingSignAction>)> {
    // Step 1: Storage create action
    let auth_id = AuthId {
        identity_key: auth.to_string(),
        user_id: None,
        is_active: None,
    };
    let storage_args = to_storage_args(args);
    let mut dcr = storage.create_action(&auth_id, &storage_args).await?;
    merge_input_beef_signer(storage, &mut dcr).await?;
    let reference = dcr.reference.clone();

    // Step 2: Build unsigned transaction via provider (async key derivation)
    let (mut tx, amount, pdi) =
        build_signable_transaction_with_provider(&dcr, args, provider).await?;

    // Step 3a: Delayed signing — return signable reference for later sign_action
    if args.is_sign_action {
        let mut tx_bytes = Vec::new();
        tx.to_binary(&mut tx_bytes)
            .map_err(|e| WalletError::Internal(format!("Serialize unsigned tx: {e}")))?;

        let pending = PendingSignAction {
            reference: reference.clone(),
            dcr: dcr.clone(),
            args: args.clone(),
            tx: tx_bytes,
            amount,
            pdi: pdi.clone(),
        };

        let signable_beef = build_beef_bytes(&tx, &dcr.input_beef)?;

        let no_send_change = if args.is_no_send {
            let txid = tx
                .id()
                .map_err(|e| WalletError::Internal(format!("Compute txid: {e}")))?;
            dcr.no_send_change_output_vouts
                .as_ref()
                .map(|vouts| vouts.iter().map(|v| format!("{txid}.{v}")).collect())
                .unwrap_or_default()
        } else {
            vec![]
        };

        let result = SignerCreateActionResult {
            txid: None,
            tx: None,
            no_send_change,
            send_with_results: vec![],
            not_delayed_results: None,
            signable_transaction: Some(SignableTransactionRef {
                reference: reference.clone(),
                tx: signable_beef,
            }),
        };

        return Ok((result, Some(pending)));
    }

    // Step 3b: Immediate signing via provider
    let signed_tx_bytes =
        complete_signed_transaction_with_provider(&mut tx, &pdi, &HashMap::new(), provider).await?;

    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Compute txid: {e}")))?;

    // Build BEEF, verify unlock scripts, then serialize
    let beef = build_beef(&tx, &dcr.input_beef)?;
    crate::signer::verify_unlock_scripts::verify_unlock_scripts(&txid, &beef)?;
    let beef_bytes = serialize_beef_atomic(&beef, &txid)?;

    let no_send_change = if args.is_no_send {
        dcr.no_send_change_output_vouts
            .as_ref()
            .map(|vouts| vouts.iter().map(|v| format!("{txid}.{v}")).collect())
            .unwrap_or_default()
    } else {
        vec![]
    };

    // Step 4: Process action
    let process_args = StorageProcessActionArgs {
        is_new_tx: args.is_new_tx,
        is_send_with: args.is_send_with,
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        reference: Some(reference),
        txid: Some(txid.clone()),
        raw_tx: Some(signed_tx_bytes),
        send_with: if args.is_send_with {
            args.options.send_with.clone()
        } else {
            vec![]
        },
    };
    let process_result = storage.process_action(&auth_id, &process_args).await?;

    // Step 5: Broadcast and update status
    if !args.is_no_send && !args.is_delayed {
        let post_results = services
            .post_beef(&beef_bytes, std::slice::from_ref(&txid))
            .await;

        let outcome = crate::signer::broadcast_outcome::classify_broadcast_results(&post_results);

        match &outcome {
            crate::signer::broadcast_outcome::BroadcastOutcome::Success
            | crate::signer::broadcast_outcome::BroadcastOutcome::OrphanMempool { .. } => {
                if let Err(e) = crate::signer::broadcast_outcome::apply_success_or_orphan_outcome(
                    storage, &txid, &outcome,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        txid = %txid,
                        "createAction: failed to update status after successful broadcast"
                    );
                }
            }
            crate::signer::broadcast_outcome::BroadcastOutcome::DoubleSpend { .. }
            | crate::signer::broadcast_outcome::BroadcastOutcome::InvalidTx { .. } => {
                if let Err(e) =
                    crate::signer::broadcast_outcome::handle_permanent_broadcast_failure(
                        storage, services, &txid, &outcome,
                    )
                    .await
                {
                    tracing::error!(
                        error = %e,
                        txid = %txid,
                        "createAction: permanent failure handler errored"
                    );
                }
            }
            crate::signer::broadcast_outcome::BroadcastOutcome::ServiceError { details } => {
                if let Err(e) = crate::signer::broadcast_outcome::apply_service_error_outcome(
                    storage,
                    &txid,
                    details.clone(),
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        txid = %txid,
                        "createAction: service error retry transition failed"
                    );
                }
            }
        }
    }

    let result = SignerCreateActionResult {
        txid: Some(txid),
        tx: if args.options.return_txid_only.0.unwrap_or(false) {
            None
        } else {
            Some(beef_bytes)
        },
        no_send_change,
        send_with_results: process_result.send_with_results.unwrap_or_default(),
        not_delayed_results: process_result.not_delayed_results,
        signable_transaction: None,
    };

    Ok((result, None))
}
