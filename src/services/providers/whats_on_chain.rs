//! WhatsOnChain provider implementing all 6 service provider traits.
//!
//! Ported from wallet-toolbox/src/services/providers/WhatsOnChain.ts.
//! Makes HTTP calls to the WhatsOnChain API for merkle paths, raw transactions,
//! UTXO status, transaction status, script hash history, and beef broadcasting.

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::WalletError;
use crate::services::traits::{
    GetMerklePathProvider, GetRawTxProvider, GetScriptHashHistoryProvider,
    GetStatusForTxidsProvider, GetUtxoStatusProvider, PostBeefProvider, WalletServices,
};
use crate::services::types::{
    BlockHeader, GetMerklePathResult, GetRawTxResult, GetScriptHashHistoryResult,
    GetStatusForTxidsResult, GetUtxoStatusDetails, GetUtxoStatusOutputFormat, GetUtxoStatusResult,
    PostBeefResult, PostTxResultForTxid, ScriptHashHistoryEntry, StatusForTxidResult,
};
use crate::types::Chain;

// ---------------------------------------------------------------------------
// Utility helpers (pub(crate) for unit testing)
// ---------------------------------------------------------------------------

/// Decode a hex string to bytes.
pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, WalletError> {
    if hex.len() % 2 != 0 {
        return Err(WalletError::InvalidParameter {
            parameter: "hex".to_string(),
            must_be: "an even-length hex string".to_string(),
        });
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| WalletError::InvalidParameter {
                parameter: "hex".to_string(),
                must_be: "valid hex characters".to_string(),
            })
        })
        .collect()
}

/// Encode bytes to lowercase hex string.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

/// Compute double SHA-256 hash of data, returned in big-endian (reversed) byte order.
/// This matches the TS `doubleSha256BE` function.
pub fn double_sha256_be(data: &[u8]) -> Vec<u8> {
    let hash = bsv::primitives::hash::sha256d(data);
    let mut result = hash.to_vec();
    result.reverse();
    result
}

/// Validate and convert a script hash to big-endian hex format for WoC API calls.
///
/// Matches the TS `validateScriptHash` function from Services.ts.
///
/// - hashLE: reverse bytes to get hashBE
/// - hashBE: use as-is
/// - script: SHA256 the bytes, reverse to BE hex
/// - None/default: if 32 bytes (64 hex chars) assume hashLE, otherwise assume script
pub fn validate_script_hash(
    output: &str,
    output_format: Option<&GetUtxoStatusOutputFormat>,
) -> Result<String, WalletError> {
    let bytes = hex_to_bytes(output)?;

    let format = match output_format {
        Some(f) => f.clone(),
        None => {
            if bytes.len() == 32 {
                GetUtxoStatusOutputFormat::HashLE
            } else {
                GetUtxoStatusOutputFormat::Script
            }
        }
    };

    let result_bytes = match format {
        GetUtxoStatusOutputFormat::HashBE => bytes,
        GetUtxoStatusOutputFormat::HashLE => {
            let mut reversed = bytes;
            reversed.reverse();
            reversed
        }
        GetUtxoStatusOutputFormat::Script => {
            let hash = bsv::primitives::hash::sha256(&bytes);
            let mut reversed = hash.to_vec();
            reversed.reverse();
            reversed
        }
    };

    Ok(bytes_to_hex(&result_bytes))
}

/// Convert a TSC proof (WoC/Bitails response format) into a MerklePath serialized
/// to BUMP binary format.
///
/// Ported from wallet-toolbox/src/utility/tscProofToMerklePath.ts.
pub fn convert_proof_to_merkle_path(
    txid: &str,
    index: u64,
    nodes: &[String],
    block_height: u32,
) -> Result<Vec<u8>, WalletError> {
    use bsv::transaction::MerklePathLeaf;

    let tree_height = nodes.len();
    let mut path: Vec<Vec<MerklePathLeaf>> = (0..tree_height).map(|_| Vec::new()).collect();

    let mut idx = index;
    for (level, node) in nodes.iter().enumerate() {
        let is_odd = idx % 2 == 1;
        let offset = if is_odd { idx - 1 } else { idx + 1 };

        let leaf = if node == "*" || (level == 0 && node == txid) {
            MerklePathLeaf {
                offset,
                hash: None,
                txid: false,
                duplicate: true,
            }
        } else {
            MerklePathLeaf {
                offset,
                hash: Some(node.clone()),
                txid: false,
                duplicate: false,
            }
        };

        if level == 0 {
            let txid_leaf = MerklePathLeaf {
                offset: index,
                hash: Some(txid.to_string()),
                txid: true,
                duplicate: false,
            };

            if is_odd {
                path[0].push(leaf);
                path[0].push(txid_leaf);
            } else {
                path[0].push(txid_leaf);
                path[0].push(leaf);
            }
        } else {
            path[level].push(leaf);
        }

        idx >>= 1;
    }

    // Use MerklePath to serialize to binary
    let mp = bsv::transaction::MerklePath::new(block_height, path)
        .map_err(|e| WalletError::Internal(format!("Failed to construct MerklePath: {}", e)))?;

    let mut buf = Vec::new();
    mp.to_binary(&mut buf)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize MerklePath: {}", e)))?;

    Ok(buf)
}

/// Reverse a hex-encoded hash from LE to BE (or vice versa).
/// Each pair of hex chars represents one byte; the byte order is reversed.
pub fn reverse_hex_hash(hex: &str) -> Result<String, WalletError> {
    let mut bytes = hex_to_bytes(hex)?;
    bytes.reverse();
    Ok(bytes_to_hex(&bytes))
}

// ---------------------------------------------------------------------------
// WoC API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WocTscProof {
    index: u64,
    nodes: Vec<String>,
    target: String,
    #[allow(dead_code)]
    #[serde(rename = "txOrId")]
    tx_or_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WocTxsStatusEntry {
    txid: String,
    #[allow(dead_code)]
    blockhash: Option<String>,
    #[allow(dead_code)]
    blockheight: Option<u32>,
    #[allow(dead_code)]
    blocktime: Option<u64>,
    confirmations: Option<u32>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WocUtxoStatusResponse {
    #[allow(dead_code)]
    script: String,
    result: Vec<WocUtxoEntry>,
}

#[derive(Debug, Deserialize)]
struct WocUtxoEntry {
    height: Option<u32>,
    tx_pos: u32,
    tx_hash: String,
    value: u64,
}

#[derive(Debug, Deserialize)]
struct WocScriptHashHistoryResponse {
    #[allow(dead_code)]
    script: Option<String>,
    result: Vec<WocScriptHashHistoryEntry>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WocScriptHashHistoryEntry {
    tx_hash: String,
    height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct WocBlockHeader {
    hash: String,
    height: u32,
    version: u32,
    merkleroot: String,
    time: u32,
    #[serde(deserialize_with = "deserialize_bits")]
    bits: u32,
    nonce: u32,
    previousblockhash: Option<String>,
}

/// Deserialize bits field that can be either a number or hex string.
fn deserialize_bits<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct BitsVisitor;

    impl<'de> de::Visitor<'de> for BitsVisitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u32 or hex string representing bits")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
            Ok(v as u32)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
            u32::from_str_radix(v, 16).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(BitsVisitor)
}

// ---------------------------------------------------------------------------
// WhatsOnChain provider
// ---------------------------------------------------------------------------

/// WhatsOnChain HTTP API provider.
///
/// Implements all 6 service provider traits:
/// - GetMerklePathProvider
/// - GetRawTxProvider
/// - PostBeefProvider
/// - GetUtxoStatusProvider
/// - GetStatusForTxidsProvider
/// - GetScriptHashHistoryProvider
pub struct WhatsOnChain {
    chain: Chain,
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

impl WhatsOnChain {
    /// Create a new WhatsOnChain provider.
    ///
    /// `chain` selects mainnet or testnet API endpoints.
    /// `api_key` is optional; if set, sent as `woc-api-key` header.
    /// `client` is a shared reqwest::Client for connection pooling.
    pub fn new(chain: Chain, api_key: Option<String>, client: reqwest::Client) -> Self {
        let network = match chain {
            Chain::Main => "main",
            Chain::Test => "test",
        };
        let base_url = format!("https://api.whatsonchain.com/v1/bsv/{}", network);
        Self {
            chain,
            api_key,
            base_url,
            client,
        }
    }

    /// Return the chain this provider is configured for.
    pub fn chain(&self) -> Chain {
        self.chain.clone()
    }

    /// Build a request with auth headers.
    fn request_builder(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.client.request(method, url);
        if let Some(ref key) = self.api_key {
            builder = builder.header("woc-api-key", key);
        }
        builder
    }

    /// Send a GET request, handling 429 rate limiting with retry.
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response, WalletError> {
        for retry in 0..2 {
            let resp = self
                .request_builder(reqwest::Method::GET, url)
                .send()
                .await
                .map_err(|e| WalletError::Internal(format!("HTTP request failed: {}", e)))?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && retry < 1 {
                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                continue;
            }

            return Ok(resp);
        }

        Err(WalletError::Internal(
            "Rate limit exceeded after retries".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// GetMerklePathProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetMerklePathProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoCTsc"
    }

    async fn get_merkle_path(
        &self,
        txid: &str,
        services: &dyn WalletServices,
    ) -> GetMerklePathResult {
        let url = format!("{}/tx/{}/proof/tsc", self.base_url, txid);

        let resp = match self.get_with_retry(&url).await {
            Ok(r) => r,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("WoCTsc".to_string()),
                    error: Some(e.to_string()),
                    ..Default::default()
                };
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                ..Default::default()
            };
        }

        if !resp.status().is_success() {
            return GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                error: Some(format!("WoC getMerklePath status {}", resp.status())),
                ..Default::default()
            };
        }

        // WoC may return a single proof or an array of proofs
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("WoCTsc".to_string()),
                    error: Some(format!("Failed to read response body: {}", e)),
                    ..Default::default()
                };
            }
        };

        if text.is_empty() {
            // Unmined, proof not yet available
            return GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                ..Default::default()
            };
        }

        // Try parsing as array first, then single proof
        let proofs: Vec<WocTscProof> = if text.starts_with('[') {
            match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    return GetMerklePathResult {
                        name: Some("WoCTsc".to_string()),
                        error: Some(format!("Failed to parse proof array: {}", e)),
                        ..Default::default()
                    };
                }
            }
        } else {
            match serde_json::from_str::<WocTscProof>(&text) {
                Ok(p) => vec![p],
                Err(e) => {
                    return GetMerklePathResult {
                        name: Some("WoCTsc".to_string()),
                        error: Some(format!("Failed to parse proof: {}", e)),
                        ..Default::default()
                    };
                }
            }
        };

        if proofs.len() != 1 {
            return GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                ..Default::default()
            };
        }

        let proof = &proofs[0];

        // Look up block header by hash to get height
        let header = match services.hash_to_header(&proof.target).await {
            Ok(h) => h,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("WoCTsc".to_string()),
                    error: Some(format!("hashToHeader failed: {}", e)),
                    ..Default::default()
                };
            }
        };

        // Convert TSC proof to MerklePath
        match convert_proof_to_merkle_path(txid, proof.index, &proof.nodes, header.height) {
            Ok(merkle_path_bytes) => GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                merkle_path: Some(merkle_path_bytes),
                header: Some(header),
                error: None,
            },
            Err(e) => GetMerklePathResult {
                name: Some("WoCTsc".to_string()),
                error: Some(format!("Failed to convert proof to MerklePath: {}", e)),
                ..Default::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// GetRawTxProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetRawTxProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoC"
    }

    async fn get_raw_tx(&self, txid: &str) -> GetRawTxResult {
        let url = format!("{}/tx/{}/hex", self.base_url, txid);

        let resp = match self.get_with_retry(&url).await {
            Ok(r) => r,
            Err(e) => {
                return GetRawTxResult {
                    txid: txid.to_string(),
                    name: Some("WoC".to_string()),
                    error: Some(e.to_string()),
                    ..Default::default()
                };
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return GetRawTxResult {
                txid: txid.to_string(),
                name: Some("WoC".to_string()),
                raw_tx: None,
                error: None,
            };
        }

        if !resp.status().is_success() {
            return GetRawTxResult {
                txid: txid.to_string(),
                name: Some("WoC".to_string()),
                error: Some(format!("WoC getRawTx status {}", resp.status())),
                ..Default::default()
            };
        }

        let hex_str = match resp.text().await {
            Ok(t) => t.trim().to_string(),
            Err(e) => {
                return GetRawTxResult {
                    txid: txid.to_string(),
                    name: Some("WoC".to_string()),
                    error: Some(format!("Failed to read response: {}", e)),
                    ..Default::default()
                };
            }
        };

        if hex_str.is_empty() {
            return GetRawTxResult {
                txid: txid.to_string(),
                name: Some("WoC".to_string()),
                raw_tx: None,
                error: None,
            };
        }

        let raw_bytes = match hex_to_bytes(&hex_str) {
            Ok(b) => b,
            Err(e) => {
                return GetRawTxResult {
                    txid: txid.to_string(),
                    name: Some("WoC".to_string()),
                    error: Some(format!("Failed to decode hex: {}", e)),
                    ..Default::default()
                };
            }
        };

        // VALIDATE: compute doubleSha256BE of decoded bytes, compare to requested txid
        let computed_hash = double_sha256_be(&raw_bytes);
        let computed_txid = bytes_to_hex(&computed_hash);
        if computed_txid != txid {
            return GetRawTxResult {
                txid: txid.to_string(),
                name: Some("WoC".to_string()),
                error: Some("getRawTx txid mismatch".to_string()),
                ..Default::default()
            };
        }

        GetRawTxResult {
            txid: txid.to_string(),
            name: Some("WoC".to_string()),
            raw_tx: Some(raw_bytes),
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// PostBeefProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl PostBeefProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoC"
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> PostBeefResult {
        // WoC does not natively support BEEF. Decompose into individual raw tx submissions.
        // For each txid, find the raw tx in the BEEF and broadcast individually.
        //
        // Parse BEEF to extract raw transactions for the requested txids.
        let parsed_beef = match bsv::transaction::Beef::from_binary(&mut std::io::Cursor::new(beef))
        {
            Ok(b) => b,
            Err(e) => {
                return PostBeefResult {
                    name: "WoC".to_string(),
                    status: "error".to_string(),
                    error: Some(format!("Failed to parse BEEF: {}", e)),
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
                        })
                        .collect(),
                };
            }
        };

        let mut result = PostBeefResult {
            name: "WoC".to_string(),
            status: "success".to_string(),
            error: None,
            txid_results: Vec::new(),
        };

        let mut delay = false;
        for txid in txids {
            // Extract raw tx for this txid from BEEF
            let raw_tx_bytes = match parsed_beef.txs.iter().find(|btx| btx.txid == *txid) {
                Some(beef_tx) => {
                    let tx = match &beef_tx.tx {
                        Some(t) => t,
                        None => {
                            result.txid_results.push(PostTxResultForTxid {
                                txid: txid.clone(),
                                status: "error".to_string(),
                                already_known: None,
                                double_spend: None,
                                block_hash: None,
                                block_height: None,
                                competing_txs: None,
                                service_error: Some(true),
                            });
                            if result.status == "success" {
                                result.status = "error".to_string();
                                result.error = Some("Tx is txid-only in BEEF".to_string());
                            }
                            continue;
                        }
                    };
                    let mut buf = Vec::new();
                    if let Err(e) = tx.to_binary(&mut buf) {
                        result.txid_results.push(PostTxResultForTxid {
                            txid: txid.clone(),
                            status: "error".to_string(),
                            already_known: None,
                            double_spend: None,
                            block_hash: None,
                            block_height: None,
                            competing_txs: None,
                            service_error: Some(true),
                        });
                        if result.status == "success" {
                            result.status = "error".to_string();
                            result.error = Some(format!("Failed to serialize tx: {}", e));
                        }
                        continue;
                    }
                    buf
                }
                None => {
                    result.txid_results.push(PostTxResultForTxid {
                        txid: txid.clone(),
                        status: "error".to_string(),
                        already_known: None,
                        double_spend: None,
                        block_hash: None,
                        block_height: None,
                        competing_txs: None,
                        service_error: Some(true),
                    });
                    if result.status == "success" {
                        result.status = "error".to_string();
                        result.error = Some(format!("txid {} not found in BEEF", txid));
                    }
                    continue;
                }
            };

            let raw_hex = bytes_to_hex(&raw_tx_bytes);

            if delay {
                // For multiple txids, give WoC time to propagate each one.
                tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
            }
            delay = true;

            // POST /tx/raw with JSON body { "txhex": rawHex }
            let url = format!("{}/tx/raw", self.base_url);
            let body = serde_json::json!({ "txhex": raw_hex });

            let post_result = self
                .request_builder(reqwest::Method::POST, &url)
                .header("Content-Type", "application/json")
                .header("Accept", "text/plain")
                .json(&body)
                .send()
                .await;

            let tr = match post_result {
                Ok(resp) => {
                    let status_code = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();

                    if status_code.is_success() {
                        PostTxResultForTxid {
                            txid: txid.clone(),
                            status: "success".to_string(),
                            already_known: None,
                            double_spend: None,
                            block_hash: None,
                            block_height: None,
                            competing_txs: None,
                            service_error: None,
                        }
                    } else if body_text.contains("already in the mempool") {
                        // Already known -- treat as success
                        PostTxResultForTxid {
                            txid: txid.clone(),
                            status: "success".to_string(),
                            already_known: Some(true),
                            double_spend: None,
                            block_hash: None,
                            block_height: None,
                            competing_txs: None,
                            service_error: None,
                        }
                    } else if body_text.contains("mempool-conflict")
                        || body_text.contains("Missing inputs")
                    {
                        PostTxResultForTxid {
                            txid: txid.clone(),
                            status: "error".to_string(),
                            already_known: None,
                            double_spend: Some(true),
                            block_hash: None,
                            block_height: None,
                            competing_txs: None,
                            service_error: None,
                        }
                    } else {
                        PostTxResultForTxid {
                            txid: txid.clone(),
                            status: "error".to_string(),
                            already_known: None,
                            double_spend: None,
                            block_hash: None,
                            block_height: None,
                            competing_txs: None,
                            service_error: Some(true),
                        }
                    }
                }
                Err(_e) => PostTxResultForTxid {
                    txid: txid.clone(),
                    status: "error".to_string(),
                    already_known: None,
                    double_spend: None,
                    block_hash: None,
                    block_height: None,
                    competing_txs: None,
                    service_error: Some(true),
                },
            };

            if tr.status != "success" && result.status == "success" {
                result.status = "error".to_string();
            }
            result.txid_results.push(tr);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// GetUtxoStatusProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetUtxoStatusProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoC"
    }

    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
    ) -> GetUtxoStatusResult {
        let mut r = GetUtxoStatusResult {
            name: "WoC".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL".to_string()),
            is_utxo: None,
            details: Vec::new(),
        };

        // Double retry loop with 2000ms wait between retries (matching TS pattern)
        for retry in 0u32.. {
            let script_hash = match validate_script_hash(output, output_format.as_ref()) {
                Ok(h) => h,
                Err(e) => {
                    r.error = Some(format!("Invalid script hash: {}", e));
                    return r;
                }
            };

            let url = format!("{}/script/{}/unspent/all", self.base_url, script_hash);

            let resp = match self.get_with_retry(&url).await {
                Ok(resp) => resp,
                Err(e) => {
                    if retry > 2 {
                        r.error = Some(format!("service failure: {}, error: {}", url, e));
                        return r;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    continue;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                if retry > 2 {
                    r.error = Some(format!("WoC getUtxoStatus response {}", status));
                    return r;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                continue;
            }

            let data: WocUtxoStatusResponse = match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    if retry > 2 {
                        r.error = Some(format!("Failed to parse response: {}", e));
                        return r;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                    continue;
                }
            };

            if data.result.is_empty() {
                r.status = "success".to_string();
                r.error = None;
                r.is_utxo = Some(false);
                return r;
            }

            r.status = "success".to_string();
            r.error = None;

            for s in &data.result {
                r.details.push(GetUtxoStatusDetails {
                    txid: Some(s.tx_hash.clone()),
                    satoshis: Some(s.value),
                    height: s.height,
                    index: Some(s.tx_pos),
                });
            }

            if let Some(outpoint_str) = outpoint {
                // Parse outpoint: "txid.vout"
                if let Some((op_txid, op_vout_str)) = outpoint_str.split_once('.') {
                    if let Ok(op_vout) = op_vout_str.parse::<u32>() {
                        r.is_utxo = Some(r.details.iter().any(|d| {
                            d.txid.as_deref() == Some(op_txid) && d.index == Some(op_vout)
                        }));
                    } else {
                        r.is_utxo = Some(!r.details.is_empty());
                    }
                } else {
                    r.is_utxo = Some(!r.details.is_empty());
                }
            } else {
                r.is_utxo = Some(!r.details.is_empty());
            }

            return r;
        }

        r
    }
}

// ---------------------------------------------------------------------------
// GetStatusForTxidsProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetStatusForTxidsProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoC"
    }

    async fn get_status_for_txids(&self, txids: &[String]) -> GetStatusForTxidsResult {
        let mut r = GetStatusForTxidsResult {
            name: "WoC".to_string(),
            status: "error".to_string(),
            error: None,
            results: Vec::new(),
        };

        let url = format!("{}/txs/status", self.base_url);
        let body = serde_json::json!({ "txids": txids });

        let resp = match self
            .request_builder(reqwest::Method::POST, &url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                r.error = Some(format!("HTTP request failed: {}", e));
                return r;
            }
        };

        if !resp.status().is_success() {
            r.error = Some(format!(
                "Unable to get status for txids, status {}",
                resp.status()
            ));
            return r;
        }

        let data: Vec<WocTxsStatusEntry> = match resp.json().await {
            Ok(d) => d,
            Err(e) => {
                r.error = Some(format!("Failed to parse response: {}", e));
                return r;
            }
        };

        for txid in txids {
            let entry = data.iter().find(|d| d.txid == *txid);
            match entry {
                None => {
                    r.results.push(StatusForTxidResult {
                        txid: txid.clone(),
                        status: "unknown".to_string(),
                        depth: None,
                    });
                }
                Some(d) if d.error.as_deref() == Some("unknown") => {
                    r.results.push(StatusForTxidResult {
                        txid: txid.clone(),
                        status: "unknown".to_string(),
                        depth: None,
                    });
                }
                Some(d) if d.error.is_some() => {
                    // Unexpected error
                    r.results.push(StatusForTxidResult {
                        txid: txid.clone(),
                        status: "unknown".to_string(),
                        depth: None,
                    });
                }
                Some(d) => {
                    if d.confirmations.is_none() || d.confirmations == Some(0) {
                        r.results.push(StatusForTxidResult {
                            txid: txid.clone(),
                            status: "known".to_string(),
                            depth: Some(0),
                        });
                    } else {
                        r.results.push(StatusForTxidResult {
                            txid: txid.clone(),
                            status: "mined".to_string(),
                            depth: d.confirmations,
                        });
                    }
                }
            }
        }

        r.status = "success".to_string();
        r
    }
}

// ---------------------------------------------------------------------------
// GetScriptHashHistoryProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetScriptHashHistoryProvider for WhatsOnChain {
    fn name(&self) -> &str {
        "WoC"
    }

    async fn get_script_hash_history(&self, hash: &str) -> GetScriptHashHistoryResult {
        // Confirmed history
        let confirmed = self
            .get_script_hash_history_inner(hash, "confirmed/history")
            .await;
        if confirmed.status != "success" || confirmed.error.is_some() {
            return confirmed;
        }

        // Unconfirmed history
        let unconfirmed = self
            .get_script_hash_history_inner(hash, "unconfirmed/history")
            .await;
        if unconfirmed.status != "success" || unconfirmed.error.is_some() {
            return unconfirmed;
        }

        // Combine: confirmed first, then unconfirmed
        let mut combined = confirmed;
        combined.history.extend(unconfirmed.history);
        combined
    }
}

impl WhatsOnChain {
    /// Internal helper to fetch script hash history (confirmed or unconfirmed).
    ///
    /// WoC expects big-endian hash in URL, but callers pass LE hash.
    /// Reverse LE -> BE before making the API call.
    async fn get_script_hash_history_inner(
        &self,
        hash: &str,
        endpoint: &str,
    ) -> GetScriptHashHistoryResult {
        let mut r = GetScriptHashHistoryResult {
            name: "WoC".to_string(),
            status: "error".to_string(),
            error: None,
            history: Vec::new(),
        };

        // Reverse hash from LE to BE for WoC
        let be_hash = match reverse_hex_hash(hash) {
            Ok(h) => h,
            Err(e) => {
                r.error = Some(format!("Invalid hash: {}", e));
                return r;
            }
        };

        let url = format!("{}/script/{}/{}", self.base_url, be_hash, endpoint);

        // Retry loop for ECONNRESET
        for retry in 0u32.. {
            let resp = match self.get_with_retry(&url).await {
                Ok(resp) => resp,
                Err(e) => {
                    if retry > 2 {
                        r.error = Some(format!(
                            "WoC getScriptHashHistory service failure: {}, error: {}",
                            url, e
                        ));
                        return r;
                    }
                    continue;
                }
            };

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                r.status = "success".to_string();
                return r;
            }

            if !resp.status().is_success() {
                r.error = Some(format!(
                    "WoC getScriptHashHistory response {} {}",
                    resp.status().is_success(),
                    resp.status()
                ));
                return r;
            }

            let data: WocScriptHashHistoryResponse = match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    if retry > 2 {
                        r.error = Some(format!("Failed to parse response: {}", e));
                        return r;
                    }
                    continue;
                }
            };

            if let Some(error) = data.error {
                r.error = Some(format!("WoC getScriptHashHistory error {}", error));
                return r;
            }

            // WoC returns tx_hash in LE format; we keep them as-is (matching TS behavior
            // where the history entries use the tx_hash directly from WoC)
            r.history = data
                .result
                .into_iter()
                .map(|entry| ScriptHashHistoryEntry {
                    txid: entry.tx_hash,
                    height: entry.height,
                })
                .collect();

            r.status = "success".to_string();
            return r;
        }

        r
    }

    /// Get a block header by its hash.
    /// Used internally for hash_to_header lookups.
    pub async fn get_block_header_by_hash(
        &self,
        hash: &str,
    ) -> Result<Option<BlockHeader>, WalletError> {
        let url = format!("{}/block/{}/header", self.base_url, hash);

        let resp = self.get_with_retry(&url).await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(WalletError::InvalidParameter {
                parameter: "hash".to_string(),
                must_be: format!("valid block hash. '{}' response {}", hash, resp.status()),
            });
        }

        let woc: WocBlockHeader = resp
            .json()
            .await
            .map_err(|e| WalletError::Internal(format!("Failed to parse block header: {}", e)))?;

        let previous_hash = woc.previousblockhash.unwrap_or_else(|| {
            "0000000000000000000000000000000000000000000000000000000000000000".to_string()
        });

        Ok(Some(BlockHeader {
            version: woc.version,
            previous_hash,
            merkle_root: woc.merkleroot,
            time: woc.time,
            bits: woc.bits,
            nonce: woc.nonce,
            hash: woc.hash,
            height: woc.height,
        }))
    }

    /// Get the BSV exchange rate from WoC.
    pub async fn update_bsv_exchange_rate(&self) -> Result<f64, WalletError> {
        let url = format!("{}/exchangerate", self.base_url);
        let resp = self.get_with_retry(&url).await?;

        if !resp.status().is_success() {
            return Err(WalletError::InvalidOperation(format!(
                "WoC exchangerate response {}",
                resp.status()
            )));
        }

        #[derive(Deserialize)]
        struct WocExchangeRate {
            rate: f64,
            currency: String,
        }

        let woc_rate: WocExchangeRate = resp
            .json()
            .await
            .map_err(|e| WalletError::Internal(format!("Failed to parse exchange rate: {}", e)))?;

        if woc_rate.currency != "USD" {
            return Err(WalletError::Internal(
                "WoC exchange rate currency is not USD".to_string(),
            ));
        }

        Ok(woc_rate.rate)
    }
}
