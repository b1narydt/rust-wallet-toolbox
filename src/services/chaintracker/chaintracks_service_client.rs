//! HTTP client for the Chaintracks API with retry on connection reset.
//!
//! Ported from wallet-toolbox/src/services/chaintracker/chaintracks/ChaintracksServiceClient.ts.

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::{WalletError, WalletResult};
use crate::services::types::{BlockHeader, FiatExchangeRates};
use crate::types::Chain;

/// Maximum number of retries on connection reset errors.
const MAX_RETRIES: u32 = 3;

/// Wrapper for Chaintracks API JSON responses.
#[derive(Debug, Deserialize)]
struct FetchStatus<T> {
    status: String,
    value: Option<T>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// HTTP client for the Chaintracks block header service.
///
/// Provides methods to query block headers by height or hash,
/// get the chain tip, and fetch exchange rates.
pub struct ChaintracksServiceClient {
    chain: Chain,
    service_url: String,
    client: reqwest::Client,
}

impl ChaintracksServiceClient {
    /// Create a new Chaintracks service client.
    ///
    /// If `service_url` is None, the default URL for the given chain is used:
    /// - Main: `https://mainnet-chaintracks.babbage.systems`
    /// - Test: `https://testnet-chaintracks.babbage.systems`
    pub fn new(chain: Chain, service_url: Option<&str>, client: reqwest::Client) -> Self {
        let url = match service_url {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => {
                let net = match chain {
                    Chain::Main => "mainnet",
                    Chain::Test => "testnet",
                };
                format!("https://{}-chaintracks.babbage.systems", net)
            }
        };

        Self {
            chain,
            service_url: url,
            client,
        }
    }

    /// The chain this client services.
    pub fn chain(&self) -> Chain {
        self.chain.clone()
    }

    /// The service URL this client connects to.
    pub fn service_url(&self) -> &str {
        &self.service_url
    }

    /// GET a JSON response from the Chaintracks API, with retry on connection reset.
    ///
    /// Returns `Ok(None)` if the API returns a non-success status (e.g., "not found").
    /// Returns `Ok(Some(value))` on success.
    /// Retries up to MAX_RETRIES times on connection reset errors.
    pub async fn get_json_or_none<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> WalletResult<Option<T>> {
        let url = format!("{}{}", self.service_url, path);
        let mut last_error: Option<WalletError> = None;

        for retry in 0..MAX_RETRIES {
            match self.client.get(&url).send().await {
                Ok(response) => {
                    let text = response.text().await.map_err(|e| {
                        WalletError::Internal(format!(
                            "Failed to read response body from {}: {}",
                            url, e
                        ))
                    })?;

                    let fetch_status: FetchStatus<T> =
                        serde_json::from_str(&text).map_err(|e| {
                            WalletError::Internal(format!(
                                "Failed to parse Chaintracks response from {}: {} (body: {})",
                                url,
                                e,
                                &text[..text.len().min(200)]
                            ))
                        })?;

                    if fetch_status.status == "success" {
                        return Ok(fetch_status.value);
                    } else {
                        last_error = Some(WalletError::Internal(format!(
                            "Chaintracks API error: status={}, code={}, desc={}",
                            fetch_status.status,
                            fetch_status.code.unwrap_or_default(),
                            fetch_status.description.unwrap_or_default()
                        )));
                        // Only retry on connection-related errors
                        break;
                    }
                }
                Err(e) => {
                    let is_connection_reset = e.to_string().contains("connection reset")
                        || e.to_string().contains("ECONNRESET")
                        || e.is_connect();

                    last_error = Some(WalletError::Internal(format!(
                        "Chaintracks request failed for {}: {}",
                        url, e
                    )));

                    if !is_connection_reset || retry + 1 >= MAX_RETRIES {
                        break;
                    }

                    tracing::debug!(
                        "Chaintracks connection reset, retrying ({}/{}): {}",
                        retry + 1,
                        MAX_RETRIES,
                        url
                    );
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            WalletError::Internal("Chaintracks request failed with no error details".to_string())
        }))
    }

    /// Get a block header by height.
    pub async fn get_header_for_height(&self, height: u32) -> WalletResult<Option<BlockHeader>> {
        self.get_json_or_none(&format!("/findHeaderHexForHeight?height={}", height))
            .await
    }

    /// Get a block header by block hash.
    pub async fn get_header_for_block_hash(&self, hash: &str) -> WalletResult<Option<BlockHeader>> {
        self.get_json_or_none(&format!("/findHeaderHexForBlockHash?hash={}", hash))
            .await
    }

    /// Get the current chain tip header.
    pub async fn get_chain_tip(&self) -> WalletResult<Option<BlockHeader>> {
        self.get_json_or_none("/findChainTipHeaderHex").await
    }

    /// Get the current chain tip height.
    pub async fn get_present_height(&self) -> WalletResult<u32> {
        let height: Option<u32> = self.get_json_or_none("/getPresentHeight").await?;
        height.ok_or_else(|| {
            WalletError::Internal("Chaintracks returned no height value".to_string())
        })
    }

    /// Get fiat exchange rates for a base currency.
    pub async fn get_fiat_exchange_rates(
        &self,
        base: &str,
    ) -> WalletResult<Option<FiatExchangeRates>> {
        self.get_json_or_none(&format!("/api/v1/exchangeRates/{}", base))
            .await
    }
}
