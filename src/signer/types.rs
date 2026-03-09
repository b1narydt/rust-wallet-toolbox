//! Types for the signer pipeline.
//!
//! Defines validated argument types, pending sign action state, and result types
//! used by the WalletSigner trait. These types bridge the bsv-sdk wallet interface
//! types and the storage action types.

use serde::{Deserialize, Serialize};

use bsv::wallet::interfaces::{
    CreateActionOptions, CreateActionOutput, InternalizeOutput, SendWithResult, SignActionOptions,
    SignActionSpend,
};
use bsv::wallet::types::{
    DescriptionString5to50Bytes, LabelStringUnder300Bytes, OutpointString, TXIDHexString,
};

use crate::storage::action_types::StorageCreateActionResult;

/// Default fee rate in satoshis per kilobyte.
pub const DEFAULT_FEE_RATE_SAT_PER_KB: u64 = 100;

// ---------------------------------------------------------------------------
// Validated argument types (received from Wallet layer after validation)
// ---------------------------------------------------------------------------

/// Validated create action arguments.
///
/// The Wallet layer validates and normalizes CreateActionArgs before passing
/// to the signer. This struct carries the validated data.
#[derive(Debug, Clone)]
pub struct ValidCreateActionArgs {
    /// Validated action description (5-50 bytes).
    pub description: DescriptionString5to50Bytes,
    /// Validated input specifications.
    pub inputs: Vec<ValidCreateActionInput>,
    /// Output specifications from the SDK.
    pub outputs: Vec<CreateActionOutput>,
    /// Transaction lock time.
    pub lock_time: u32,
    /// Transaction version.
    pub version: u32,
    /// Validated labels for this action.
    pub labels: Vec<LabelStringUnder300Bytes>,
    /// Create action options (noSend, signAndProcess, etc.).
    pub options: CreateActionOptions,
    /// BEEF data for input validity proofs.
    pub input_beef: Option<Vec<u8>>,
    /// True if this is a new transaction (not a sign-only operation).
    pub is_new_tx: bool,
    /// True if this should produce a SignableTransaction instead of signing immediately.
    pub is_sign_action: bool,
    /// True if the transaction should not be broadcast.
    pub is_no_send: bool,
    /// True if broadcasting should be deferred.
    pub is_delayed: bool,
    /// True if this is a send-with batch operation.
    pub is_send_with: bool,
}

/// A validated create action input with resolved outpoint and script info.
#[derive(Debug, Clone)]
pub struct ValidCreateActionInput {
    /// Parsed outpoint (txid + vout).
    pub outpoint: OutpointInfo,
    /// Human-readable description of this input.
    pub input_description: String,
    /// Pre-provided unlocking script bytes, if any.
    pub unlocking_script: Option<Vec<u8>>,
    /// Expected unlocking script length for size estimation.
    pub unlocking_script_length: usize,
    /// nSequence value for this input.
    pub sequence_number: u32,
}

/// Parsed outpoint reference.
#[derive(Debug, Clone)]
pub struct OutpointInfo {
    /// Transaction ID hex string.
    pub txid: TXIDHexString,
    /// Output index.
    pub vout: u32,
}

/// Validated sign action arguments.
#[derive(Debug, Clone)]
pub struct ValidSignActionArgs {
    /// Transaction reference string from createAction.
    pub reference: String,
    /// Per-input spend data (vin -> unlocking script info).
    pub spends: std::collections::HashMap<u32, SignActionSpend>,
    /// Sign action options.
    pub options: SignActionOptions,
    /// Whether this is a new transaction.
    pub is_new_tx: bool,
    /// Whether the transaction should not be broadcast.
    pub is_no_send: bool,
    /// Whether broadcasting should be deferred.
    pub is_delayed: bool,
    /// Whether this is a send-with batch operation.
    pub is_send_with: bool,
}

/// Validated internalize action arguments.
#[derive(Debug, Clone)]
pub struct ValidInternalizeActionArgs {
    /// The AtomicBEEF transaction bytes.
    pub tx: Vec<u8>,
    /// Human-readable description of the internalization.
    pub description: String,
    /// Labels to apply to the internalized action.
    pub labels: Vec<LabelStringUnder300Bytes>,
    /// Which outputs to internalize and how.
    pub outputs: Vec<InternalizeOutput>,
}

/// Validated abort action arguments.
#[derive(Debug, Clone)]
pub struct ValidAbortActionArgs {
    /// The reference string identifying the transaction to abort.
    pub reference: String,
}

// ---------------------------------------------------------------------------
// Pending sign action state (in-memory during delayed signing)
// ---------------------------------------------------------------------------

/// In-memory state for a transaction awaiting delayed signing.
///
/// Created during createAction with isSignAction=true, consumed during signAction.
#[derive(Debug, Clone)]
pub struct PendingSignAction {
    /// The transaction reference string from storage.
    pub reference: String,
    /// The storage create action result containing inputs, outputs, and BEEF data.
    pub dcr: StorageCreateActionResult,
    /// The original validated create action args.
    pub args: ValidCreateActionArgs,
    /// The serialized transaction bytes (unsigned).
    pub tx: Vec<u8>,
    /// The total input amount in satoshis.
    pub amount: u64,
    /// Per-input derivation info for signing.
    pub pdi: Vec<PendingStorageInput>,
}

/// Per-input derivation info for delayed signing.
///
/// Contains the information needed to derive the signing key for each input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingStorageInput {
    /// The input index (vin) in the transaction.
    pub vin: u32,
    /// The derivation prefix for key derivation.
    pub derivation_prefix: String,
    /// The derivation suffix for key derivation.
    pub derivation_suffix: String,
    /// The counterparty public key (hex) for key derivation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlocker_pub_key: Option<String>,
    /// The source output value in satoshis.
    pub source_satoshis: u64,
    /// The source output locking script (hex).
    pub locking_script: String,
}

// ---------------------------------------------------------------------------
// Signer result types
// ---------------------------------------------------------------------------

/// Result from signer createAction.
#[derive(Debug, Clone)]
pub struct SignerCreateActionResult {
    /// The transaction ID (if signed immediately).
    pub txid: Option<TXIDHexString>,
    /// The AtomicBEEF transaction bytes (if signed immediately).
    pub tx: Option<Vec<u8>>,
    /// Outpoints of change outputs for noSend transactions.
    pub no_send_change: Vec<OutpointString>,
    /// Results of sending batched transactions.
    pub send_with_results: Vec<SendWithResult>,
    /// If isSignAction, the signable transaction for deferred signing.
    pub signable_transaction: Option<SignableTransactionRef>,
}

/// Reference to a signable transaction for delayed signing.
#[derive(Debug, Clone)]
pub struct SignableTransactionRef {
    /// The reference string for later signAction calls.
    pub reference: String,
    /// The AtomicBEEF containing the unsigned transaction and source txs.
    pub tx: Vec<u8>,
}

/// Result from signer signAction.
#[derive(Debug, Clone)]
pub struct SignerSignActionResult {
    /// The transaction ID.
    pub txid: Option<TXIDHexString>,
    /// The AtomicBEEF transaction bytes.
    pub tx: Option<Vec<u8>>,
    /// Results of sending batched transactions.
    pub send_with_results: Vec<SendWithResult>,
}

/// Result from signer internalizeAction.
#[derive(Debug, Clone)]
pub struct SignerInternalizeActionResult {
    /// Whether the internalization was accepted.
    pub accepted: bool,
    /// Whether this was a merge with an existing transaction.
    pub is_merge: bool,
    /// The transaction ID.
    pub txid: String,
    /// Net change in satoshis (positive = received, negative = spent).
    pub satoshis: i64,
    /// Send-with results if broadcast was attempted.
    pub send_with_results: Option<Vec<SendWithResult>>,
}
