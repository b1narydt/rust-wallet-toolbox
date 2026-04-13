//! Bitails provider implementing GetMerklePathProvider and PostBeefProvider.
//!
//! Ported from wallet-toolbox/src/services/providers/Bitails.ts.
//! Bitails supports main and test chains only.

use async_trait::async_trait;
use serde::Deserialize;

use crate::services::traits::{GetMerklePathProvider, PostBeefProvider, WalletServices};
use crate::services::types::{GetMerklePathResult, PostBeefResult, PostTxResultForTxid};
use crate::types::Chain;

use super::whats_on_chain::{bytes_to_hex, convert_proof_to_merkle_path, double_sha256_be};

// ---------------------------------------------------------------------------
// Bitails API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BitailsTscProof {
    index: u64,
    nodes: Vec<String>,
    target: String,
    #[allow(dead_code)]
    #[serde(rename = "txOrId")]
    tx_or_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitailsPostRawsResultEntry {
    txid: Option<String>,
    error: Option<BitailsPostError>,
}

#[derive(Debug, Deserialize)]
struct BitailsPostError {
    code: i32,
    #[allow(dead_code)]
    message: String,
}

// ---------------------------------------------------------------------------
// Bitails provider
// ---------------------------------------------------------------------------

/// Bitails HTTP API provider.
///
/// Implements GetMerklePathProvider and PostBeefProvider.
/// Only available for Main and Test chains.
pub struct Bitails {
    #[allow(dead_code)]
    chain: Chain,
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

impl Bitails {
    /// Create a new Bitails provider.
    ///
    /// Only Main and Test chains are supported.
    pub fn new(chain: Chain, api_key: Option<String>, client: reqwest::Client) -> Self {
        let base_url = match chain {
            Chain::Main => "https://api.bitails.io".to_string(),
            Chain::Test => "https://test-api.bitails.io".to_string(),
        };
        Self {
            chain,
            api_key,
            base_url,
            client,
        }
    }

    /// Return the provider name string.
    pub fn name_str(&self) -> &str {
        "BitailsTsc"
    }

    /// Build request headers with auth if api_key is set.
    fn request_builder(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.client.request(method, url);
        builder = builder.header("Accept", "application/json");
        if let Some(ref key) = self.api_key {
            if !key.is_empty() {
                builder = builder.header("Authorization", key);
            }
        }
        builder
    }
}

// ---------------------------------------------------------------------------
// GetMerklePathProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl GetMerklePathProvider for Bitails {
    fn name(&self) -> &str {
        "BitailsTsc"
    }

    async fn get_merkle_path(
        &self,
        txid: &str,
        services: &dyn WalletServices,
    ) -> GetMerklePathResult {
        let url = format!("{}/tx/{}/proof/tsc", self.base_url, txid);

        let resp = match self
            .request_builder(reqwest::Method::GET, &url)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("BitailsTsc".to_string()),
                    error: Some(format!("HTTP request failed: {}", e)),
                    ..Default::default()
                };
            }
        };

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return GetMerklePathResult {
                name: Some("BitailsTsc".to_string()),
                ..Default::default()
            };
        }

        if !resp.status().is_success() {
            return GetMerklePathResult {
                name: Some("BitailsTsc".to_string()),
                error: Some(format!("Bitails getMerklePath status {}", resp.status())),
                ..Default::default()
            };
        }

        let proof: BitailsTscProof = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("BitailsTsc".to_string()),
                    error: Some(format!("Failed to parse proof: {}", e)),
                    ..Default::default()
                };
            }
        };

        // Look up block header by hash to get height
        let header = match services.hash_to_header(&proof.target).await {
            Ok(h) => h,
            Err(e) => {
                return GetMerklePathResult {
                    name: Some("BitailsTsc".to_string()),
                    error: Some(format!("hashToHeader failed: {}", e)),
                    ..Default::default()
                };
            }
        };

        // Convert TSC proof to MerklePath
        match convert_proof_to_merkle_path(txid, proof.index, &proof.nodes, header.height) {
            Ok(merkle_path_bytes) => GetMerklePathResult {
                name: Some("BitailsTsc".to_string()),
                merkle_path: Some(merkle_path_bytes),
                header: Some(header),
                error: None,
            },
            Err(e) => GetMerklePathResult {
                name: Some("BitailsTsc".to_string()),
                error: Some(format!("Failed to convert proof: {}", e)),
                ..Default::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PostBeefProvider
// ---------------------------------------------------------------------------

#[async_trait]
impl PostBeefProvider for Bitails {
    fn name(&self) -> &str {
        "Bitails"
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> PostBeefResult {
        // Bitails does not accept BEEF directly. Decompose into individual raw transactions.
        let parsed_beef = match bsv::transaction::Beef::from_binary(&mut std::io::Cursor::new(beef))
        {
            Ok(b) => b,
            Err(e) => {
                return PostBeefResult {
                    name: "Bitails".to_string(),
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
                            orphan_mempool: None,
                        })
                        .collect(),
                };
            }
        };

        // Extract raw hex strings for all txids
        let mut raws: Vec<String> = Vec::new();
        let mut raw_txids: Vec<String> = Vec::new();

        for beef_tx in &parsed_beef.txs {
            if let Some(ref tx) = beef_tx.tx {
                let mut buf = Vec::new();
                if tx.to_binary(&mut buf).is_ok() {
                    let raw_hex = bytes_to_hex(&buf);
                    let computed_txid = bytes_to_hex(&double_sha256_be(&buf));
                    raw_txids.push(computed_txid);
                    raws.push(raw_hex);
                }
            }
        }

        // Build per-txid result tracking
        let mut result = PostBeefResult {
            name: "Bitails".to_string(),
            status: "success".to_string(),
            error: None,
            txid_results: txids
                .iter()
                .map(|txid| PostTxResultForTxid {
                    txid: txid.clone(),
                    status: "success".to_string(),
                    already_known: None,
                    double_spend: None,
                    block_hash: None,
                    block_height: None,
                    competing_txs: None,
                    service_error: None,
                    orphan_mempool: None,
                })
                .collect(),
        };

        if raws.is_empty() {
            result.status = "error".to_string();
            result.error = Some("No raw transactions extracted from BEEF".to_string());
            return result;
        }

        // POST /tx/broadcast/multi with JSON body { "raws": [...] }
        let url = format!("{}/tx/broadcast/multi", self.base_url);
        let body = serde_json::json!({ "raws": raws });

        let resp = match self
            .request_builder(reqwest::Method::POST, &url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                result.status = "error".to_string();
                result.error = Some(format!("HTTP request failed: {}", e));
                for tr in &mut result.txid_results {
                    tr.status = "error".to_string();
                    tr.service_error = Some(true);
                }
                return result;
            }
        };

        if !resp.status().is_success() {
            result.status = "error".to_string();
            result.error = Some(format!("Bitails postBeef status {}", resp.status()));
            for tr in &mut result.txid_results {
                tr.status = "error".to_string();
                tr.service_error = Some(true);
            }
            return result;
        }

        // Parse response: array of results
        let btrs: Vec<BitailsPostRawsResultEntry> = match resp.json().await {
            Ok(d) => d,
            Err(e) => {
                result.status = "error".to_string();
                result.error = Some(format!("Failed to parse response: {}", e));
                return result;
            }
        };

        if btrs.len() != raws.len() {
            result.status = "error".to_string();
            result.error = Some("Response results count mismatch".to_string());
            return result;
        }

        // Map responses back to requested txids
        for (i, btr) in btrs.iter().enumerate() {
            let btr_txid = btr.txid.clone().unwrap_or_else(|| {
                if i < raw_txids.len() {
                    raw_txids[i].clone()
                } else {
                    String::new()
                }
            });

            // Find the matching result entry
            if let Some(tr) = result.txid_results.iter_mut().find(|r| r.txid == btr_txid) {
                if let Some(ref err) = btr.error {
                    if err.code == -27 {
                        // already-in-mempool -- treat as success
                        tr.already_known = Some(true);
                    } else if err.code == -25 {
                        // missing-inputs -- possible double spend
                        tr.status = "error".to_string();
                        tr.double_spend = Some(true);
                    } else {
                        tr.status = "error".to_string();
                        tr.service_error = Some(true);
                    }
                }
                // No error = success (already initialized)
            }
        }

        // Update overall status
        if result.txid_results.iter().any(|tr| tr.status != "success") {
            result.status = "error".to_string();
        }

        result
    }
}
