//! Signer-level createAction orchestration.
//!
//! Ported from wallet-toolbox/src/signer/methods/createAction.ts.
//! Coordinates storage.create_action, build_signable_transaction,
//! complete_signed_transaction, storage.process_action, and BEEF construction.

use std::collections::HashMap;

use bsv::transaction::Beef;

use crate::error::{WalletError, WalletResult};
use crate::services::traits::WalletServices;
use crate::signer::backend::SigningBackend;
use crate::signer::build_signable::build_signable_transaction;
use crate::signer::complete_signed::complete_signed_transaction;
use crate::signer::provider_signing::{
    build_signable_transaction_with_provider, complete_signed_transaction_with_provider,
};
use crate::signer::types::{
    PendingSignAction, SignableTransactionRef, SignerCreateActionResult, ValidCreateActionArgs,
};
use crate::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionInput, StorageCreateActionOptions,
    StorageCreateActionOutput, StorageOutPoint, StorageProcessActionArgs,
};
use crate::storage::manager::WalletStorageManager;
use crate::wallet::types::AuthId;

/// Simple bytes-to-hex encoding.
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse an OutpointString ("txid.vout") into a StorageOutPoint.
fn parse_outpoint_string(s: &str) -> StorageOutPoint {
    let parts: Vec<&str> = s.rsplitn(2, '.').collect();
    if parts.len() == 2 {
        StorageOutPoint {
            txid: parts[1].to_string(),
            vout: parts[0].parse().unwrap_or(0),
        }
    } else {
        StorageOutPoint {
            txid: s.to_string(),
            vout: 0,
        }
    }
}

/// Convert ValidCreateActionArgs to StorageCreateActionArgs.
///
/// Strips unlocking scripts (storage only needs the length).
/// Maps SDK CreateActionOptions to StorageCreateActionOptions.
pub(crate) fn to_storage_args(args: &ValidCreateActionArgs) -> StorageCreateActionArgs {
    use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue};

    let opts = &args.options;
    let storage_options = StorageCreateActionOptions {
        sign_and_process: BooleanDefaultTrue(opts.sign_and_process.0),
        accept_delayed_broadcast: BooleanDefaultTrue(opts.accept_delayed_broadcast.0),
        trust_self: opts.trust_self.as_ref().map(|ts| ts.as_str().to_string()),
        known_txids: opts.known_txids.clone(),
        return_txid_only: BooleanDefaultFalse(opts.return_txid_only.0),
        no_send: BooleanDefaultFalse(opts.no_send.0),
        no_send_change: opts
            .no_send_change
            .iter()
            .map(|s| parse_outpoint_string(s))
            .collect(),
        send_with: opts.send_with.clone(),
        randomize_outputs: BooleanDefaultTrue(opts.randomize_outputs.0),
    };

    StorageCreateActionArgs {
        description: args.description.clone(),
        inputs: args
            .inputs
            .iter()
            .map(|i| StorageCreateActionInput {
                outpoint: StorageOutPoint {
                    txid: i.outpoint.txid.clone(),
                    vout: i.outpoint.vout,
                },
                input_description: i.input_description.clone(),
                unlocking_script_length: i
                    .unlocking_script
                    .as_ref()
                    .map(|s| s.len())
                    .unwrap_or(i.unlocking_script_length),
                sequence_number: i.sequence_number,
            })
            .collect(),
        outputs: args
            .outputs
            .iter()
            .map(|o| {
                let script_hex = o
                    .locking_script
                    .as_ref()
                    .map(|s| bytes_to_hex(s))
                    .unwrap_or_default();
                StorageCreateActionOutput {
                    locking_script: script_hex,
                    satoshis: o.satoshis,
                    output_description: o.output_description.clone(),
                    basket: o.basket.clone(),
                    custom_instructions: o.custom_instructions.clone(),
                    tags: o.tags.clone(),
                }
            })
            .collect(),
        lock_time: args.lock_time,
        version: args.version,
        labels: args.labels.iter().map(|l| l.to_string()).collect(),
        options: storage_options,
        input_beef: args.input_beef.clone(),
        is_new_tx: args.is_new_tx,
        is_sign_action: args.is_sign_action,
        is_no_send: args.is_no_send,
        is_delayed: args.is_delayed,
        is_send_with: args.is_send_with,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    }
}

/// Execute the signer-level createAction flow.
///
/// 1. Call storage.create_action to allocate UTXOs and create records
/// 2. Build the unsigned transaction, deriving change locking scripts via `backend`
/// 3. Branch based on whether this is a deferred sign action:
///    - If `is_sign_action`: store `PendingSignAction`, return `SignableTransaction`.
///    - Otherwise: sign the inputs via `backend`, process, optionally broadcast.
///
/// `backend` selects the custody mode. [`SigningBackend::Local`] derives and
/// signs in-process from a root private key; [`SigningBackend::Delegated`]
/// hands both to a [`SigningProvider`], so no root key is touched anywhere in
/// this flow.
///
/// `ctx` is who the action is made on behalf of — the transport-authenticated
/// caller, or the wallet itself. It reaches the provider's ceremony-driving
/// methods; `auth` is distinct — it is the wallet's own identity, scoping the
/// storage rows.
///
/// [`SigningProvider`]: crate::signer::signing_provider::SigningProvider
pub async fn signer_create_action(
    storage: &WalletStorageManager,
    services: &(dyn WalletServices + Send + Sync),
    backend: &SigningBackend<'_>,
    auth: &str,
    args: &ValidCreateActionArgs,
    ctx: &crate::signer::signing_context::SigningContext,
) -> WalletResult<(SignerCreateActionResult, Option<PendingSignAction>)> {
    // --- Step 1: Storage create action ---
    let auth_id = AuthId {
        identity_key: auth.to_string(),
        user_id: None,
        is_active: None,
    };
    let storage_args = to_storage_args(args);
    // The spend lock serializes UTXO allocation across concurrent callers.
    // It covers exactly the allocating call (and the read-back of what was
    // allocated) — NOT signing or broadcast, which can take seconds over the
    // network and would stall every other spend-path caller.
    let dcr = {
        let _spend_guard = storage.acquire_spend_lock().await?;
        let mut dcr = storage.create_action(&auth_id, &storage_args).await?;
        merge_input_beef_signer(storage, &mut dcr).await?;
        dcr
    };
    let reference = dcr.reference.clone();

    // --- Step 2: Build unsigned transaction ---
    // Change locking scripts are the first custody decision: locally derived
    // from the root key, or derived by the provider.
    let (mut tx, amount, pdi) = match backend {
        SigningBackend::Local {
            key_deriver,
            identity_pub_key,
        } => build_signable_transaction(&dcr, args, *key_deriver, identity_pub_key)?,
        SigningBackend::Delegated(provider) => {
            build_signable_transaction_with_provider(&dcr, args, *provider, ctx).await?
        }
    };

    // --- Step 3a: Delayed signing (is_sign_action) ---
    if args.is_sign_action {
        // Serialize unsigned tx for later recovery
        let mut tx_bytes = Vec::new();
        tx.to_binary(&mut tx_bytes)
            .map_err(|e| WalletError::Internal(format!("Failed to serialize unsigned tx: {e}")))?;

        let pending = PendingSignAction {
            reference: reference.clone(),
            dcr: dcr.clone(),
            args: args.clone(),
            tx: tx_bytes.clone(),
            amount,
            pdi: pdi.clone(),
        };

        // Build signable transaction BEEF for the caller
        let signable_beef = build_beef_bytes(&tx, &dcr.input_beef)?;

        // The unsigned transaction's txid — the SAME value the TS reference
        // computes here (`prior.tx.id('hex')`, used for noSendChange
        // outpoints). Persisted on the transaction row so a `listActions` row
        // for a still-'unsigned' action carries a real 64-hex txid instead of
        // an empty string (the BRC-100 `Action` row requires one).
        // `process_action` overwrites it with the final txid when signAction
        // completes, exactly as it already overwrites `status`.
        let unsigned_txid = tx
            .id()
            .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {e}")))?;
        {
            use crate::storage::find_args::{FindTransactionsArgs, TransactionPartial};
            let rows = storage
                .find_transactions(&FindTransactionsArgs {
                    partial: TransactionPartial {
                        reference: Some(reference.clone()),
                        ..Default::default()
                    },
                    no_raw_tx: true,
                    ..Default::default()
                })
                .await?;
            if let Some(row) = rows.first() {
                storage
                    .update_transaction(
                        row.transaction_id,
                        &TransactionPartial {
                            txid: Some(unsigned_txid.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
        }

        let no_send_change = if args.is_no_send {
            dcr.no_send_change_output_vouts
                .as_ref()
                .map(|vouts| {
                    vouts
                        .iter()
                        .map(|v| format!("{unsigned_txid}.{v}"))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let result = SignerCreateActionResult {
            txid: None,
            tx: None,
            no_send_change,
            send_with_results: vec![],
            signable_transaction: Some(SignableTransactionRef {
                reference: reference.clone(),
                tx: signable_beef,
            }),
            not_delayed_results: None,
        };

        return Ok((result, Some(pending)));
    }

    // --- Step 3b: Immediate signing ---
    let signed_tx_bytes = match backend {
        SigningBackend::Local {
            key_deriver,
            identity_pub_key,
        } => {
            complete_signed_transaction(
                &mut tx,
                &pdi,
                &HashMap::new(),
                *key_deriver,
                identity_pub_key,
            )
            .await?
        }
        SigningBackend::Delegated(provider) => {
            // Let the provider capture per-input spend context before it is
            // asked for signatures (no-op by default).
            provider.prepare_spend_contexts(&tx, &pdi, ctx).await?;
            complete_signed_transaction_with_provider(
                &mut tx,
                &pdi,
                &HashMap::new(),
                *provider,
                ctx,
            )
            .await?
        }
    };

    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {e}")))?;

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

    // --- Step 4: Process action in storage ---
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
    // process_action's status check-then-act (find by reference, verify
    // Unsigned/Unprocessed, then flip) is not atomic on its own; the spend
    // lock serializes it against a concurrent duplicate submission. Released
    // before broadcast — the network must never run under this lock.
    let process_result = {
        let _spend_guard = storage.acquire_spend_lock().await?;
        storage.process_action(&auth_id, &process_args).await?
    };
    // --- Step 5: Broadcast and update status ---
    // In the TS implementation, shareReqsWithWorld handles both broadcast
    // and the post-broadcast status update. The initial status from
    // processAction is unprocessed/unprocessed. After successful broadcast,
    // status should transition to unmined/unproven (or sending/unproven
    // for the non-delayed case). Without this update, outputs remain
    // invisible to the balance query and UTXO selection.
    if !args.is_no_send && !args.is_delayed {
        let post_results = services
            .post_beef(&beef_bytes, std::slice::from_ref(&txid))
            .await;

        let outcome = crate::signer::broadcast_outcome::classify_broadcast_results(&post_results);

        match &outcome {
            crate::signer::broadcast_outcome::BroadcastOutcome::Success
            | crate::signer::broadcast_outcome::BroadcastOutcome::OrphanMempool { .. } => {
                // Success or transient orphan — transition to unproven/unmined.
                // OrphanMempool stays in sending for monitor retry.
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
                // Permanent failure — mark tx failed, outputs unspendable, reconcile against chain.
                // Log any handler error for ops visibility; do not propagate — the broadcast
                // outcome we already have remains authoritative for the caller.
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
                // Transition tx+req back to Sending and bump attempts so
                // TaskSendWaiting will actually retry. Log any handler error
                // but don't propagate — the broadcast already happened and the
                // caller's contract is to return the classified outcome.
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
        signable_transaction: None,
        not_delayed_results: process_result.not_delayed_results,
    };

    Ok((result, None))
}

/// Build a Beef object containing the transaction and its input proofs.
///
/// Constructs a Beef by:
/// 1. Merging input_beef if available (contains source txs with proofs)
/// 2. Recursively merging every input's `source_transaction` graph not
///    already represented (the TS `mergeTransaction` recursion — flat
///    `merge_raw_tx` alone ignored `tx.inputs[*].source_transaction` and
///    produced BEEFs that violated the BRC-95 closure, rust-mpc#352)
/// 3. Merging the signed/unsigned transaction itself via merge_raw_tx
///
/// Fails closed: an input whose source transaction is neither in the BEEF
/// nor carried (genuinely — see `merge_input_ancestry`) on the input errors
/// naming the txid. This is defence-in-depth behind `merge_input_beef_signer`,
/// which is responsible for making `input_beef` cover every input.
pub(crate) fn build_beef(
    tx: &bsv::transaction::transaction::Transaction,
    input_beef: &Option<Vec<u8>>,
) -> WalletResult<Beef> {
    let mut beef = Beef::new(bsv::transaction::beef::BEEF_V1);
    if let Some(ref input_beef_bytes) = input_beef {
        if !input_beef_bytes.is_empty() {
            beef.merge_beef_from_binary(input_beef_bytes)
                .map_err(|e| WalletError::Internal(format!("Failed to merge input BEEF: {e}")))?;
        }
    }

    // Ancestors first, subject last, so dependency order holds without a
    // sort and `to_binary_atomic`'s truncation-after-subject keeps them.
    merge_input_ancestry(&mut beef, tx)?;

    let mut raw_tx = Vec::new();
    tx.to_binary(&mut raw_tx)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize tx: {e}")))?;
    beef.merge_raw_tx(&raw_tx, None)
        .map_err(|e| WalletError::Internal(format!("Failed to merge raw tx: {e}")))?;

    Ok(beef)
}

/// Recursively merge the `source_transaction` graph of `tx`'s inputs into
/// `beef` — the toolbox port of TS `Beef.mergeTransaction`'s
/// sourceTransaction recursion (@bsv/sdk Beef.ts), which the Rust SDK's
/// `Beef` does not yet provide.
///
/// TODO(bsv-sdk): upstream as `Beef::merge_transaction` so callers get the
/// TS method directly; this helper then reduces to that call plus the
/// fail-closed closure check.
///
/// Deliberate deviations from TS, both narrowing:
///   * sources already represented in the BEEF are not re-merged (dedup;
///     the resulting closure is identical), and
///   * a branch that cannot be completed FAILS CLOSED here, where TS defers
///     the hole to `verifyValid` at consumption time — a returned BEEF must
///     be complete without relying on later rehydration (one-shot processes
///     exit before any monitor runs; the live rust-mpc#352 failure mode).
///
/// For each input: if its source txid is already represented in the BEEF,
/// done. Otherwise the input must carry a genuine `source_transaction` —
/// one whose computed txid matches the input's `source_txid`; the synthetic
/// output-padded stubs `complete_signed_transaction` fabricates for signing
/// hash their own (wrong) txid and are deliberately NOT merged, because a
/// stub cannot satisfy the closure. A genuine source with a `merkle_path`
/// terminates its branch (bump merged alongside); one without recurses into
/// its own inputs first. A branch ending with neither a usable source nor
/// BEEF representation is an error naming the txid — fail closed, never a
/// partial BEEF.
fn merge_input_ancestry(
    beef: &mut Beef,
    tx: &bsv::transaction::transaction::Transaction,
) -> WalletResult<()> {
    for input in &tx.inputs {
        let Some(source_txid) = input.source_txid.as_ref().filter(|t| !t.is_empty()) else {
            continue;
        };
        if beef.find_txid(source_txid).is_some() {
            continue;
        }

        let genuine_source = input
            .source_transaction
            .as_deref()
            .filter(|src| src.id().is_ok_and(|computed| computed == *source_txid));
        let Some(src) = genuine_source else {
            return Err(WalletError::Internal(format!(
                "BEEF closure incomplete: input source transaction {source_txid} is not in \
                 the assembled BEEF and the input carries no source transaction for it \
                 (fail closed rather than return a partial BEEF)"
            )));
        };

        let mut src_bytes = Vec::new();
        src.to_binary(&mut src_bytes).map_err(|e| {
            WalletError::Internal(format!(
                "Failed to serialize source transaction {source_txid}: {e}"
            ))
        })?;

        if let Some(ref merkle_path) = src.merkle_path {
            // Proven ancestor: merge its bump and terminate this branch.
            let bump_index = beef.merge_bump(merkle_path).map_err(|e| {
                WalletError::Internal(format!(
                    "Failed to merge merkle path for source transaction {source_txid}: {e}"
                ))
            })?;
            beef.merge_raw_tx(&src_bytes, Some(bump_index))
                .map_err(|e| {
                    WalletError::Internal(format!(
                        "Failed to merge source transaction {source_txid}: {e}"
                    ))
                })?;
        } else {
            // Unproven ancestor: its own ancestry first, then itself.
            merge_input_ancestry(beef, src)?;
            beef.merge_raw_tx(&src_bytes, None).map_err(|e| {
                WalletError::Internal(format!(
                    "Failed to merge source transaction {source_txid}: {e}"
                ))
            })?;
        }
    }
    Ok(())
}

/// Serialize a Beef as Atomic BEEF bytes targeting a specific txid.
///
/// Port of TS `Beef.toBinaryAtomic` (@bsv/sdk Beef.ts) — BRC-95 semantics,
/// byte-verified against the TS-golden vectors
/// (conformance/vectors/beef/beef_atomic_closure.json + beef_spend_closure.json):
///   * the emitted BEEF is the subject's DEPENDENCY CLOSURE only —
///     transactions unrelated to the subject are dropped;
///   * bumps not referenced by a retained transaction are pruned, and
///     retained bump indices are re-derived;
///   * ancestors precede dependents (the TS SDK's `toBinary` sorts
///     implicitly; the Rust SDK's `to_binary_atomic` instead truncates after
///     the subject in insertion order, which both keeps unrelated txs and
///     can silently drop late-merged ancestors — so the closure is computed
///     here, not delegated).
///
/// TODO(bsv-sdk): upstream this as the conformant `Beef::to_binary_atomic`;
/// the SDK's current truncate-after-subject behavior is nonconformant with
/// the normative TS output.
pub fn serialize_beef_atomic(beef: &Beef, txid: &str) -> WalletResult<Vec<u8>> {
    use std::collections::HashSet;

    if beef.find_txid(txid).is_none() {
        return Err(WalletError::Internal(format!(
            "Failed to serialize Atomic BEEF: {txid} does not exist in this Beef"
        )));
    }

    // Dependency closure of the subject over the txs present in this BEEF.
    let mut closure: HashSet<&str> = HashSet::new();
    let mut frontier: Vec<&str> = vec![txid];
    while let Some(t) = frontier.pop() {
        if !closure.insert(t) {
            continue;
        }
        if let Some(btx) = beef.find_txid(t) {
            for input_txid in &btx.input_txids {
                if beef.find_txid(input_txid).is_some() {
                    frontier.push(input_txid.as_str());
                }
            }
        }
    }

    // Rebuild: closure txs in dependency order, referenced bumps re-indexed
    // in first-use order.
    let mut sorted = beef.clone();
    sorted.sort_txs();

    let mut atomic = Beef::new(beef.version);
    atomic.atomic_txid = Some(txid.to_string());
    let mut bump_map: Vec<Option<usize>> = vec![None; beef.bumps.len()];
    for btx in &sorted.txs {
        if !closure.contains(btx.txid.as_str()) {
            continue;
        }
        let mut new_btx = btx.clone();
        if let Some(old_idx) = btx.bump_index {
            let slot = bump_map.get_mut(old_idx).ok_or_else(|| {
                WalletError::Internal(format!(
                    "Failed to serialize Atomic BEEF: {} names bump {old_idx} which does not exist",
                    btx.txid
                ))
            })?;
            let new_idx = match slot {
                Some(i) => *i,
                None => {
                    let i = atomic.bumps.len();
                    atomic.bumps.push(sorted.bumps[old_idx].clone());
                    *slot = Some(i);
                    i
                }
            };
            new_btx.bump_index = Some(new_idx);
        }
        atomic.txs.push(new_btx);
    }

    let mut buf = Vec::new();
    atomic
        .to_binary(&mut buf)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize Atomic BEEF: {e}")))?;
    Ok(buf)
}

/// Build Atomic BEEF bytes containing the transaction and its input proofs.
///
/// Convenience wrapper around `build_beef` + `serialize_beef_atomic`.
/// Used for the signable BEEF path (delayed signing) where verification
/// is NOT done since the tx is unsigned.
pub(crate) fn build_beef_bytes(
    tx: &bsv::transaction::transaction::Transaction,
    input_beef: &Option<Vec<u8>>,
) -> WalletResult<Vec<u8>> {
    let txid = tx
        .id()
        .map_err(|e| WalletError::Internal(format!("Failed to compute txid: {e}")))?;
    let beef = build_beef(tx, input_beef)?;
    serialize_beef_atomic(&beef, &txid)
}

/// Populate `dcr.input_beef` with BEEF proof data for ALL inputs.
///
/// Without this, the outgoing BEEF won't include source tx proofs and
/// recipients/miners will reject it during SPV verification.
///
/// Covers every input regardless of `providedBy` — a caller-named input
/// (`providedBy: you`) spending a wallet-known output (e.g. a minted token
/// spent by outpoint) needs its ancestor chain exactly as storage-allocated
/// change does; gating on `providedBy == storage` was the rust-mpc#352
/// defect (returned Atomic BEEF missing the caller-named input's parent,
/// violating the BRC-95 closure). Fail closed: an input whose ancestry is
/// neither already in the assembled BEEF nor producible from storage is an
/// error naming the txid — never a silently partial BEEF.
///
/// Also merges any stored `inputBEEF` from source transactions (which may
/// contain the original sender's BEEF chain, needed for full verification
/// when our UTXOs came from received payments).
pub(crate) async fn merge_input_beef_signer(
    storage: &WalletStorageManager,
    dcr: &mut crate::storage::action_types::StorageCreateActionResult,
) -> WalletResult<()> {
    use crate::storage::beef::{get_valid_beef_for_txid, TrustSelf};
    use bsv::transaction::beef::{Beef, BEEF_V2};
    use std::collections::HashSet;

    let active = match storage.active() {
        Some(a) => a.clone(),
        None => return Ok(()),
    };

    let mut beef = Beef::new(BEEF_V2);

    // Merge the base input_beef from storage — this is the foundation.
    if let Some(ref ib) = dcr.input_beef {
        if !ib.is_empty() {
            beef.merge_beef_from_binary(ib).map_err(|e| {
                WalletError::Internal(format!(
                    "Failed to merge base input BEEF ({} bytes): {e}",
                    ib.len()
                ))
            })?;
        }
    }

    // Merge BEEF for every input not already represented in the assembled
    // BEEF (the caller's inputBEEF is the base, merged above). TrustSelf::No:
    // the walk must produce full raw txs + proofs, never trust-elided
    // txid-only entries (#326 posture).
    let mut known_txids: HashSet<String> = HashSet::new();
    for input in &dcr.inputs {
        let txid = &input.source_txid;
        if !txid.is_empty() && beef.find_txid(txid).is_none() {
            let tx_beef_bytes =
                get_valid_beef_for_txid(&*active, txid, TrustSelf::No, &known_txids)
                    .await
                    .map_err(|e| {
                        WalletError::Internal(format!("Failed to fetch BEEF for input {txid}: {e}"))
                    })?
                    .ok_or_else(|| {
                        WalletError::Internal(format!(
                            "No BEEF ancestry available for input {txid}: not in the provided \
                         inputBEEF and storage cannot produce its transaction (fail closed \
                         rather than return a partial BEEF)"
                        ))
                    })?;

            beef.merge_beef_from_binary(&tx_beef_bytes).map_err(|e| {
                WalletError::Internal(format!("Failed to merge BEEF for input {txid}: {e}"))
            })?;

            known_txids.insert(txid.clone());
        }
    }

    // Merge any stored inputBEEF from source transactions.
    // When UTXOs came from received payments, the sender's proof chain
    // is stored as inputBEEF on the source transaction record.
    for input in &dcr.inputs {
        if !input.source_txid.is_empty() {
            let txs = active
                .find_transactions(&crate::storage::find_args::FindTransactionsArgs {
                    partial: crate::storage::find_args::TransactionPartial {
                        txid: Some(input.source_txid.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .await
                .map_err(|e| {
                    WalletError::Internal(format!(
                        "Failed to look up source tx {}: {e}",
                        input.source_txid
                    ))
                })?;

            for tx_rec in &txs {
                if let Some(ref ib) = tx_rec.input_beef {
                    if !ib.is_empty() {
                        beef.merge_beef_from_binary(ib).map_err(|e| {
                            WalletError::Internal(format!(
                                "Failed to merge stored inputBEEF for {}: {e}",
                                input.source_txid
                            ))
                        })?;
                    }
                }
            }
        }
    }

    // Serialize back
    if beef.txs.is_empty() {
        dcr.input_beef = None;
    } else {
        let mut buf = Vec::new();
        beef.to_binary(&mut buf).map_err(|e| {
            WalletError::Internal(format!(
                "Failed to serialize merged BEEF ({} txs): {e}",
                beef.txs.len()
            ))
        })?;
        dcr.input_beef = Some(buf);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use bsv::wallet::types::BooleanDefaultFalse;

    mod build_beef_ancestry {
        use crate::signer::methods::create_action::{build_beef, serialize_beef_atomic};
        use bsv::script::locking_script::LockingScript;
        use bsv::transaction::transaction::Transaction;
        use bsv::transaction::transaction_input::TransactionInput;
        use bsv::transaction::transaction_output::TransactionOutput;
        use bsv::transaction::Beef;

        fn simple_tx(satoshis: u64) -> Transaction {
            let mut tx = Transaction::new();
            tx.add_output(TransactionOutput {
                satoshis: Some(satoshis),
                locking_script: LockingScript::from_binary(&[0x51]),
                change: false,
            });
            tx
        }

        fn spend_of(parent: &Transaction, carry_source: bool) -> Transaction {
            let mut tx = Transaction::new();
            tx.inputs.push(TransactionInput {
                source_transaction: carry_source.then(|| Box::new(parent.clone())),
                source_txid: Some(parent.id().unwrap()),
                source_output_index: 0,
                unlocking_script: Some(
                    bsv::script::unlocking_script::UnlockingScript::from_binary(&[0x51]),
                ),
                sequence: 0xffffffff,
            });
            tx.add_output(TransactionOutput {
                satoshis: Some(1),
                locking_script: LockingScript::from_binary(&[0x51]),
                change: false,
            });
            tx
        }

        /// A source_transaction carried on the input must be merged into the
        /// BEEF (the TS mergeTransaction recursion), grandparents included.
        #[test]
        fn merges_carried_source_transaction_graph() {
            let grandparent = simple_tx(3);
            let mut parent = spend_of(&grandparent, true);
            parent.outputs[0].satoshis = Some(2);
            let child = spend_of(&parent, true);

            let beef = build_beef(&child, &None).expect("build_beef");
            for tx in [&grandparent, &parent, &child] {
                let txid = tx.id().unwrap();
                let btx = beef
                    .find_txid(&txid)
                    .unwrap_or_else(|| panic!("{txid} missing from beef"));
                assert!(!btx.is_txid_only(), "{txid} must be a full transaction");
            }

            // And the atomic serialization keeps the whole closure.
            let atomic = serialize_beef_atomic(&beef, &child.id().unwrap()).unwrap();
            let mut cursor = std::io::Cursor::new(&atomic);
            let parsed = Beef::from_binary(&mut cursor).unwrap();
            assert!(parsed.find_txid(&grandparent.id().unwrap()).is_some());
            assert!(parsed.find_txid(&parent.id().unwrap()).is_some());
        }

        /// An input whose source is neither in the BEEF nor carried on the
        /// input fails closed, naming the txid.
        #[test]
        fn missing_ancestry_errors_naming_the_txid() {
            let parent = simple_tx(2);
            let child = spend_of(&parent, false);
            let parent_txid = parent.id().unwrap();

            let err = build_beef(&child, &None).expect_err("must fail closed");
            assert!(
                err.to_string().contains(&parent_txid),
                "error must name the missing txid, got: {err}"
            );
        }

        /// The synthetic output-padded source stubs that
        /// complete_signed_transaction fabricates (whose computed txid does
        /// not match the input's source_txid) must not be merged — and since
        /// they cannot satisfy the closure, the build fails closed.
        #[test]
        fn synthetic_source_stub_is_not_merged() {
            let parent = simple_tx(2);
            let mut child = spend_of(&parent, false);
            // Fabricate the stub the signer builds: right output shape,
            // wrong txid.
            let mut stub = Transaction::new();
            stub.add_output(TransactionOutput {
                satoshis: Some(2),
                locking_script: LockingScript::from_binary(&[0x51]),
                change: false,
            });
            stub.add_output(TransactionOutput {
                satoshis: Some(0),
                locking_script: LockingScript::from_binary(&[]),
                change: false,
            });
            child.inputs[0].source_transaction = Some(Box::new(stub.clone()));
            let parent_txid = parent.id().unwrap();
            assert_ne!(stub.id().unwrap(), parent_txid);

            let err = build_beef(&child, &None).expect_err("stub must not satisfy closure");
            assert!(
                err.to_string().contains(&parent_txid),
                "error must name the real missing txid, got: {err}"
            );
        }

        /// Sources already present in the input BEEF are not re-merged and
        /// do not require a carried source_transaction.
        #[test]
        fn source_in_input_beef_is_sufficient() {
            let parent = simple_tx(2);
            let child = spend_of(&parent, false);

            let mut input_beef = Beef::new(bsv::transaction::beef::BEEF_V1);
            let mut parent_bytes = Vec::new();
            parent.to_binary(&mut parent_bytes).unwrap();
            input_beef.merge_raw_tx(&parent_bytes, None).unwrap();
            let mut ib_bytes = Vec::new();
            input_beef.to_binary(&mut ib_bytes).unwrap();

            let beef = build_beef(&child, &Some(ib_bytes)).expect("build_beef");
            assert!(beef.find_txid(&parent.id().unwrap()).is_some());
            assert!(beef.find_txid(&child.id().unwrap()).is_some());
        }
    }

    #[test]
    fn test_return_txid_only_controls_tx_field() {
        let return_txid_only = BooleanDefaultFalse(Some(true));
        let beef_bytes = vec![1, 2, 3];
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) {
            None
        } else {
            Some(beef_bytes.clone())
        };
        assert!(tx.is_none());

        let return_txid_only = BooleanDefaultFalse(Some(false));
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) {
            None
        } else {
            Some(beef_bytes.clone())
        };
        assert!(tx.is_some());

        // Default (None) should behave as false — tx is included
        let return_txid_only = BooleanDefaultFalse(None);
        let tx: Option<Vec<u8>> = if return_txid_only.0.unwrap_or(false) {
            None
        } else {
            Some(beef_bytes)
        };
        assert!(tx.is_some());
    }

    #[test]
    fn test_send_with_conditional() {
        let send_with_txids = vec!["aabb".to_string(), "ccdd".to_string()];

        let is_send_with = true;
        let result: Vec<String> = if is_send_with {
            send_with_txids.clone()
        } else {
            vec![]
        };
        assert_eq!(result.len(), 2);

        let is_send_with = false;
        let result: Vec<String> = if is_send_with {
            send_with_txids
        } else {
            vec![]
        };
        assert!(result.is_empty());
    }
}
