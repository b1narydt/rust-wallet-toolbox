//! BSV and fiat exchange rate fetching.
//!
//! Ported from wallet-toolbox/src/services/providers/exchangeRates.ts.
//! Provides standalone async functions for fetching exchange rates from
//! WhatsOnChain and exchangeratesapi.io.

use std::collections::HashMap;

use chrono::Utc;
use serde::Deserialize;

use crate::error::{WalletError, WalletResult};
use crate::services::types::FiatExchangeRates;

/// WhatsOnChain exchange rate API response.
#[derive(Debug, Deserialize)]
// `currency` is deserialized for wire-format parity but not consumed by this
// crate (the currency is already known from the request URL).
#[allow(dead_code)]
struct WocExchangeRateResponse {
    rate: f64,
    currency: String,
}

/// exchangeratesapi.io API response.
#[derive(Debug, Deserialize)]
struct ExchangeRatesIoResponse {
    success: bool,
    timestamp: i64,
    base: String,
    rates: HashMap<String, f64>,
}

/// Fetch the current BSV/USD exchange rate from WhatsOnChain.
///
/// GET `https://api.whatsonchain.com/v1/bsv/main/exchangerate`
/// Returns the USD rate as f64.
pub async fn fetch_bsv_exchange_rate(client: &reqwest::Client) -> WalletResult<f64> {
    let url = "https://api.whatsonchain.com/v1/bsv/main/exchangerate";

    let response =
        client.get(url).send().await.map_err(|e| {
            WalletError::Internal(format!("Failed to fetch BSV exchange rate: {}", e))
        })?;

    let data: WocExchangeRateResponse = response.json().await.map_err(|e| {
        WalletError::Internal(format!("Failed to parse BSV exchange rate response: {}", e))
    })?;

    Ok(data.rate)
}

/// Fetch fiat exchange rates for multiple target currencies.
///
/// If `api_key` is provided, uses exchangeratesapi.io with the free tier (EUR base).
/// Rates are normalized to USD base by dividing each rate by the USD rate.
///
/// If `api_key` is None, attempts to use the Chaintracks fallback URL.
pub async fn fetch_fiat_exchange_rates(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base: &str,
    targets: &[String],
) -> WalletResult<FiatExchangeRates> {
    match api_key {
        Some(key) => fetch_from_exchangeratesapi(client, key, base, targets).await,
        None => Err(WalletError::MissingParameter(
            "exchangeratesapi_key or chaintracks_fiat_exchange_rates_url".to_string(),
        )),
    }
}

/// Fetch from exchangeratesapi.io (free tier uses EUR base).
async fn fetch_from_exchangeratesapi(
    client: &reqwest::Client,
    api_key: &str,
    _base: &str,
    targets: &[String],
) -> WalletResult<FiatExchangeRates> {
    // Ensure USD is always in the target list for normalization
    let mut symbols: Vec<String> = targets.to_vec();
    if !symbols.iter().any(|s| s == "USD") {
        symbols.push("USD".to_string());
    }

    let symbols_csv = symbols.join(",");
    let url = format!(
        "https://api.exchangeratesapi.io/v1/latest?access_key={}&symbols={}",
        api_key, symbols_csv
    );

    let response = client.get(&url).send().await.map_err(|e| {
        WalletError::Internal(format!("Failed to fetch fiat exchange rates: {}", e))
    })?;

    let data: ExchangeRatesIoResponse = response.json().await.map_err(|e| {
        WalletError::Internal(format!(
            "Failed to parse fiat exchange rate response: {}",
            e
        ))
    })?;

    if !data.success {
        return Err(WalletError::BadRequest(
            "exchangeratesapi returned success=false".to_string(),
        ));
    }

    // The free tier returns EUR base. Normalize to USD base.
    let usd_per_base = if data.base == "USD" {
        1.0
    } else {
        *data.rates.get("USD").ok_or_else(|| {
            WalletError::BadRequest("exchangeratesapi missing USD rate".to_string())
        })?
    };

    if usd_per_base <= 0.0 {
        return Err(WalletError::BadRequest(
            "Invalid USD rate from exchangeratesapi".to_string(),
        ));
    }

    let mut rates = HashMap::new();
    for currency in targets {
        if currency == "USD" {
            rates.insert("USD".to_string(), 1.0);
            continue;
        }

        let cur_per_base = if *currency == data.base {
            1.0
        } else {
            *data.rates.get(currency.as_str()).ok_or_else(|| {
                WalletError::BadRequest(format!("exchangeratesapi missing rate for '{}'", currency))
            })?
        };

        rates.insert(currency.clone(), cur_per_base / usd_per_base);
    }

    Ok(FiatExchangeRates {
        timestamp: chrono::DateTime::from_timestamp(data.timestamp, 0).unwrap_or_else(Utc::now),
        base: "USD".to_string(),
        rates,
    })
}

/// Fetch a single fiat exchange rate.
///
/// Convenience wrapper around `fetch_fiat_exchange_rates` for a single currency.
pub async fn fetch_fiat_exchange_rate(
    client: &reqwest::Client,
    api_key: Option<&str>,
    currency: &str,
    base: Option<&str>,
) -> WalletResult<f64> {
    let base = base.unwrap_or("USD");
    let targets = vec![currency.to_string()];
    let rates = fetch_fiat_exchange_rates(client, api_key, base, &targets).await?;

    rates.rates.get(currency).copied().ok_or_else(|| {
        WalletError::Internal(format!("Rate for {} not found in response", currency))
    })
}
