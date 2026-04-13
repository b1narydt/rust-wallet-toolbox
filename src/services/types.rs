//! Result types, config structs, call history types, and enums for the services layer.
//!
//! Ported from wallet-toolbox/src/sdk/WalletServices.interfaces.ts.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::WalletError;
use crate::types::Chain;

// ---------------------------------------------------------------------------
// Block Header
// ---------------------------------------------------------------------------

/// Fields of an 80-byte serialized block header whose double SHA-256 hash
/// produces the block hash.
///
/// All hash values are 32-byte hex strings with byte order reversed from
/// the serialized byte order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockHeader {
    /// Block header version value (4 bytes serialized).
    pub version: u32,
    /// Hash of the previous block header (32 bytes serialized).
    pub previous_hash: String,
    /// Root hash of the merkle tree of all transactions (32 bytes serialized).
    pub merkle_root: String,
    /// Block header time value (4 bytes serialized).
    pub time: u32,
    /// Block header bits (difficulty target) value (4 bytes serialized).
    pub bits: u32,
    /// Block header nonce value (4 bytes serialized).
    pub nonce: u32,
    /// Height of the header, starting from zero.
    pub height: u32,
    /// The double SHA-256 hash of the serialized base header fields.
    pub hash: String,
}

// ---------------------------------------------------------------------------
// GetMerklePath
// ---------------------------------------------------------------------------

/// Result returned from `WalletServices::get_merkle_path`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetMerklePathResult {
    /// The name of the service returning the proof, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The merkle path (proof) for the transaction, if found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merkle_path: Option<Vec<u8>>,
    /// Block header associated with the merkle path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<BlockHeader>,
    /// The first error that occurred during processing, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// GetRawTx
// ---------------------------------------------------------------------------

/// Result returned from `WalletServices::get_raw_tx`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetRawTxResult {
    /// Transaction hash of the request.
    pub txid: String,
    /// The name of the service returning the raw tx, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Raw transaction bytes, if found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_tx: Option<Vec<u8>>,
    /// The first error that occurred during processing, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// PostBeef
// ---------------------------------------------------------------------------

/// Result for a single txid in a post beef response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostTxResultForTxid {
    /// Transaction ID.
    pub txid: String,
    /// "success" or "error"
    pub status: String,
    /// True if the transaction was already known to the service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_known: Option<bool>,
    /// True if the broadcast double-spends at least one input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_spend: Option<bool>,
    /// Block hash, if the transaction has been mined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<String>,
    /// Block height, if the transaction has been mined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_height: Option<u32>,
    /// TXIDs of competing (double-spend) transactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub competing_txs: Option<Vec<String>>,
    /// True if the service was unable to process a potentially valid transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_error: Option<bool>,
    /// True if ARC returned SEEN_IN_ORPHAN_MEMPOOL (parent tx not yet propagated).
    /// Distinct from double_spend — orphan mempool is transient and should be retried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphan_mempool: Option<bool>,
}

/// Result returned from `WalletServices::post_beef` (per provider).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostBeefResult {
    /// The name of the service the transaction was submitted to.
    pub name: String,
    /// "success" if all txids returned success; "error" otherwise.
    pub status: String,
    /// The first error that occurred, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Per-txid results.
    pub txid_results: Vec<PostTxResultForTxid>,
}

impl PostBeefResult {
    /// Create an error result for a service timeout.
    pub fn timeout(provider_name: &str, txids: &[String], timeout_ms: u64) -> Self {
        PostBeefResult {
            name: provider_name.to_string(),
            status: "error".to_string(),
            error: Some(format!("Service timeout after {}ms", timeout_ms)),
            txid_results: txids
                .iter()
                .map(|txid| PostTxResultForTxid {
                    txid: txid.clone(),
                    status: "error".to_string(),
                    already_known: None,
                    double_spend: None,
                    block_hash: None,
                    block_height: None,
                    competing_txs: None,
                    service_error: Some(true),
                    orphan_mempool: None,
                })
                .collect(),
        }
    }
}

/// Mode for posting BEEF to broadcast services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PostBeefMode {
    /// Try providers sequentially until one succeeds.
    UntilSuccess,
    /// Send to all providers concurrently.
    PromiseAll,
}

// ---------------------------------------------------------------------------
// GetUtxoStatus
// ---------------------------------------------------------------------------

/// Output format for getUtxoStatus queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GetUtxoStatusOutputFormat {
    /// Little-endian SHA-256 hash of output script.
    #[serde(rename = "hashLE")]
    HashLE,
    /// Big-endian SHA-256 hash of output script.
    #[serde(rename = "hashBE")]
    HashBE,
    /// Entire transaction output script.
    #[serde(rename = "script")]
    Script,
}

/// Details about an occurrence of an output script as a UTXO.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetUtxoStatusDetails {
    /// Transaction ID containing the UTXO.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// Output index within the transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Value of the UTXO in satoshis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub satoshis: Option<u64>,
    /// Block height where the transaction was mined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Result returned from `WalletServices::get_utxo_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetUtxoStatusResult {
    /// The name of the service returning the result.
    pub name: String,
    /// "success" or "error".
    pub status: String,
    /// Error details, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the output is associated with at least one unspent tx output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_utxo: Option<bool>,
    /// Additional details about occurrences of this output script as a UTXO.
    pub details: Vec<GetUtxoStatusDetails>,
}

// ---------------------------------------------------------------------------
// GetStatusForTxids
// ---------------------------------------------------------------------------

/// Status result for a single txid.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusForTxidResult {
    /// Transaction ID.
    pub txid: String,
    /// Roughly the depth of the block containing txid from chain tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// "mined" if depth > 0, "known" if depth == 0, "unknown" if depth is None.
    pub status: String,
}

/// Result returned from `WalletServices::get_status_for_txids`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetStatusForTxidsResult {
    /// The name of the service returning results.
    pub name: String,
    /// "success" or "error".
    pub status: String,
    /// Error details, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Per-txid status results.
    pub results: Vec<StatusForTxidResult>,
}

// ---------------------------------------------------------------------------
// GetScriptHashHistory
// ---------------------------------------------------------------------------

/// A single entry in a script hash history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptHashHistoryEntry {
    /// Transaction ID.
    pub txid: String,
    /// Block height (None for unconfirmed transactions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Result returned from `WalletServices::get_script_hash_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetScriptHashHistoryResult {
    /// The name of the service returning results.
    pub name: String,
    /// "success" or "error".
    pub status: String,
    /// Error details, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Transaction history entries.
    pub history: Vec<ScriptHashHistoryEntry>,
}

// ---------------------------------------------------------------------------
// Call History Types
// ---------------------------------------------------------------------------

/// Maximum number of individual call records kept per provider.
pub const MAX_CALL_HISTORY: usize = 32;

/// Maximum number of reset count intervals kept per provider.
pub const MAX_RESET_COUNTS: usize = 32;

/// Error details for a failed service call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCallError {
    /// Error message text.
    pub message: String,
    /// WERR error code string.
    pub code: String,
}

impl ServiceCallError {
    /// Create from a `WalletError`.
    pub fn from_wallet_error(err: &WalletError) -> Self {
        ServiceCallError {
            message: err.to_string(),
            code: err.code().to_string(),
        }
    }
}

/// Minimum data tracked for each service call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCall {
    /// When the call was made.
    pub when: DateTime<Utc>,
    /// Duration of the call in milliseconds.
    pub msecs: i64,
    /// True if the service provider successfully processed the request.
    pub success: bool,
    /// Simple text summary of the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Error code and message if success is false and an exception was thrown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ServiceCallError>,
}

/// Counts of service calls over a time interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCallHistoryCounts {
    /// Count of calls returning success true.
    pub success: u32,
    /// Count of calls returning success false.
    pub failure: u32,
    /// Of failures, count of calls with a valid error code and message.
    pub error: u32,
    /// Start of the counting interval.
    pub since: DateTime<Utc>,
    /// End of the counting interval (bumped with each new call).
    pub until: DateTime<Utc>,
}

impl ServiceCallHistoryCounts {
    /// Create a new zero-count interval starting now.
    pub fn new_now() -> Self {
        let now = Utc::now();
        Self {
            success: 0,
            failure: 0,
            error: 0,
            since: now,
            until: now,
        }
    }

    /// Create a new zero-count interval starting at a specific time.
    pub fn new_at(since: DateTime<Utc>) -> Self {
        Self {
            success: 0,
            failure: 0,
            error: 0,
            since,
            until: since,
        }
    }
}

/// History of service calls for a single service, single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCallHistory {
    /// Name of the service (e.g., "postBeef", "getMerklePath").
    pub service_name: String,
    /// Name of the provider (e.g., "TAAL", "WhatsOnChain").
    pub provider_name: String,
    /// Most recent service calls (limited to MAX_CALL_HISTORY).
    pub calls: Vec<ServiceCall>,
    /// Counts since creation of the Services instance.
    pub total_counts: ServiceCallHistoryCounts,
    /// Entry at index 0 is the current interval. On reset, a new zero-count entry is prepended.
    /// Limited to MAX_RESET_COUNTS.
    pub reset_counts: Vec<ServiceCallHistoryCounts>,
}

/// History of service calls for a single service, all providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCallHistory {
    /// Name of the service.
    pub service_name: String,
    /// Per-provider history keyed by provider name.
    pub history_by_provider: HashMap<String, ProviderCallHistory>,
}

/// Aggregated call history across all service collections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesCallHistory {
    /// History for each service type.
    pub services: Vec<ServiceCallHistory>,
}

// ---------------------------------------------------------------------------
// Exchange Rates
// ---------------------------------------------------------------------------

/// Fiat exchange rates with a base currency.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiatExchangeRates {
    /// When the rates were last updated.
    pub timestamp: DateTime<Utc>,
    /// Base currency code (e.g., "USD").
    pub base: String,
    /// Exchange rates keyed by currency code (relative to base).
    pub rates: HashMap<String, f64>,
}

impl Default for FiatExchangeRates {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            base: "USD".to_string(),
            rates: HashMap::new(),
        }
    }
}

/// BSV exchange rate (USD per BSV).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BsvExchangeRate {
    /// When the rate was last updated.
    pub timestamp: DateTime<Utc>,
    /// BSV price in USD.
    pub rate_usd: f64,
}

impl Default for BsvExchangeRate {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            rate_usd: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// ARC SSE Event
// ---------------------------------------------------------------------------

/// A single event received from the ARC SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArcSseEvent {
    /// Transaction ID the event relates to.
    pub txid: String,
    /// Transaction status string (e.g., "SEEN_ON_NETWORK", "MINED").
    pub tx_status: String,
    /// ISO 8601 timestamp of the event.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// NLockTime
// ---------------------------------------------------------------------------

/// Input for `n_lock_time_is_final` -- either a raw locktime value or a transaction.
pub enum NLockTimeInput {
    /// A raw nLockTime u32 value.
    Raw(u32),
    /// A full transaction (locktime extracted from the transaction).
    Transaction(bsv::transaction::Transaction),
}

// ---------------------------------------------------------------------------
// Config Types
// ---------------------------------------------------------------------------

/// Configuration for an ARC broadcast service endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArcConfig {
    /// API key for authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Deployment ID for tracking (default: "wallet-toolbox").
    pub deployment_id: String,
    /// Callback URL for transaction status updates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    /// Callback authentication token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_token: Option<String>,
    /// Optional HTTP client identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_client: Option<String>,
    /// Additional headers to be attached to all tx submissions.
    /// Matches TS SDK's `headers?: Record<string, string>` field.
    /// Use for `X-SkipScriptValidation`, `X-SkipTxValidation`, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::HashMap<String, String>>,
}

/// Generate a default deployment ID matching TS SDK pattern: `rust-sdk-{16 random hex chars}`.
///
/// TS SDK uses `ts-sdk-{Random(16) as hex}` so each deployment is uniquely
/// traceable in ARC's request logs. Rust uses the same pattern with `rust-sdk-` prefix.
fn default_deployment_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // 16 hex chars derived from timestamp + process id for uniqueness without rand dependency.
    let pid = std::process::id() as u128;
    let mixed = nanos.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(pid);
    format!("rust-sdk-{:016x}", mixed as u64)
}

impl Default for ArcConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            deployment_id: default_deployment_id(),
            callback_url: None,
            callback_token: None,
            http_client: None,
            headers: None,
        }
    }
}

/// Configuration for the wallet services layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesConfig {
    /// Which BSV chain to use.
    pub chain: Chain,
    /// TAAL ARC service URL.
    pub arc_url: String,
    /// TAAL ARC configuration.
    pub arc_config: ArcConfig,
    /// GorillaPool ARC service URL (mainnet only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arc_gorilla_pool_url: Option<String>,
    /// GorillaPool ARC configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arc_gorilla_pool_config: Option<ArcConfig>,
    /// API key for WhatsOnChain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whats_on_chain_api_key: Option<String>,
    /// API key for Bitails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitails_api_key: Option<String>,
    /// Chaintracks service URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chaintracks_url: Option<String>,
    /// Chaintracks fiat exchange rates endpoint URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chaintracks_fiat_exchange_rates_url: Option<String>,
    /// API key for exchangeratesapi.io.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchangeratesapi_key: Option<String>,
    /// Current BSV exchange rate.
    pub bsv_exchange_rate: BsvExchangeRate,
    /// Update interval for BSV exchange rate in milliseconds (default: 15 minutes).
    pub bsv_update_msecs: u64,
    /// Current fiat exchange rates.
    pub fiat_exchange_rates: FiatExchangeRates,
    /// Update interval for fiat exchange rates in milliseconds (default: 24 hours).
    pub fiat_update_msecs: u64,
    /// Base timeout for postBeef in milliseconds.
    pub post_beef_soft_timeout_ms: u64,
    /// Additional timeout per KiB of BEEF payload.
    pub post_beef_soft_timeout_per_kb_ms: u64,
    /// Maximum timeout for postBeef in milliseconds.
    pub post_beef_soft_timeout_max_ms: u64,
}

impl From<Chain> for ServicesConfig {
    fn from(chain: Chain) -> Self {
        let (arc_url, gorilla_pool_url, chaintracks_url) = match chain {
            Chain::Main => (
                "https://arc.taal.com".to_string(),
                Some("https://arc.gorillapool.io".to_string()),
                "https://mainnet-chaintracks.babbage.systems".to_string(),
            ),
            Chain::Test => (
                "https://arc-test.taal.com".to_string(),
                None,
                "https://testnet-chaintracks.babbage.systems".to_string(),
            ),
        };

        let gorilla_pool_config = gorilla_pool_url.as_ref().map(|_| ArcConfig::default());

        Self {
            chain,
            arc_url,
            arc_config: ArcConfig::default(),
            arc_gorilla_pool_url: gorilla_pool_url,
            arc_gorilla_pool_config: gorilla_pool_config,
            whats_on_chain_api_key: None,
            bitails_api_key: None,
            chaintracks_url: Some(chaintracks_url),
            chaintracks_fiat_exchange_rates_url: None,
            exchangeratesapi_key: None,
            bsv_exchange_rate: BsvExchangeRate::default(),
            bsv_update_msecs: 15 * 60 * 1000, // 15 minutes
            fiat_exchange_rates: FiatExchangeRates::default(),
            fiat_update_msecs: 24 * 60 * 60 * 1000, // 24 hours
            post_beef_soft_timeout_ms: 5000,
            post_beef_soft_timeout_per_kb_ms: 50,
            post_beef_soft_timeout_max_ms: 30000,
        }
    }
}

impl ServicesConfig {
    /// Calculate the adaptive soft timeout for postBeef based on BEEF payload size.
    pub fn get_post_beef_soft_timeout_ms(&self, beef_bytes_len: usize) -> u64 {
        let base_ms = self.post_beef_soft_timeout_ms;
        let per_kb_ms = self.post_beef_soft_timeout_per_kb_ms;
        let max_ms = self.post_beef_soft_timeout_max_ms.max(base_ms);
        if per_kb_ms == 0 {
            return base_ms.min(max_ms);
        }
        let extra_ms = ((beef_bytes_len as f64 / 1024.0) * per_kb_ms as f64).ceil() as u64;
        (base_ms + extra_ms).min(max_ms)
    }
}
