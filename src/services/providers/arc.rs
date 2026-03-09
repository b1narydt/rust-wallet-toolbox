//! ARC broadcaster implementing PostBeefProvider.
//!
//! Ported from wallet-toolbox/src/services/providers/ARC.ts.
//! Handles BEEF V2 to V1 downgrade, callback URL/token support,
//! and API key authentication.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use tracing;

use crate::services::traits::PostBeefProvider;
use crate::services::types::{ArcConfig, PostBeefResult, PostTxResultForTxid};

/// ARC API JSON response structure.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArcResponse {
    txid: String,
    #[serde(default)]
    extra_info: Option<String>,
    #[serde(default)]
    tx_status: Option<String>,
    #[serde(default)]
    competing_txs: Option<Vec<String>>,
}

/// ARC transaction broadcaster.
///
/// Named `ArcProvider` to avoid conflict with `std::sync::Arc`.
/// Implements `PostBeefProvider` for BEEF transaction broadcasting.
pub struct ArcProvider {
    name: String,
    base_url: String,
    config: ArcConfig,
    client: reqwest::Client,
}

impl ArcProvider {
    /// Create a new ARC provider.
    pub fn new(name: &str, base_url: &str, config: ArcConfig, client: reqwest::Client) -> Self {
        Self {
            name: name.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            config,
            client,
        }
    }

    /// Build request headers from the ARC configuration.
    ///
    /// Exposed as a standalone function for testability.
    pub fn build_headers(&self) -> HeaderMap {
        build_arc_headers(&self.config)
    }
}

/// Build the HTTP headers for an ARC request from the given config.
///
/// Extracted as a pub(crate) helper so it can be unit-tested independently.
pub fn build_arc_headers(config: &ArcConfig) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/octet-stream"),
    );

    headers.insert(
        "XDeployment-ID",
        HeaderValue::from_str(&config.deployment_id).unwrap_or_else(|_| {
            HeaderValue::from_static("wallet-toolbox")
        }),
    );

    if let Some(ref api_key) = config.api_key {
        if !api_key.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
                headers.insert("Authorization", val);
            }
        }
    }

    if let Some(ref callback_url) = config.callback_url {
        if !callback_url.is_empty() {
            if let Ok(val) = HeaderValue::from_str(callback_url) {
                headers.insert("X-CallbackUrl", val);
            }
        }
    }

    if let Some(ref callback_token) = config.callback_token {
        if !callback_token.is_empty() {
            if let Ok(val) = HeaderValue::from_str(callback_token) {
                headers.insert("X-CallbackToken", val);
            }
        }
    }

    headers
}

/// BEEF V2 version marker bytes (first 4 bytes, little-endian).
/// BEEF_V2 = 0xEFBE0002, stored as LE: [0x02, 0x00, 0xBE, 0xEF]
const BEEF_V2_BYTES: [u8; 4] = [0x02, 0x00, 0xBE, 0xEF];

/// Attempt to downgrade BEEF V2 to V1 format.
///
/// If the BEEF is V2 and all transactions have full raw data (no txid-only entries),
/// the version bytes are changed to V1. Otherwise the original bytes are returned unchanged.
///
/// BEEF V2 differs from V1 in that each transaction has a format byte prefix:
///   0 = raw tx without bump, 1 = raw tx with bump index, 2 = txid only.
/// If any tx has format byte 2 (txid-only), we cannot downgrade.
pub fn maybe_downgrade_beef_v2(beef: &[u8]) -> Vec<u8> {
    if beef.len() < 4 {
        return beef.to_vec();
    }

    // Check if this is V2
    if beef[0..4] != BEEF_V2_BYTES {
        return beef.to_vec();
    }

    // Parse the BEEF to check for txid-only entries.
    // We use the bsv-sdk Beef parser to check reliably.
    let mut cursor = std::io::Cursor::new(beef);
    match bsv::transaction::Beef::from_binary(&mut cursor) {
        Ok(parsed_beef) => {
            // Check if any tx is txid-only
            if parsed_beef.txs.iter().any(|btx| btx.is_txid_only()) {
                // Cannot downgrade -- has txid-only entries
                return beef.to_vec();
            }

            // All txs have raw data, safe to downgrade.
            // Re-serialize with V1 version.
            let mut downgraded = parsed_beef;
            downgraded.version = bsv::transaction::beef::BEEF_V1;
            let mut out = Vec::new();
            match downgraded.to_binary(&mut out) {
                Ok(()) => {
                    tracing::debug!("Downgraded BEEF V2 to V1 ({} bytes -> {} bytes)", beef.len(), out.len());
                    out
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize downgraded BEEF: {}", e);
                    beef.to_vec()
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to parse BEEF for V2 downgrade check: {}", e);
            beef.to_vec()
        }
    }
}

#[async_trait]
impl PostBeefProvider for ArcProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> PostBeefResult {
        // Attempt BEEF V2 -> V1 downgrade
        let beef_bytes = maybe_downgrade_beef_v2(beef);

        let url = format!("{}/v1/tx", self.base_url);
        let headers = self.build_headers();

        let result = self
            .client
            .post(&url)
            .headers(headers)
            .body(beef_bytes)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await;

        match result {
            Ok(response) => {
                let status_code = response.status();
                match response.json::<ArcResponse>().await {
                    Ok(arc_resp) => {
                        let tx_status = arc_resp.tx_status.unwrap_or_default();
                        let extra_info = arc_resp.extra_info.unwrap_or_default();

                        let is_double_spend = tx_status == "DOUBLE_SPEND_ATTEMPTED"
                            || tx_status == "SEEN_IN_ORPHAN_MEMPOOL";

                        let tx_result = PostTxResultForTxid {
                            txid: arc_resp.txid,
                            status: if status_code.is_success() && !is_double_spend {
                                "success".to_string()
                            } else {
                                "error".to_string()
                            },
                            already_known: None,
                            double_spend: if is_double_spend { Some(true) } else { None },
                            block_hash: None,
                            block_height: None,
                            competing_txs: arc_resp.competing_txs,
                            service_error: if !status_code.is_success() && !is_double_spend {
                                Some(true)
                            } else {
                                None
                            },
                        };

                        let overall_status = if tx_result.status == "success" {
                            "success"
                        } else {
                            "error"
                        };

                        PostBeefResult {
                            name: self.name.clone(),
                            status: overall_status.to_string(),
                            error: if overall_status == "error" {
                                Some(format!("{} {}", tx_status, extra_info))
                            } else {
                                None
                            },
                            txid_results: vec![tx_result],
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[{}] Failed to parse ARC response (status {}): {}",
                            self.name, status_code, e
                        );
                        PostBeefResult {
                            name: self.name.clone(),
                            status: "error".to_string(),
                            error: Some(format!("HTTP {} - parse error: {}", status_code, e)),
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
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("[{}] ARC request failed: {}", self.name, e);
                PostBeefResult {
                    name: self.name.clone(),
                    status: "error".to_string(),
                    error: Some(format!("Request failed: {}", e)),
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
                }
            }
        }
    }
}
