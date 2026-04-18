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
use super::create_action::{build_beef_bytes, merge_input_beef_signer, to_storage_args};

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

    let beef_bytes = build_beef_bytes(&tx, &dcr.input_beef)?;

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
        send_with: vec![],
    };
    let process_result = storage.process_action(&auth_id, &process_args).await?;

    // Step 5: Broadcast if applicable
    if !args.is_no_send && !args.is_delayed {
        let reqs = storage
            .find_proven_tx_reqs(&crate::storage::find_args::FindProvenTxReqsArgs {
                partial: crate::storage::find_args::ProvenTxReqPartial {
                    txid: Some(txid.clone()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .map_err(|e| {
                tracing::error!(
                    txid = %txid,
                    error = %e,
                    "createAction broadcast: lookup of newly-created req failed"
                );
                e
            })?;

        match reqs.into_iter().next() {
            Some(req) => {
                if let Err(e) = crate::monitor::helpers::attempt_to_post_reqs_to_network(
                    storage, services, &[req],
                )
                .await
                {
                    tracing::error!(
                        txid = %txid,
                        error = %e,
                        "createAction broadcast: attempt_to_post_reqs_to_network failed"
                    );
                    return Err(e);
                }
            }
            None => {
                tracing::warn!(
                    txid = %txid,
                    "createAction broadcast: could not find newly-created req"
                );
            }
        }
    }

    let result = SignerCreateActionResult {
        txid: Some(txid),
        tx: Some(beef_bytes),
        no_send_change,
        send_with_results: process_result.send_with_results.unwrap_or_default(),
        not_delayed_results: process_result.not_delayed_results,
        signable_transaction: None,
    };

    Ok((result, None))
}
