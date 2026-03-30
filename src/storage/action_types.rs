//! Storage-side action types for the signer pipeline.
//!
//! These types define the arguments and results for storage-level
//! create, process, and internalize action operations. They bridge
//! the signer pipeline to the storage layer.

use serde::{Deserialize, Serialize};

use bsv::wallet::interfaces::{InternalizeOutput, SendWithResult};
use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue, TXIDHexString};

use crate::types::StorageProvidedBy;

// ---------------------------------------------------------------------------
// Fee model
// ---------------------------------------------------------------------------

/// Storage fee model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageFeeModel {
    /// The fee model type (currently only "sat/kb" is supported).
    pub model: String,
    /// The fee rate value (satoshis per KB for "sat/kb" model).
    pub value: u64,
}

impl Default for StorageFeeModel {
    fn default() -> Self {
        Self {
            model: "sat/kb".to_string(),
            value: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Create action types
// ---------------------------------------------------------------------------

/// Outpoint reference matching the TS `{ txid, vout }` wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOutPoint {
    pub txid: TXIDHexString,
    pub vout: u32,
}

/// Options for create action, matching TS ValidCreateActionOptions wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateActionOptions {
    pub sign_and_process: BooleanDefaultTrue,
    pub accept_delayed_broadcast: BooleanDefaultTrue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_self: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub known_txids: Vec<TXIDHexString>,
    pub return_txid_only: BooleanDefaultFalse,
    pub no_send: BooleanDefaultFalse,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub no_send_change: Vec<StorageOutPoint>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub send_with: Vec<TXIDHexString>,
    pub randomize_outputs: BooleanDefaultTrue,
}

impl Default for StorageCreateActionOptions {
    fn default() -> Self {
        Self {
            sign_and_process: BooleanDefaultTrue(Some(true)),
            accept_delayed_broadcast: BooleanDefaultTrue(Some(true)),
            trust_self: None,
            known_txids: vec![],
            return_txid_only: BooleanDefaultFalse(Some(false)),
            no_send: BooleanDefaultFalse(Some(false)),
            no_send_change: vec![],
            send_with: vec![],
            randomize_outputs: BooleanDefaultTrue(Some(true)),
        }
    }
}

/// Arguments for storage.create_action.
///
/// Carries validated create action data from the signer to storage,
/// which allocates UTXOs, creates transaction records, and returns
/// the data needed to build the actual transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateActionArgs {
    /// Human-readable description of the action.
    pub description: String,
    /// The transaction inputs to spend.
    pub inputs: Vec<StorageCreateActionInput>,
    /// The transaction outputs to create.
    pub outputs: Vec<StorageCreateActionOutput>,
    /// Transaction lock time.
    pub lock_time: u32,
    /// Transaction version.
    pub version: u32,
    /// Labels to apply to this action.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub labels: Vec<String>,
    /// Action options (signAndProcess, noSend, etc.).
    pub options: StorageCreateActionOptions,
    /// BEEF data for input validity proofs.
    #[serde(rename = "inputBEEF", skip_serializing_if = "Option::is_none")]
    pub input_beef: Option<Vec<u8>>,
    /// True if this is a new transaction (not a sign-only operation).
    pub is_new_tx: bool,
    /// If true, this is a signAction (partial signing) flow.
    pub is_sign_action: bool,
    /// If true, do not broadcast the transaction.
    pub is_no_send: bool,
    /// If true, defer broadcasting.
    pub is_delayed: bool,
    /// True if this is a send-with batch operation.
    pub is_send_with: bool,
    /// True if this action remixes change.
    pub is_remix_change: bool,
    /// Test hook for werr review actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_test_werr_review_actions: Option<bool>,
    /// Whether to include all source transactions in BEEF.
    pub include_all_source_transactions: bool,
    /// Test hook: pre-determined random values for deterministic testing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random_vals: Option<Vec<f64>>,
}

/// An input specification for storage create action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateActionInput {
    /// Nested outpoint reference ({ txid, vout }).
    pub outpoint: StorageOutPoint,
    /// Human-readable description of why this input is spent.
    pub input_description: String,
    /// Expected unlocking script length for size estimation.
    pub unlocking_script_length: usize,
    /// nSequence value for this input.
    pub sequence_number: u32,
}

/// An output specification for storage create action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateActionOutput {
    /// Locking script as hex string.
    pub locking_script: String,
    /// Output value in satoshis.
    pub satoshis: u64,
    /// Human-readable output description.
    pub output_description: String,
    /// Optional basket name to categorize this output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basket: Option<String>,
    /// Optional custom spending instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Tags to apply to this output.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
}

/// Result from storage.create_action.
///
/// Contains all the data the signer needs to build and sign the transaction:
/// allocated inputs (with locking scripts), output assignments, BEEF data, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateActionResult {
    /// The transaction reference string.
    pub reference: String,
    /// Transaction version.
    pub version: u32,
    /// Transaction lock time.
    pub lock_time: u32,
    /// The allocated inputs with their source data.
    pub inputs: Vec<StorageCreateTransactionSdkInput>,
    /// The assigned outputs.
    pub outputs: Vec<StorageCreateTransactionSdkOutput>,
    /// The derivation prefix for this transaction.
    pub derivation_prefix: String,
    /// BEEF data for input validity proofs.
    #[serde(rename = "inputBEEF", skip_serializing_if = "Option::is_none")]
    pub input_beef: Option<Vec<u8>>,
    /// Output vouts that are change outputs (for noSend).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_send_change_output_vouts: Option<Vec<u32>>,
}

/// An allocated input from storage, with source transaction data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateTransactionSdkInput {
    /// The input index in the transaction being built.
    pub vin: u32,
    /// The source transaction ID.
    pub source_txid: String,
    /// The source output index.
    pub source_vout: u32,
    /// The source output value in satoshis.
    pub source_satoshis: u64,
    /// The source output locking script (hex).
    pub source_locking_script: String,
    /// Optional raw source transaction bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_transaction: Option<Vec<u8>>,
    /// The expected unlocking script length for size estimation.
    pub unlocking_script_length: usize,
    /// Who provided this input.
    pub provided_by: StorageProvidedBy,
    /// The type of output (e.g., "P2PKH", "custom").
    #[serde(rename = "type")]
    pub output_type: String,
    /// Description of why this output is being spent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spending_description: Option<String>,
    /// Derivation prefix for key derivation (change inputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_prefix: Option<String>,
    /// Derivation suffix for key derivation (change inputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_suffix: Option<String>,
    /// Sender identity key for authenticated payments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_identity_key: Option<String>,
}

/// An assigned output from storage create action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCreateTransactionSdkOutput {
    /// The output index in the transaction.
    pub vout: u32,
    /// The output value in satoshis.
    pub satoshis: u64,
    /// The locking script (hex).
    pub locking_script: String,
    /// Who provided this output.
    pub provided_by: StorageProvidedBy,
    /// The purpose of this output (e.g., "change", "service-charge").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The basket name (if tracked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basket: Option<String>,
    /// Output tags.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
    /// Output description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_description: Option<String>,
    /// Derivation suffix for change outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_suffix: Option<String>,
    /// Custom instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
}

// ---------------------------------------------------------------------------
// Allocated change types (for the change generation algorithm)
// ---------------------------------------------------------------------------

/// A UTXO allocated as a change input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocatedChangeInput {
    /// The storage output ID.
    pub output_id: i64,
    /// The value in satoshis.
    pub satoshis: u64,
    /// Derivation prefix for key derivation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_prefix: Option<String>,
    /// Derivation suffix for key derivation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_suffix: Option<String>,
    /// The locking script (hex).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locking_script: Option<String>,
}

/// Arguments for the change generation algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateChangeSdkArgs {
    /// Fixed inputs with known satoshis and script lengths.
    pub fixed_inputs: Vec<FixedInput>,
    /// Fixed outputs with known satoshis and script lengths.
    pub fixed_outputs: Vec<FixedOutput>,
    /// The fee model to use.
    pub fee_model: StorageFeeModel,
    /// Initial satoshis for new change outputs.
    pub change_initial_satoshis: u64,
    /// Satoshis for the first change output (typically smaller).
    pub change_first_satoshis: u64,
    /// Locking script length for change outputs (25 for P2PKH).
    pub change_locking_script_length: usize,
    /// Unlocking script length for change outputs (107 for P2PKH).
    pub change_unlocking_script_length: usize,
    /// Target net change UTXO count adjustment.
    /// None means only add change output if there is excess, matching TS `targetNetCount?: number`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_net_count: Option<i64>,
}

/// A fixed input for change generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedInput {
    /// Input value in satoshis.
    pub satoshis: u64,
    /// Expected unlocking script length.
    pub unlocking_script_length: usize,
}

/// A fixed output for change generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedOutput {
    /// Output value in satoshis.
    pub satoshis: u64,
    /// Locking script length in bytes.
    pub locking_script_length: usize,
}

/// Result from the change generation algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateChangeSdkResult {
    /// The change outputs to create.
    pub change_outputs: Vec<ChangeOutput>,
    /// The allocated change inputs.
    pub allocated_change_inputs: Vec<AllocatedChangeInputRef>,
    /// The computed transaction size in bytes.
    pub size: usize,
    /// The total fee in satoshis.
    pub fee: u64,
    /// The fee rate used (satoshis per KB).
    pub sats_per_kb: u64,
    /// Adjustment for "max possible satoshis" outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_possible_satoshis_adjustment: Option<MaxPossibleSatoshisAdjustment>,
}

/// A change output to create.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOutput {
    /// Change output value in satoshis.
    pub satoshis: u64,
}

/// Reference to an allocated change input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocatedChangeInputRef {
    /// Storage output ID of the allocated input.
    pub output_id: i64,
    /// Value in satoshis.
    pub satoshis: u64,
}

/// Adjustment for outputs that request "max possible" satoshis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaxPossibleSatoshisAdjustment {
    /// Index of the fixed output that receives the adjustment.
    pub fixed_output_index: usize,
    /// The adjusted satoshi value.
    pub satoshis: u64,
}

// ---------------------------------------------------------------------------
// Process action types
// ---------------------------------------------------------------------------

/// Arguments for storage.process_action.
///
/// Sent after a transaction has been signed. Contains the signed transaction
/// data for storage to commit and optionally broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProcessActionArgs {
    /// True if this is a new transaction (not just a send-with).
    pub is_new_tx: bool,
    /// True if this is a send-with batch.
    pub is_send_with: bool,
    /// True if the transaction should not be broadcast.
    pub is_no_send: bool,
    /// True if broadcasting should be deferred.
    pub is_delayed: bool,
    /// The transaction reference string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The transaction ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<TXIDHexString>,
    /// The raw signed transaction bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_tx: Option<Vec<u8>>,
    /// TXIDs to send as a batch with this transaction.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub send_with: Vec<TXIDHexString>,
}

/// Result from storage.process_action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProcessActionResult {
    /// Results from batch sending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_with_results: Option<Vec<SendWithResult>>,
}

// ---------------------------------------------------------------------------
// Internalize action types
// ---------------------------------------------------------------------------

/// Arguments for storage.internalize_action.
///
/// Takes ownership of outputs from an external transaction.
/// Uses the SDK `InternalizeOutput` tagged enum so the wire format matches
/// the TypeScript `StorageClient.internalizeAction` shape exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInternalizeActionArgs {
    /// The AtomicBEEF transaction bytes.
    pub tx: Vec<u8>,
    /// Description of the internalization.
    pub description: String,
    /// Labels to apply.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub labels: Vec<String>,
    /// Whether to seek user permission before internalizing.
    #[serde(default = "default_true")]
    pub seek_permission: bool,
    /// Output specifications (which outputs to internalize and how).
    /// Uses the SDK tagged enum: `{ protocol: "wallet payment", paymentRemittance: {...} }`
    /// or `{ protocol: "basket insertion", insertionRemittance: {...} }`.
    pub outputs: Vec<InternalizeOutput>,
}

fn default_true() -> bool {
    true
}

/// Result from storage.internalize_action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInternalizeActionResult {
    /// Whether the internalization was accepted.
    pub accepted: bool,
    /// Whether this was a merge with an existing transaction.
    pub is_merge: bool,
    /// The transaction ID.
    pub txid: String,
    /// Net change in satoshis.
    pub satoshis: i64,
    /// Send-with results if broadcast was attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_with_results: Option<Vec<SendWithResult>>,
}
