//! Services struct implementing WalletServices trait.
//!
//! Wires together all providers (WhatsOnChain, ARC, Bitails, ChainTracker)
//! into a single struct that consumers use via `Arc<dyn WalletServices>`.
//!
//! Ported from wallet-toolbox/src/services/Services.ts.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::Beef;
use chrono::Utc;
use tokio::sync::Mutex;

use crate::error::{WalletError, WalletResult};
use crate::types::Chain;

use super::chaintracker::{ChaintracksChainTracker, ChaintracksServiceClient};
use super::providers::exchange_rates::{fetch_bsv_exchange_rate, fetch_fiat_exchange_rates};
use super::providers::{ArcProvider, ArcadeProvider, Bitails, WhatsOnChain};
use super::service_collection::ServiceCollection;
use super::traits::{
    GetMerklePathProvider, GetRawTxProvider, GetScriptHashHistoryProvider,
    GetStatusForTxidsProvider, GetUtxoStatusProvider, PostBeefProvider, WalletServices,
};
use super::types::{
    BlockHeader, BsvExchangeRate, FiatExchangeRates, GetMerklePathResult, GetRawTxResult,
    GetScriptHashHistoryResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat,
    GetUtxoStatusResult, NLockTimeInput, PostBeefMode, PostBeefResult, ServiceCall,
    ServicesCallHistory, ServicesConfig,
};
use bsv::transaction::beef::BEEF_V2;
use bsv::transaction::beef_tx::BeefTx;
use bsv::transaction::merkle_path::MerklePath;
use bsv::transaction::Transaction as BsvTransaction;

/// The main services orchestrator struct.
///
/// Owns all provider instances and `ServiceCollection`s, exposes the
/// `WalletServices` trait for consumption by Wallet/Monitor.
pub struct Services {
    config: ServicesConfig,
    client: reqwest::Client,
    get_merkle_path: Mutex<ServiceCollection<dyn GetMerklePathProvider>>,
    get_raw_tx: Mutex<ServiceCollection<dyn GetRawTxProvider>>,
    post_beef: Mutex<ServiceCollection<dyn PostBeefProvider>>,
    get_utxo_status: Mutex<ServiceCollection<dyn GetUtxoStatusProvider>>,
    get_status_for_txids: Mutex<ServiceCollection<dyn GetStatusForTxidsProvider>>,
    get_script_hash_history: Mutex<ServiceCollection<dyn GetScriptHashHistoryProvider>>,
    chain_tracker: ChaintracksChainTracker,
    post_beef_mode: PostBeefMode,
    bsv_exchange_rate: Mutex<BsvExchangeRate>,
    fiat_exchange_rates: Mutex<FiatExchangeRates>,
}

impl Services {
    /// Create a Services instance from a full configuration.
    pub fn from_config(config: ServicesConfig) -> Self {
        let client = reqwest::Client::new();
        let chain = config.chain.clone();

        let has_bitails = matches!(chain, Chain::Main | Chain::Test);

        // -- getMerklePath collection --
        let mut get_merkle_path_coll =
            ServiceCollection::<dyn GetMerklePathProvider>::new("getMerklePath");
        // Need a second WoC for getMerklePath since the first is consumed by getRawTx etc.
        let woc_merkle = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_merkle_path_coll.add("WhatsOnChain", Arc::new(woc_merkle));
        if has_bitails {
            let bitails = Bitails::new(
                chain.clone(),
                config.bitails_api_key.clone(),
                client.clone(),
            );
            get_merkle_path_coll.add("Bitails", Arc::new(bitails));
        }

        // -- getRawTx collection --
        let mut get_raw_tx_coll = ServiceCollection::<dyn GetRawTxProvider>::new("getRawTx");
        let woc_raw = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_raw_tx_coll.add("WhatsOnChain", Arc::new(woc_raw));

        // -- postBeef collection --
        // Arcade is the primary broadcaster when configured, followed by the
        // existing ARC and explorer fallbacks.
        let mut post_beef_coll = ServiceCollection::<dyn PostBeefProvider>::new("postBeef");

        if let Some(ref arcade_url) = config.arcade_url {
            if !arcade_url.is_empty() {
                let arcade_config = config.arcade_config.clone().unwrap_or_default();
                let arcade = ArcadeProvider::new(arcade_url, arcade_config, client.clone());
                post_beef_coll.add("ArcadeBeef", Arc::new(arcade));
            }
        }

        if let Some(ref gp_url) = config.arc_gorilla_pool_url {
            let gp_config = config.arc_gorilla_pool_config.clone().unwrap_or_default();
            let arc_gp = ArcProvider::new("GorillaPoolArcBeef", gp_url, gp_config, client.clone());
            post_beef_coll.add("GorillaPoolArcBeef", Arc::new(arc_gp));
        }

        let arc_taal = ArcProvider::new(
            "TaalArcBeef",
            &config.arc_url,
            config.arc_config.clone(),
            client.clone(),
        );
        post_beef_coll.add("TaalArcBeef", Arc::new(arc_taal));

        if has_bitails {
            let bitails_beef = Bitails::new(
                chain.clone(),
                config.bitails_api_key.clone(),
                client.clone(),
            );
            post_beef_coll.add("Bitails", Arc::new(bitails_beef));
        }

        let woc_beef = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        post_beef_coll.add("WhatsOnChain", Arc::new(woc_beef));

        // -- getUtxoStatus collection --
        let mut get_utxo_status_coll =
            ServiceCollection::<dyn GetUtxoStatusProvider>::new("getUtxoStatus");
        let woc_utxo = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_utxo_status_coll.add("WhatsOnChain", Arc::new(woc_utxo));

        // -- getStatusForTxids collection --
        let mut get_status_for_txids_coll =
            ServiceCollection::<dyn GetStatusForTxidsProvider>::new("getStatusForTxids");
        let woc_status = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_status_for_txids_coll.add("WhatsOnChain", Arc::new(woc_status));

        // -- getScriptHashHistory collection --
        let mut get_script_hash_history_coll =
            ServiceCollection::<dyn GetScriptHashHistoryProvider>::new("getScriptHashHistory");
        let woc_history = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_script_hash_history_coll.add("WhatsOnChain", Arc::new(woc_history));

        // -- ChainTracker --
        let chaintracks_url = config.chaintracks_url.as_deref();
        let service_client =
            ChaintracksServiceClient::new(chain.clone(), chaintracks_url, client.clone());
        let chain_tracker = ChaintracksChainTracker::new(service_client);

        Services {
            bsv_exchange_rate: Mutex::new(config.bsv_exchange_rate.clone()),
            fiat_exchange_rates: Mutex::new(config.fiat_exchange_rates.clone()),
            config,
            client,
            get_merkle_path: Mutex::new(get_merkle_path_coll),
            get_raw_tx: Mutex::new(get_raw_tx_coll),
            post_beef: Mutex::new(post_beef_coll),
            get_utxo_status: Mutex::new(get_utxo_status_coll),
            get_status_for_txids: Mutex::new(get_status_for_txids_coll),
            get_script_hash_history: Mutex::new(get_script_hash_history_coll),
            chain_tracker,
            post_beef_mode: PostBeefMode::UntilSuccess,
        }
    }

    /// Create Services from a chain with default configuration.
    pub fn from_chain(chain: Chain) -> Self {
        let config = ServicesConfig::from(chain);
        Self::from_config(config)
    }

    /// Create default services and optionally install Arcade as the primary
    /// broadcaster using the callback token consumed by its SSE stream.
    pub(crate) fn from_chain_with_arcade(
        chain: Chain,
        arcade_url: Option<String>,
        callback_token: Option<String>,
    ) -> Self {
        let mut config = ServicesConfig::from(chain);
        if let Some(url) = arcade_url.filter(|url| !url.is_empty()) {
            let arcade_config = super::types::ArcConfig {
                callback_token,
                ..Default::default()
            };
            config.arcade_url = Some(url);
            config.arcade_config = Some(arcade_config);
        }
        Self::from_config(config)
    }

    /// Access the underlying config.
    pub fn config(&self) -> &ServicesConfig {
        &self.config
    }

    /// Access the shared HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Set the postBeef mode.
    pub fn set_post_beef_mode(&mut self, mode: PostBeefMode) {
        self.post_beef_mode = mode;
    }
}

impl From<Chain> for Services {
    fn from(chain: Chain) -> Self {
        Services::from_chain(chain)
    }
}

// ---------------------------------------------------------------------------
// WalletServices trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl WalletServices for Services {
    fn chain(&self) -> Chain {
        self.config.chain.clone()
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        // Hand out a clone of the warm tracker, which shares its root cache.
        // Building a fresh one here gave every caller an empty cache, so
        // `internalize_action` — the heaviest user, one root lookup per bump —
        // re-fetched roots this instance had already resolved.
        Ok(Box::new(self.chain_tracker.clone()))
    }

    async fn get_merkle_path(&self, txid: &str, use_next: bool) -> GetMerklePathResult {
        // Snapshot the pre-rotated dispatch order so the collection mutex is never held
        // across a provider `.await`. Walk that order instead of re-reading the cursor:
        // concurrent callers mutate it mid-walk, which could otherwise make one call hit
        // a provider twice and never reach a healthy one. The in-loop `coll.next()` only
        // advances the shared cursor for other callers; it cannot change this owned walk.
        let providers = {
            let mut coll = self.get_merkle_path.lock().await;
            if use_next {
                coll.next();
            }
            coll.call_order()
        };
        let mut r0 = GetMerklePathResult::default();

        for (provider, provider_name) in providers {
            let start = Utc::now();
            let result = provider.get_merkle_path(txid, self).await;
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            if r0.name.is_none() {
                r0.name = result.name.clone();
            }

            if result.merkle_path.is_some() {
                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: true,
                    result: None,
                    error: None,
                };
                self.get_merkle_path.lock().await.add_service_call_success(
                    &provider_name,
                    call,
                    None,
                );
                return result;
            }

            let call = ServiceCall {
                when: start,
                msecs: elapsed,
                success: false,
                result: None,
                error: None,
            };
            {
                let mut coll = self.get_merkle_path.lock().await;
                if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&provider_name, call, &err);
                    if r0.error.is_none() {
                        r0.error = result.error.clone();
                    }
                } else {
                    coll.add_service_call_failure(&provider_name, call);
                }
                coll.next();
            }
        }

        r0
    }

    async fn get_raw_tx(&self, txid: &str, use_next: bool) -> GetRawTxResult {
        // See `get_merkle_path`: snapshot before provider awaits and keep this walk independent of shared cursor updates.
        let providers = {
            let mut coll = self.get_raw_tx.lock().await;
            if use_next {
                coll.next();
            }
            coll.call_order()
        };
        let mut r0 = GetRawTxResult {
            txid: txid.to_string(),
            ..Default::default()
        };

        for (provider, provider_name) in providers {
            let start = Utc::now();
            let result = provider.get_raw_tx(txid).await;
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            if result.raw_tx.is_some() && result.error.is_none() {
                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: true,
                    result: None,
                    error: None,
                };
                self.get_raw_tx
                    .lock()
                    .await
                    .add_service_call_success(&provider_name, call, None);
                return result;
            }

            {
                let mut coll = self.get_raw_tx.lock().await;
                if let Some(ref err_str) = result.error {
                    let call = ServiceCall {
                        when: start,
                        msecs: elapsed,
                        success: false,
                        result: None,
                        error: None,
                    };
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&provider_name, call, &err);
                    if r0.error.is_none() {
                        r0.error = result.error.clone();
                    }
                } else if result.raw_tx.is_none() {
                    // Not found -- still a success for the provider
                    let call = ServiceCall {
                        when: start,
                        msecs: elapsed,
                        success: true,
                        result: Some("not found".to_string()),
                        error: None,
                    };
                    coll.add_service_call_success(
                        &provider_name,
                        call,
                        Some("not found".to_string()),
                    );
                } else {
                    let call = ServiceCall {
                        when: start,
                        msecs: elapsed,
                        success: false,
                        result: None,
                        error: None,
                    };
                    coll.add_service_call_failure(&provider_name, call);
                }
                coll.next();
            }
        }

        r0
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Vec<PostBeefResult> {
        self.post_beef_impl(beef, txids).await
    }

    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
        use_next: bool,
    ) -> GetUtxoStatusResult {
        // See `get_merkle_path`: snapshot before provider awaits and keep this walk independent of shared cursor updates.
        let providers = {
            let mut coll = self.get_utxo_status.lock().await;
            if use_next {
                coll.next();
            }
            coll.call_order()
        };
        let mut r0 = GetUtxoStatusResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            is_utxo: None,
            details: Vec::new(),
        };

        // The two outer attempts and 2000ms delay match TS. The inner walk
        // deliberately diverges: TS re-reads `serviceToCall` live, while this
        // replays one frozen snapshot and does not retake it for the second attempt.
        for _retry in 0..2u32 {
            for (provider, provider_name) in &providers {
                let start = Utc::now();
                let result = provider
                    .get_utxo_status(output, output_format.clone(), outpoint)
                    .await;
                let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

                if result.status == "success" {
                    let call = ServiceCall {
                        when: start,
                        msecs: elapsed,
                        success: true,
                        result: None,
                        error: None,
                    };
                    self.get_utxo_status.lock().await.add_service_call_success(
                        provider_name,
                        call,
                        None,
                    );
                    return result;
                }

                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: false,
                    result: None,
                    error: None,
                };
                {
                    let mut coll = self.get_utxo_status.lock().await;
                    if let Some(ref err_str) = result.error {
                        let err = WalletError::Internal(err_str.clone());
                        coll.add_service_call_error(provider_name, call, &err);
                    } else {
                        coll.add_service_call_failure(provider_name, call);
                    }

                    coll.next();
                }

                r0 = result;
            }

            if r0.status == "success" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2000)).await;
        }

        r0
    }

    async fn get_status_for_txids(
        &self,
        txids: &[String],
        use_next: bool,
    ) -> GetStatusForTxidsResult {
        // See `get_merkle_path`: snapshot before provider awaits and keep this walk independent of shared cursor updates.
        let providers = {
            let mut coll = self.get_status_for_txids.lock().await;
            if use_next {
                coll.next();
            }
            coll.call_order()
        };
        let mut r0 = GetStatusForTxidsResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            results: Vec::new(),
        };

        for (provider, provider_name) in providers {
            let start = Utc::now();
            let result = provider.get_status_for_txids(txids).await;
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            if result.status == "success" {
                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: true,
                    result: None,
                    error: None,
                };
                self.get_status_for_txids
                    .lock()
                    .await
                    .add_service_call_success(&provider_name, call, None);
                return result;
            }

            let call = ServiceCall {
                when: start,
                msecs: elapsed,
                success: false,
                result: None,
                error: None,
            };
            {
                let mut coll = self.get_status_for_txids.lock().await;
                if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&provider_name, call, &err);
                } else {
                    coll.add_service_call_failure(&provider_name, call);
                }

                coll.next();
            }

            r0 = result;
        }

        r0
    }

    async fn get_script_hash_history(
        &self,
        hash: &str,
        use_next: bool,
    ) -> GetScriptHashHistoryResult {
        // See `get_merkle_path`: snapshot before provider awaits and keep this walk independent of shared cursor updates.
        let providers = {
            let mut coll = self.get_script_hash_history.lock().await;
            if use_next {
                coll.next();
            }
            coll.call_order()
        };
        let mut r0 = GetScriptHashHistoryResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            history: Vec::new(),
        };

        for (provider, provider_name) in providers {
            let start = Utc::now();
            let result = provider.get_script_hash_history(hash).await;
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            if result.status == "success" {
                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: true,
                    result: None,
                    error: None,
                };
                self.get_script_hash_history
                    .lock()
                    .await
                    .add_service_call_success(&provider_name, call, None);
                return result;
            }

            let call = ServiceCall {
                when: start,
                msecs: elapsed,
                success: false,
                result: None,
                error: None,
            };
            {
                let mut coll = self.get_script_hash_history.lock().await;
                if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&provider_name, call, &err);
                } else {
                    coll.add_service_call_failure(&provider_name, call);
                }

                coll.next();
            }

            r0 = result;
        }

        r0
    }

    async fn hash_to_header(&self, hash: &str) -> WalletResult<BlockHeader> {
        self.chain_tracker.hash_to_header(hash).await
    }

    async fn get_header_for_height(&self, height: u32) -> WalletResult<Vec<u8>> {
        let header = self.chain_tracker.get_header_for_height(height).await?;
        Ok(serialize_block_header(&header))
    }

    async fn get_height(&self) -> WalletResult<u32> {
        use bsv::transaction::chain_tracker::ChainTracker as _;
        self.chain_tracker
            .current_height()
            .await
            .map_err(|e| WalletError::Internal(format!("ChainTracker error: {e}")))
    }

    async fn n_lock_time_is_final(&self, input: NLockTimeInput) -> WalletResult<bool> {
        const MAXINT: u32 = 0xFFFF_FFFF;
        const BLOCK_LIMIT: u32 = 500_000_000;

        let n_lock_time = match input {
            NLockTimeInput::Raw(locktime) => locktime,
            NLockTimeInput::Transaction(tx) => {
                // If all input sequences are MAXINT, transaction is final regardless
                if tx.inputs.iter().all(|i| i.sequence == MAXINT) {
                    return Ok(true);
                }
                tx.lock_time
            }
        };

        if n_lock_time == 0 {
            return Ok(true);
        }

        if n_lock_time >= BLOCK_LIMIT {
            // Unix timestamp mode: compare to current time
            let now_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;
            return Ok(n_lock_time < now_secs);
        }

        // Block height mode: compare to current chain height
        let height = self.get_height().await?;
        Ok(n_lock_time < height)
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<BsvExchangeRate> {
        self.get_bsv_exchange_rate_impl().await
    }

    async fn get_fiat_exchange_rate(
        &self,
        currency: &str,
        base: Option<&str>,
    ) -> WalletResult<f64> {
        self.get_fiat_exchange_rate_impl(currency, base).await
    }

    async fn get_fiat_exchange_rates(
        &self,
        target_currencies: &[String],
    ) -> WalletResult<FiatExchangeRates> {
        self.get_fiat_exchange_rates_impl(target_currencies).await
    }

    fn get_services_call_history(&self, _reset: bool) -> ServicesCallHistory {
        // This needs to be sync but our mutexes are tokio::sync::Mutex.
        // We use try_lock; if contended, return empty history.
        let mut services = Vec::new();

        if let Ok(mut coll) = self.get_merkle_path.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }
        if let Ok(mut coll) = self.get_raw_tx.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }
        if let Ok(mut coll) = self.post_beef.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }
        if let Ok(mut coll) = self.get_utxo_status.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }
        if let Ok(mut coll) = self.get_status_for_txids.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }
        if let Ok(mut coll) = self.get_script_hash_history.try_lock() {
            services.push(coll.get_service_call_history(_reset));
        }

        ServicesCallHistory { services }
    }

    async fn get_beef_for_txid(&self, txid: &str) -> WalletResult<Beef> {
        // Build a proof-bearing ancestry BEEF from the chain services, mirroring
        // the TypeScript `getBeefForTxid`. A bare single-tx wrapper with no bump
        // and no inputs cannot be SPV-verified by a consumer (e.g. internalizeAction).
        build_beef_for_txid(self, txid).await
    }

    fn hash_output_script(&self, script: &[u8]) -> String {
        // Plain SHA-256 of the script bytes, hex-encoded in natural (unreversed)
        // byte order -- the "hashLE" convention, matching TS `Services.hashOutputScript`
        // (`toHex(sha256(script))`, no reverse). The single reversal into ElectrumX
        // byte order happens exactly once downstream in `validate_script_hash`, which
        // treats a 32-byte input as `hashLE` and reverses it for the WoC query.
        // Reversing here as well would double-reverse and query the wrong scripthash,
        // making `is_utxo` always false for real UTXOs.
        let hash = bsv::primitives::hash::sha256(script);
        let mut hex = String::with_capacity(hash.len() * 2);
        for b in hash.iter() {
            hex.push_str(&format!("{b:02x}"));
        }
        hex
    }

    async fn is_utxo(&self, locking_script: &[u8], txid: &str, vout: u32) -> WalletResult<bool> {
        let hash = self.hash_output_script(locking_script);
        let outpoint = format!("{txid}.{vout}");
        let result = self
            .get_utxo_status(&hash, None, Some(&outpoint), false)
            .await;
        Ok(result.is_utxo == Some(true))
    }
}

// ---------------------------------------------------------------------------
// postBeef orchestration and exchange rate caching
// ---------------------------------------------------------------------------

impl Services {
    /// Post BEEF with UntilSuccess or PromiseAll orchestration.
    async fn post_beef_impl(&self, beef: &[u8], txids: &[String]) -> Vec<PostBeefResult> {
        let soft_timeout_ms = self.config.get_post_beef_soft_timeout_ms(beef.len());

        match self.post_beef_mode {
            PostBeefMode::UntilSuccess => {
                self.post_beef_until_success(beef, txids, soft_timeout_ms)
                    .await
            }
            PostBeefMode::PromiseAll => self.post_beef_promise_all(beef, txids).await,
        }
    }

    /// UntilSuccess mode: try each provider sequentially with adaptive timeout.
    async fn post_beef_until_success(
        &self,
        beef: &[u8],
        txids: &[String],
        soft_timeout_ms: u64,
    ) -> Vec<PostBeefResult> {
        let mut results: Vec<PostBeefResult> = Vec::new();

        // See `get_merkle_path`: snapshot before provider awaits and keep this walk independent of shared cursor updates.
        let providers = {
            let coll = self.post_beef.lock().await;
            coll.call_order()
        };

        for (provider, provider_name) in providers {
            let start = Utc::now();
            let result = if soft_timeout_ms > 0 {
                match tokio::time::timeout(
                    Duration::from_millis(soft_timeout_ms),
                    provider.post_beef(beef, txids),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => PostBeefResult::timeout(&provider_name, txids, soft_timeout_ms),
                }
            } else {
                provider.post_beef(beef, txids).await
            };
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            let is_success = result.status == "success";
            let is_timeout = result
                .error
                .as_ref()
                .map(|e| e.contains("timeout"))
                .unwrap_or(false);

            let all_service_error = result
                .txid_results
                .iter()
                .all(|tx_result| tx_result.service_error == Some(true));

            {
                let mut coll = self.post_beef.lock().await;
                let call = ServiceCall {
                    when: start,
                    msecs: elapsed,
                    success: is_success,
                    result: None,
                    error: None,
                };
                if is_success {
                    coll.add_service_call_success(&provider_name, call, None);
                } else if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&provider_name, call, &err);
                } else {
                    coll.add_service_call_failure(&provider_name, call);
                }

                if !is_success {
                    if !is_timeout && all_service_error {
                        coll.move_service_to_last(&provider_name);
                    }
                    coll.next();
                }
            }

            results.push(result);

            if is_success {
                break;
            }
        }

        if results.is_empty() {
            vec![PostBeefResult {
                name: "<noservices>".to_string(),
                status: "error".to_string(),
                error: Some("No postBeef services available".to_string()),
                txid_results: Vec::new(),
            }]
        } else {
            results
        }
    }

    /// PromiseAll mode: call all providers concurrently and collect results.
    async fn post_beef_promise_all(&self, beef: &[u8], txids: &[String]) -> Vec<PostBeefResult> {
        let providers = {
            let coll = self.post_beef.lock().await;
            coll.call_order()
        };

        if providers.is_empty() {
            return vec![PostBeefResult {
                name: "<noservices>".to_string(),
                status: "error".to_string(),
                error: Some("No postBeef services available".to_string()),
                txid_results: Vec::new(),
            }];
        }

        let results =
            futures::future::join_all(providers.into_iter().map(|(provider, name)| async move {
                let start = Utc::now();
                let result = provider.post_beef(beef, txids).await;
                let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();
                (name, start, elapsed, result)
            }))
            .await;

        {
            let mut coll = self.post_beef.lock().await;
            for (name, start, elapsed, ref result) in &results {
                let call = ServiceCall {
                    when: *start,
                    msecs: *elapsed,
                    success: result.status == "success",
                    result: None,
                    error: None,
                };
                if result.status == "success" {
                    coll.add_service_call_success(name, call, None);
                } else if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(name, call, &err);
                } else {
                    coll.add_service_call_failure(name, call);
                }
            }
        }

        results
            .into_iter()
            .map(|(_, _, _, result)| result)
            .collect()
    }

    /// BSV exchange rate with caching.
    ///
    /// Returns cached rate if within `bsv_update_msecs`, otherwise fetches fresh.
    async fn get_bsv_exchange_rate_impl(&self) -> WalletResult<BsvExchangeRate> {
        let update_ms = self.config.bsv_update_msecs;

        {
            let cached = self.bsv_exchange_rate.lock().await;
            let age_ms = Utc::now()
                .signed_duration_since(cached.timestamp)
                .num_milliseconds() as u64;
            if cached.rate_usd > 0.0 && age_ms < update_ms {
                return Ok(cached.clone());
            }
        }

        // Fetch fresh rate
        let rate = fetch_bsv_exchange_rate(&self.client).await?;
        let new_rate = BsvExchangeRate {
            timestamp: Utc::now(),
            rate_usd: rate,
        };

        let mut cached = self.bsv_exchange_rate.lock().await;
        *cached = new_rate.clone();
        Ok(new_rate)
    }

    /// Single fiat exchange rate with caching.
    async fn get_fiat_exchange_rate_impl(
        &self,
        currency: &str,
        base: Option<&str>,
    ) -> WalletResult<f64> {
        let base = base.unwrap_or("USD");
        if currency == base {
            return Ok(1.0);
        }

        // Determine which currencies we need
        let required: Vec<String> = if base == "USD" {
            vec![currency.to_string()]
        } else {
            vec![currency.to_string(), base.to_string()]
        };

        // Update fiat rates (will use cache if fresh)
        self.update_fiat_exchange_rates(&required).await?;

        let cached = self.fiat_exchange_rates.lock().await;
        let c = cached
            .rates
            .get(currency)
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "currency".to_string(),
                must_be: format!("valid fiat currency '{currency}' with an exchange rate"),
            })?;
        let b = cached
            .rates
            .get(base)
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "base".to_string(),
                must_be: format!("valid fiat currency '{base}' with an exchange rate"),
            })?;

        Ok(c / b)
    }

    /// Multiple fiat exchange rates with caching.
    async fn get_fiat_exchange_rates_impl(
        &self,
        target_currencies: &[String],
    ) -> WalletResult<FiatExchangeRates> {
        self.update_fiat_exchange_rates(target_currencies).await?;

        let cached = self.fiat_exchange_rates.lock().await;
        let mut rates = std::collections::HashMap::new();
        for c in target_currencies {
            if let Some(v) = cached.rates.get(c.as_str()) {
                rates.insert(c.clone(), *v);
            }
        }

        Ok(FiatExchangeRates {
            timestamp: cached.timestamp,
            base: "USD".to_string(),
            rates,
        })
    }

    /// Internal: update fiat exchange rates cache if stale for any requested currencies.
    async fn update_fiat_exchange_rates(&self, target_currencies: &[String]) -> WalletResult<()> {
        let update_ms = self.config.fiat_update_msecs;
        let _freshness_cutoff = Utc::now() - chrono::Duration::milliseconds(update_ms as i64);

        let to_fetch: Vec<String> = {
            let cached = self.fiat_exchange_rates.lock().await;
            target_currencies
                .iter()
                .filter(|c| {
                    if c.as_str() == "USD" {
                        return false; // USD is always 1.0
                    }
                    !cached.rates.contains_key(c.as_str())
                })
                .cloned()
                .collect()
        };

        if to_fetch.is_empty() {
            // Ensure USD is always present
            let mut cached = self.fiat_exchange_rates.lock().await;
            cached.rates.entry("USD".to_string()).or_insert(1.0);
            return Ok(());
        }

        // Fetch from provider
        let fetched = fetch_fiat_exchange_rates(
            &self.client,
            self.config.exchangeratesapi_key.as_deref(),
            "USD",
            &to_fetch,
        )
        .await?;

        // Merge into cache
        let mut cached = self.fiat_exchange_rates.lock().await;
        for (currency, rate) in &fetched.rates {
            cached.rates.insert(currency.clone(), *rate);
        }
        cached.rates.entry("USD".to_string()).or_insert(1.0);
        if fetched.timestamp > cached.timestamp {
            cached.timestamp = fetched.timestamp;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize a BlockHeader to 80 bytes in standard Bitcoin block header format.
///
/// Format: version(4) + prevHash(32) + merkleRoot(32) + time(4) + bits(4) + nonce(4) = 80 bytes.
/// Hash fields are written in reversed byte order (internal byte order).
fn serialize_block_header(header: &BlockHeader) -> Vec<u8> {
    let mut buf = Vec::with_capacity(80);

    // version (4 bytes LE)
    buf.extend_from_slice(&header.version.to_le_bytes());

    // previous hash (32 bytes, reversed from display hex)
    if let Ok(bytes) = hex_to_bytes_reversed(&header.previous_hash) {
        buf.extend_from_slice(&bytes);
    } else {
        buf.extend_from_slice(&[0u8; 32]);
    }

    // merkle root (32 bytes, reversed from display hex)
    if let Ok(bytes) = hex_to_bytes_reversed(&header.merkle_root) {
        buf.extend_from_slice(&bytes);
    } else {
        buf.extend_from_slice(&[0u8; 32]);
    }

    // time (4 bytes LE)
    buf.extend_from_slice(&header.time.to_le_bytes());

    // bits (4 bytes LE)
    buf.extend_from_slice(&header.bits.to_le_bytes());

    // nonce (4 bytes LE)
    buf.extend_from_slice(&header.nonce.to_le_bytes());

    buf
}

/// Decode a hex string into bytes with reversed byte order.
/// Used for block header hash fields which are displayed in reverse byte order.
fn hex_to_bytes_reversed(hex: &str) -> Result<Vec<u8>, WalletError> {
    if !hex.len().is_multiple_of(2) {
        return Err(WalletError::InvalidParameter {
            parameter: "hex".to_string(),
            must_be: "an even-length hex string".to_string(),
        });
    }
    let mut bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| WalletError::InvalidParameter {
                parameter: "hex".to_string(),
                must_be: "valid hex characters".to_string(),
            })
        })
        .collect::<Result<Vec<u8>, _>>()?;
    bytes.reverse();
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Proof-bearing BEEF construction for a txid via chain services (BRC-62 / BRC-74)
// ---------------------------------------------------------------------------

/// Chain-service data source used to build a proof-bearing BEEF for a txid.
///
/// Abstracts the two lookups the builder needs -- the raw transaction bytes and,
/// for a mined transaction, its BUMP merkle path. Implemented for [`Services`]
/// against the live provider collections; the seam keeps the recursive builder
/// unit-testable without real HTTP.
#[async_trait]
trait BeefTxSource {
    /// BUMP merkle path bytes for `txid`, or `None` when unmined/unknown.
    async fn source_merkle_path(&self, txid: &str) -> WalletResult<Option<Vec<u8>>>;
    /// Raw transaction bytes for `txid`, or `None` when unknown.
    async fn source_raw_tx(&self, txid: &str) -> WalletResult<Option<Vec<u8>>>;
}

#[async_trait]
impl BeefTxSource for Services {
    async fn source_merkle_path(&self, txid: &str) -> WalletResult<Option<Vec<u8>>> {
        Ok(self.get_merkle_path(txid, false).await.merkle_path)
    }

    async fn source_raw_tx(&self, txid: &str) -> WalletResult<Option<Vec<u8>>> {
        Ok(self.get_raw_tx(txid, false).await.raw_tx)
    }
}

/// Build a valid, proof-bearing BEEF for `txid` from chain-service lookups.
///
/// Mirrors the TypeScript `getBeefForTxid`: walk the input ancestry, stopping each
/// branch at the first mined ancestor whose BUMP merkle path proves it. Every
/// included transaction carries either its own merkle proof or the raw transactions
/// of its unproven inputs, so the result verifies under SPV -- unlike a bare
/// single-transaction wrapper with `bump_index: None` and no inputs.
async fn build_beef_for_txid<S>(source: &S, txid: &str) -> WalletResult<Beef>
where
    S: BeefTxSource + Sync + ?Sized,
{
    let mut beef = Beef::new(BEEF_V2);
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    merge_txid_into_beef(source, txid, &mut beef, &mut visited).await?;
    if beef.txs.is_empty() {
        return Err(WalletError::Internal(format!(
            "Could not retrieve raw transaction for txid {txid}"
        )));
    }
    Ok(beef)
}

/// Recursively merge `txid` (and, if unproven, its input ancestry) into `beef`.
async fn merge_txid_into_beef<S>(
    source: &S,
    txid: &str,
    beef: &mut Beef,
    visited: &mut std::collections::HashSet<String>,
) -> WalletResult<()>
where
    S: BeefTxSource + Sync + ?Sized,
{
    if !visited.insert(txid.to_string()) {
        return Ok(());
    }

    let raw_tx = source.source_raw_tx(txid).await?.ok_or_else(|| {
        WalletError::Internal(format!(
            "Could not retrieve raw transaction for txid {txid}"
        ))
    })?;
    let tx = BsvTransaction::from_binary(&mut std::io::Cursor::new(&raw_tx))
        .map_err(|e| WalletError::Internal(format!("Failed to parse transaction {txid}: {e}")))?;

    // A mined transaction is self-proving: attach its BUMP and stop the branch.
    if let Some(mp_bytes) = source.source_merkle_path(txid).await? {
        let merkle_path =
            MerklePath::from_binary(&mut std::io::Cursor::new(&mp_bytes)).map_err(|e| {
                WalletError::Internal(format!("Failed to parse merkle path for {txid}: {e}"))
            })?;
        let bump_index = beef.bumps.len();
        beef.bumps.push(merkle_path);
        let beef_tx = BeefTx::from_tx(tx, Some(bump_index)).map_err(|e| {
            WalletError::Internal(format!("Failed to build BeefTx for {txid}: {e}"))
        })?;
        beef.txs.push(beef_tx);
        return Ok(());
    }

    // Unproven: include the raw transaction and recurse over its input ancestry.
    let input_txids: Vec<String> = tx
        .inputs
        .iter()
        .filter_map(|input| input.source_txid.clone())
        .collect();
    let beef_tx = BeefTx::from_tx(tx, None)
        .map_err(|e| WalletError::Internal(format!("Failed to build BeefTx for {txid}: {e}")))?;
    beef.txs.push(beef_tx);

    for input_txid in input_txids {
        if !visited.contains(&input_txid) {
            Box::pin(merge_txid_into_beef(source, &input_txid, beef, visited)).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod beef_builder_tests {
    use super::*;
    use bsv::transaction::merkle_path::{MerklePath, MerklePathLeaf};
    use bsv::transaction::transaction_input::TransactionInput;
    use std::collections::HashMap;
    use std::future::Future;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time;

    const CONCURRENT_CALLS: usize = 4;
    type VisitLog = Arc<Mutex<HashMap<String, Vec<String>>>>;

    async fn record_visit(visits: &VisitLog, walk_id: &str, provider_name: &str) {
        visits
            .lock()
            .await
            .entry(walk_id.to_string())
            .or_default()
            .push(provider_name.to_string());
    }

    async fn assert_walk_visits(visits: &VisitLog, walk_ids: &[&str]) {
        let visits = visits.lock().await;
        for walk_id in walk_ids {
            let providers: Vec<&str> = visits
                .get(*walk_id)
                .unwrap_or_else(|| panic!("missing visits for {walk_id}"))
                .iter()
                .map(String::as_str)
                .collect();
            assert_eq!(
                providers,
                vec!["AlwaysFails", "AlwaysSucceeds"],
                "unexpected provider walk for {walk_id}"
            );
        }
    }

    #[derive(Default)]
    struct InFlightTracker {
        current: AtomicUsize,
        high_water: AtomicUsize,
    }

    impl InFlightTracker {
        async fn track_call(&self) {
            let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.high_water.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(200)).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct ConcurrentProvider {
        tracker: Arc<InFlightTracker>,
    }

    #[async_trait]
    impl GetMerklePathProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn get_merkle_path(
            &self,
            _txid: &str,
            _services: &dyn WalletServices,
        ) -> GetMerklePathResult {
            self.tracker.track_call().await;
            GetMerklePathResult {
                name: Some("ConcurrentMock".to_string()),
                merkle_path: Some(vec![1]),
                header: None,
                error: None,
            }
        }
    }

    #[async_trait]
    impl GetRawTxProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn get_raw_tx(&self, txid: &str) -> GetRawTxResult {
            self.tracker.track_call().await;
            GetRawTxResult {
                txid: txid.to_string(),
                name: Some("ConcurrentMock".to_string()),
                raw_tx: Some(vec![1]),
                error: None,
            }
        }
    }

    #[async_trait]
    impl GetUtxoStatusProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
        ) -> GetUtxoStatusResult {
            self.tracker.track_call().await;
            GetUtxoStatusResult {
                name: "ConcurrentMock".to_string(),
                status: "success".to_string(),
                error: None,
                is_utxo: Some(true),
                details: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl GetStatusForTxidsProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn get_status_for_txids(&self, _txids: &[String]) -> GetStatusForTxidsResult {
            self.tracker.track_call().await;
            GetStatusForTxidsResult {
                name: "ConcurrentMock".to_string(),
                status: "success".to_string(),
                error: None,
                results: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl GetScriptHashHistoryProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn get_script_hash_history(&self, _hash: &str) -> GetScriptHashHistoryResult {
            self.tracker.track_call().await;
            GetScriptHashHistoryResult {
                name: "ConcurrentMock".to_string(),
                status: "success".to_string(),
                error: None,
                history: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl PostBeefProvider for ConcurrentProvider {
        fn name(&self) -> &str {
            "ConcurrentMock"
        }

        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> PostBeefResult {
            self.tracker.track_call().await;
            PostBeefResult {
                name: "ConcurrentMock".to_string(),
                status: "success".to_string(),
                error: None,
                txid_results: Vec::new(),
            }
        }
    }

    async fn assert_overlapping_calls<F, Fut, T>(tracker: &InFlightTracker, call: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = T>,
    {
        time::timeout(
            Duration::from_secs(5),
            futures::future::join_all((0..CONCURRENT_CALLS).map(|_| call())),
        )
        .await
        .expect("concurrent service calls wedged on a provider collection lock");
        assert_eq!(
            tracker.high_water.load(Ordering::SeqCst),
            CONCURRENT_CALLS,
            "not all provider calls overlapped"
        );
    }

    // Returns (success, failure, error). `add_service_call_error` increments both
    // failure and error, so a provider that errors N times has counts (0, N, N).
    fn provider_history_counts<T: ?Sized>(
        collection: &mut ServiceCollection<T>,
        provider_name: &str,
    ) -> (u32, u32, u32) {
        let history = collection.get_service_call_history(false);
        let counts = &history
            .history_by_provider
            .get(provider_name)
            .expect("provider history")
            .total_counts;
        (counts.success, counts.failure, counts.error)
    }

    // Paused virtual time auto-advances only when all tasks are idle. The five-second
    // timeout is therefore a deadlock detector, not a wall-clock guard, and the
    // overlap assertions remain valid.
    #[tokio::test(start_paused = true)]
    async fn concurrent_service_calls_do_not_hold_collection_locks_across_awaits() {
        let services = Services::from_chain(Chain::Test);

        let merkle_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.get_merkle_path.lock().await;
            *providers = ServiceCollection::new("getMerklePath");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&merkle_tracker),
                }),
            );
        }
        assert_overlapping_calls(&merkle_tracker, || {
            services.get_merkle_path("test-txid", false)
        })
        .await;
        {
            let mut providers = services.get_merkle_path.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }

        let raw_tx_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.get_raw_tx.lock().await;
            *providers = ServiceCollection::new("getRawTx");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&raw_tx_tracker),
                }),
            );
        }
        assert_overlapping_calls(&raw_tx_tracker, || services.get_raw_tx("test-txid", false)).await;
        {
            let mut providers = services.get_raw_tx.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }

        let utxo_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.get_utxo_status.lock().await;
            *providers = ServiceCollection::new("getUtxoStatus");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&utxo_tracker),
                }),
            );
        }
        assert_overlapping_calls(&utxo_tracker, || {
            services.get_utxo_status("output", None, Some("txid.0"), false)
        })
        .await;
        {
            let mut providers = services.get_utxo_status.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }

        let status_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.get_status_for_txids.lock().await;
            *providers = ServiceCollection::new("getStatusForTxids");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&status_tracker),
                }),
            );
        }
        let txids = vec!["test-txid".to_string()];
        assert_overlapping_calls(&status_tracker, || {
            services.get_status_for_txids(&txids, false)
        })
        .await;
        {
            let mut providers = services.get_status_for_txids.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }

        let script_history_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.get_script_hash_history.lock().await;
            *providers = ServiceCollection::new("getScriptHashHistory");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&script_history_tracker),
                }),
            );
        }
        assert_overlapping_calls(&script_history_tracker, || {
            services.get_script_hash_history("script-hash", false)
        })
        .await;
        {
            let mut providers = services.get_script_hash_history.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }

        let post_beef_tracker = Arc::new(InFlightTracker::default());
        {
            let mut providers = services.post_beef.lock().await;
            *providers = ServiceCollection::new("postBeef");
            providers.add(
                "ConcurrentMock",
                Arc::new(ConcurrentProvider {
                    tracker: Arc::clone(&post_beef_tracker),
                }),
            );
        }
        assert_overlapping_calls(&post_beef_tracker, || services.post_beef(&[1], &txids)).await;
        {
            let mut providers = services.post_beef.lock().await;
            assert_eq!(
                provider_history_counts(&mut providers, "ConcurrentMock"),
                (4, 0, 0)
            );
        }
    }

    struct PromiseAllPostBeefProvider {
        provider_name: &'static str,
        tracker: Arc<InFlightTracker>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PostBeefProvider for PromiseAllPostBeefProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> PostBeefResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.tracker.track_call().await;
            PostBeefResult {
                name: self.name().to_string(),
                status: "success".to_string(),
                error: None,
                txid_results: Vec::new(),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn post_beef_promise_all_calls_every_provider_concurrently() {
        const PROVIDER_NAMES: [&str; 3] = ["PromiseAll-0", "PromiseAll-1", "PromiseAll-2"];

        let mut services = Services::from_chain(Chain::Test);
        services.set_post_beef_mode(PostBeefMode::PromiseAll);
        let tracker = Arc::new(InFlightTracker::default());
        let calls: Vec<Arc<AtomicUsize>> = PROVIDER_NAMES
            .iter()
            .map(|_| Arc::new(AtomicUsize::new(0)))
            .collect();
        {
            let mut providers = services.post_beef.lock().await;
            *providers = ServiceCollection::new("postBeef");
            for (provider_name, provider_calls) in PROVIDER_NAMES.iter().zip(&calls) {
                providers.add(
                    provider_name,
                    Arc::new(PromiseAllPostBeefProvider {
                        provider_name,
                        tracker: Arc::clone(&tracker),
                        calls: Arc::clone(provider_calls),
                    }),
                );
            }
        }
        let services = Arc::new(services);
        let txids = vec!["test-txid".to_string()];

        let results = time::timeout(Duration::from_secs(5), services.post_beef(&[1], &txids))
            .await
            .expect("PromiseAll post_beef providers wedged");

        assert_eq!(
            tracker.high_water.load(Ordering::SeqCst),
            PROVIDER_NAMES.len()
        );
        assert!(calls
            .iter()
            .all(|provider_calls| provider_calls.load(Ordering::SeqCst) == 1));
        assert_eq!(results.len(), PROVIDER_NAMES.len());
        assert_eq!(
            results
                .iter()
                .map(|result| result.name.as_str())
                .collect::<Vec<_>>(),
            PROVIDER_NAMES
        );
        assert!(results.iter().all(|result| result.status == "success"));

        let mut providers = services.post_beef.lock().await;
        for provider_name in PROVIDER_NAMES {
            assert_eq!(
                provider_history_counts(&mut providers, provider_name),
                (1, 0, 0)
            );
        }
    }

    struct FailingMerklePathProvider {
        calls: Arc<AtomicUsize>,
        visits: VisitLog,
    }

    #[async_trait]
    impl GetMerklePathProvider for FailingMerklePathProvider {
        fn name(&self) -> &str {
            "AlwaysFails"
        }

        async fn get_merkle_path(
            &self,
            txid: &str,
            _services: &dyn WalletServices,
        ) -> GetMerklePathResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            record_visit(&self.visits, txid, self.name()).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            GetMerklePathResult {
                name: Some(self.name().to_string()),
                ..Default::default()
            }
        }
    }

    struct SuccessfulMerklePathProvider {
        calls: Arc<AtomicUsize>,
        visits: VisitLog,
    }

    #[async_trait]
    impl GetMerklePathProvider for SuccessfulMerklePathProvider {
        fn name(&self) -> &str {
            "AlwaysSucceeds"
        }

        async fn get_merkle_path(
            &self,
            txid: &str,
            _services: &dyn WalletServices,
        ) -> GetMerklePathResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            record_visit(&self.visits, txid, self.name()).await;
            GetMerklePathResult {
                name: Some(self.name().to_string()),
                merkle_path: Some(vec![1]),
                header: None,
                error: None,
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_failover_visits_every_merkle_path_provider_once() {
        let services = Services::from_chain(Chain::Test);
        let failing_calls = Arc::new(AtomicUsize::new(0));
        let successful_calls = Arc::new(AtomicUsize::new(0));
        let visits = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut providers = services.get_merkle_path.lock().await;
            *providers = ServiceCollection::new("getMerklePath");
            providers.add(
                "AlwaysFails",
                Arc::new(FailingMerklePathProvider {
                    calls: Arc::clone(&failing_calls),
                    visits: Arc::clone(&visits),
                }),
            );
            providers.add(
                "AlwaysSucceeds",
                Arc::new(SuccessfulMerklePathProvider {
                    calls: Arc::clone(&successful_calls),
                    visits: Arc::clone(&visits),
                }),
            );
        }

        let txids = ["txid-0", "txid-1"];
        let results = time::timeout(
            Duration::from_secs(5),
            futures::future::join_all(
                txids
                    .iter()
                    .map(|txid| services.get_merkle_path(txid, false)),
            ),
        )
        .await
        .expect("concurrent merkle-path failover wedged");

        assert!(results.iter().all(|result| result.merkle_path.is_some()));
        assert_walk_visits(&visits, &txids).await;
        assert_eq!(failing_calls.load(Ordering::SeqCst), 2);
        assert_eq!(successful_calls.load(Ordering::SeqCst), 2);

        let mut providers = services.get_merkle_path.lock().await;
        assert_eq!(
            provider_history_counts(&mut providers, "AlwaysFails"),
            (0, 2, 0)
        );
        assert_eq!(
            provider_history_counts(&mut providers, "AlwaysSucceeds"),
            (2, 0, 0)
        );
    }

    struct FailoverProvider {
        provider_name: &'static str,
        succeeds: bool,
        visits: VisitLog,
    }

    impl FailoverProvider {
        async fn visit(&self, walk_id: &str) {
            record_visit(&self.visits, walk_id, self.provider_name).await;
            if !self.succeeds {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        fn status(&self) -> String {
            if self.succeeds {
                "success".to_string()
            } else {
                "error".to_string()
            }
        }

        fn error(&self) -> Option<String> {
            (!self.succeeds).then(|| "mock provider failure".to_string())
        }
    }

    #[async_trait]
    impl GetRawTxProvider for FailoverProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn get_raw_tx(&self, txid: &str) -> GetRawTxResult {
            self.visit(txid).await;
            GetRawTxResult {
                txid: txid.to_string(),
                name: Some(self.provider_name.to_string()),
                raw_tx: self.succeeds.then(|| vec![1]),
                error: self.error(),
            }
        }
    }

    #[async_trait]
    impl GetUtxoStatusProvider for FailoverProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn get_utxo_status(
            &self,
            output: &str,
            _output_format: Option<GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
        ) -> GetUtxoStatusResult {
            self.visit(output).await;
            GetUtxoStatusResult {
                name: self.provider_name.to_string(),
                status: self.status(),
                error: self.error(),
                is_utxo: self.succeeds.then_some(true),
                details: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl GetStatusForTxidsProvider for FailoverProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn get_status_for_txids(&self, txids: &[String]) -> GetStatusForTxidsResult {
            let walk_id = txids.first().map(String::as_str).unwrap_or("<missing>");
            self.visit(walk_id).await;
            GetStatusForTxidsResult {
                name: self.provider_name.to_string(),
                status: self.status(),
                error: self.error(),
                results: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl GetScriptHashHistoryProvider for FailoverProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn get_script_hash_history(&self, hash: &str) -> GetScriptHashHistoryResult {
            self.visit(hash).await;
            GetScriptHashHistoryResult {
                name: self.provider_name.to_string(),
                status: self.status(),
                error: self.error(),
                history: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl PostBeefProvider for FailoverProvider {
        fn name(&self) -> &str {
            self.provider_name
        }

        async fn post_beef(&self, _beef: &[u8], txids: &[String]) -> PostBeefResult {
            let walk_id = txids.first().map(String::as_str).unwrap_or("<missing>");
            self.visit(walk_id).await;
            if self.succeeds {
                PostBeefResult {
                    name: self.provider_name.to_string(),
                    status: "success".to_string(),
                    error: None,
                    txid_results: Vec::new(),
                }
            } else {
                PostBeefResult::timeout(self.provider_name, txids, 100)
            }
        }
    }

    fn failover_provider_pair(visits: &VisitLog) -> (Arc<FailoverProvider>, Arc<FailoverProvider>) {
        (
            Arc::new(FailoverProvider {
                provider_name: "AlwaysFails",
                succeeds: false,
                visits: Arc::clone(visits),
            }),
            Arc::new(FailoverProvider {
                provider_name: "AlwaysSucceeds",
                succeeds: true,
                visits: Arc::clone(visits),
            }),
        )
    }

    fn add_failover_providers<T: ?Sized>(
        providers: &mut ServiceCollection<T>,
        failing: Arc<T>,
        successful: Arc<T>,
    ) {
        providers.add("AlwaysFails", failing);
        providers.add("AlwaysSucceeds", successful);
    }

    async fn assert_concurrent_failover<F, Fut>(
        service_name: &str,
        visits: &VisitLog,
        walk_ids: &[&str],
        call: F,
    ) where
        F: Fn(String) -> Fut,
        Fut: Future<Output = bool>,
    {
        let results = time::timeout(
            Duration::from_secs(5),
            futures::future::join_all(walk_ids.iter().map(|walk_id| call((*walk_id).to_string()))),
        )
        .await
        .unwrap_or_else(|_| panic!("concurrent {service_name} failover wedged"));
        assert!(
            results.into_iter().all(|succeeded| succeeded),
            "{service_name} did not fail over successfully"
        );
        assert_walk_visits(visits, walk_ids).await;
    }

    fn assert_failover_history<T: ?Sized>(providers: &mut ServiceCollection<T>) {
        assert_eq!(provider_history_counts(providers, "AlwaysFails"), (0, 2, 2));
        assert_eq!(
            provider_history_counts(providers, "AlwaysSucceeds"),
            (2, 0, 0)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_failover_visits_every_provider_once_for_all_other_service_methods() {
        let services = Services::from_chain(Chain::Test);
        let visits = Arc::new(Mutex::new(HashMap::new()));
        let (failing, successful) = failover_provider_pair(&visits);
        let services = &services;

        {
            let mut providers = services.get_raw_tx.lock().await;
            *providers = ServiceCollection::new("getRawTx");
            add_failover_providers(&mut providers, failing.clone(), successful.clone());
        }
        let walk_ids = ["raw-txid-0", "raw-txid-1"];
        assert_concurrent_failover("raw-tx", &visits, &walk_ids, |walk_id| async move {
            services.get_raw_tx(&walk_id, false).await.raw_tx.is_some()
        })
        .await;
        {
            let mut providers = services.get_raw_tx.lock().await;
            assert_failover_history(&mut providers);
        }

        {
            let mut providers = services.get_utxo_status.lock().await;
            *providers = ServiceCollection::new("getUtxoStatus");
            add_failover_providers(&mut providers, failing.clone(), successful.clone());
        }
        let walk_ids = ["utxo-output-0", "utxo-output-1"];
        assert_concurrent_failover("UTXO-status", &visits, &walk_ids, |walk_id| async move {
            services
                .get_utxo_status(&walk_id, None, None, false)
                .await
                .is_utxo
                == Some(true)
        })
        .await;
        {
            let mut providers = services.get_utxo_status.lock().await;
            assert_failover_history(&mut providers);
        }

        {
            let mut providers = services.get_status_for_txids.lock().await;
            *providers = ServiceCollection::new("getStatusForTxids");
            add_failover_providers(&mut providers, failing.clone(), successful.clone());
        }
        let walk_ids = ["status-txid-0", "status-txid-1"];
        assert_concurrent_failover("txid-status", &visits, &walk_ids, |walk_id| async move {
            services
                .get_status_for_txids(&[walk_id], false)
                .await
                .status
                == "success"
        })
        .await;
        {
            let mut providers = services.get_status_for_txids.lock().await;
            assert_failover_history(&mut providers);
        }

        {
            let mut providers = services.get_script_hash_history.lock().await;
            *providers = ServiceCollection::new("getScriptHashHistory");
            add_failover_providers(&mut providers, failing.clone(), successful.clone());
        }
        let walk_ids = ["script-hash-0", "script-hash-1"];
        assert_concurrent_failover(
            "script-hash-history",
            &visits,
            &walk_ids,
            |walk_id| async move {
                services
                    .get_script_hash_history(&walk_id, false)
                    .await
                    .status
                    == "success"
            },
        )
        .await;
        {
            let mut providers = services.get_script_hash_history.lock().await;
            assert_failover_history(&mut providers);
        }

        {
            let mut providers = services.post_beef.lock().await;
            *providers = ServiceCollection::new("postBeef");
            add_failover_providers(&mut providers, failing, successful);
        }
        let walk_ids = ["post-txid-0", "post-txid-1"];
        assert_concurrent_failover("post-beef", &visits, &walk_ids, |walk_id| async move {
            services
                .post_beef(&[1], &[walk_id])
                .await
                .last()
                .is_some_and(|result| result.status == "success")
        })
        .await;
        {
            let mut providers = services.post_beef.lock().await;
            assert_failover_history(&mut providers);
        }
    }

    struct ServiceErrorPostBeefProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PostBeefProvider for ServiceErrorPostBeefProvider {
        fn name(&self) -> &str {
            "PostFails"
        }

        async fn post_beef(&self, _beef: &[u8], txids: &[String]) -> PostBeefResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut result = PostBeefResult::timeout(self.name(), txids, 100);
            result.error = Some("mock service error".to_string());
            result
        }
    }

    struct SuccessfulPostBeefProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PostBeefProvider for SuccessfulPostBeefProvider {
        fn name(&self) -> &str {
            "PostSucceeds"
        }

        async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> PostBeefResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            PostBeefResult {
                name: self.name().to_string(),
                status: "success".to_string(),
                error: None,
                txid_results: Vec::new(),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn post_beef_failover_uses_snapshot_when_failure_reorders_collection() {
        let services = Services::from_chain(Chain::Test);
        let failing_calls = Arc::new(AtomicUsize::new(0));
        let successful_calls = Arc::new(AtomicUsize::new(0));
        {
            let mut providers = services.post_beef.lock().await;
            *providers = ServiceCollection::new("postBeef");
            providers.add(
                "PostFails",
                Arc::new(ServiceErrorPostBeefProvider {
                    calls: Arc::clone(&failing_calls),
                }),
            );
            providers.add(
                "PostSucceeds",
                Arc::new(SuccessfulPostBeefProvider {
                    calls: Arc::clone(&successful_calls),
                }),
            );
        }

        let txids = vec!["test-txid".to_string()];
        let results = services.post_beef_until_success(&[1], &txids, 0).await;

        assert_eq!(results.len(), 2);
        assert_eq!(
            results.last().map(|result| result.status.as_str()),
            Some("success")
        );
        assert_eq!(failing_calls.load(Ordering::SeqCst), 1);
        assert_eq!(successful_calls.load(Ordering::SeqCst), 1);

        let mut providers = services.post_beef.lock().await;
        assert_eq!(
            provider_history_counts(&mut providers, "PostFails"),
            (0, 1, 1)
        );
        assert_eq!(
            provider_history_counts(&mut providers, "PostSucceeds"),
            (1, 0, 0)
        );
        let provider_order: Vec<String> = providers
            .call_order()
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect();
        // Pins CURRENTLY-DEFECTIVE behavior: after `move_service_to_last`, the cursor
        // still points at the just-de-prioritized provider, so `call_order()[0]` is
        // "PostFails" instead of the new front "PostSucceeds". Fixing that defect
        // should flip this expectation; change the code, not this assertion.
        assert_eq!(provider_order, vec!["PostFails", "PostSucceeds"]);
    }

    #[tokio::test]
    async fn arcade_sse_defaults_install_primary_broadcaster_with_same_token() {
        let services = Services::from_chain_with_arcade(
            Chain::Test,
            Some("https://arcade.example".to_string()),
            Some("wallet-token".to_string()),
        );

        assert_eq!(
            services.config.arcade_url.as_deref(),
            Some("https://arcade.example")
        );
        assert_eq!(
            services
                .config
                .arcade_config
                .as_ref()
                .and_then(|config| config.callback_token.as_deref()),
            Some("wallet-token")
        );

        let providers: Vec<String> = services
            .post_beef
            .lock()
            .await
            .call_order()
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect();
        assert_eq!(providers.first().map(String::as_str), Some("ArcadeBeef"));
    }

    #[derive(Default)]
    struct MockTxSource {
        raw: HashMap<String, Vec<u8>>,
        merkle: HashMap<String, Vec<u8>>,
    }

    #[async_trait]
    impl BeefTxSource for MockTxSource {
        async fn source_merkle_path(&self, txid: &str) -> WalletResult<Option<Vec<u8>>> {
            Ok(self.merkle.get(txid).cloned())
        }
        async fn source_raw_tx(&self, txid: &str) -> WalletResult<Option<Vec<u8>>> {
            Ok(self.raw.get(txid).cloned())
        }
    }

    /// A minimal valid single-tx-block BUMP proving `txid_hash`, serialized to bytes.
    fn bump_bytes_for(txid_hash: &str) -> Vec<u8> {
        let level0 = vec![MerklePathLeaf {
            offset: 0,
            hash: Some(txid_hash.to_string()),
            txid: true,
            duplicate: false,
        }];
        let mp = MerklePath::new(800_000, vec![level0]).expect("valid merkle path");
        let mut buf = Vec::new();
        mp.to_binary(&mut buf).expect("serialize merkle path");
        buf
    }

    /// A minimal transaction with no inputs -- returns (raw_bytes, txid).
    fn leaf_tx() -> (Vec<u8>, String) {
        let raw = vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let tx = BsvTransaction::from_binary(&mut Cursor::new(&raw)).unwrap();
        let txid = tx.id().unwrap();
        (raw, txid)
    }

    /// A transaction spending `parent_txid`:0 -- returns (raw_bytes, txid).
    fn tx_spending(parent_txid: &str) -> (Vec<u8>, String) {
        let mut tx = BsvTransaction::new();
        tx.add_input(TransactionInput {
            source_txid: Some(parent_txid.to_string()),
            ..Default::default()
        })
        .expect("spend input has a source txid");
        let mut raw = Vec::new();
        tx.to_binary(&mut raw).unwrap();
        let txid = tx.id().unwrap();
        (raw, txid)
    }

    /// The core parity fix: a proven txid yields a BEEF that carries its merkle
    /// proof (a bump), not the old degenerate `bump_index: None` / empty-inputs wrapper.
    #[tokio::test]
    async fn build_beef_for_proven_txid_carries_merkle_proof() {
        let (raw, txid) = leaf_tx();
        let mut source = MockTxSource::default();
        source.raw.insert(txid.clone(), raw);
        source.merkle.insert(txid.clone(), bump_bytes_for(&txid));

        let beef = build_beef_for_txid(&source, &txid).await.unwrap();

        assert_eq!(
            beef.bumps.len(),
            1,
            "BEEF must carry the merkle proof (bump)"
        );
        assert_eq!(beef.txs.len(), 1);
        assert_eq!(beef.txs[0].txid, txid);
        assert_eq!(
            beef.txs[0].bump_index,
            Some(0),
            "the proven tx must reference its bump, not None"
        );
    }

    /// Unproven target with a proven parent: the BEEF includes the full ancestry
    /// (target raw tx + parent) and the parent's merkle proof.
    #[tokio::test]
    async fn build_beef_for_unproven_txid_includes_proven_ancestry() {
        let (parent_raw, parent_txid) = leaf_tx();
        let (child_raw, child_txid) = tx_spending(&parent_txid);

        let mut source = MockTxSource::default();
        source.raw.insert(parent_txid.clone(), parent_raw);
        source
            .merkle
            .insert(parent_txid.clone(), bump_bytes_for(&parent_txid));
        source.raw.insert(child_txid.clone(), child_raw);
        // child has no merkle proof (unmined)

        let beef = build_beef_for_txid(&source, &child_txid).await.unwrap();

        assert_eq!(beef.bumps.len(), 1, "ancestry proof must be present");
        assert!(beef.find_txid(&child_txid).is_some(), "child tx included");
        assert!(
            beef.find_txid(&parent_txid).is_some(),
            "proven parent included"
        );
    }

    /// A txid the services cannot resolve is an error, not a silent empty BEEF.
    #[tokio::test]
    async fn build_beef_for_unknown_txid_errors() {
        let source = MockTxSource::default();
        let missing = "ee".repeat(32);
        assert!(
            build_beef_for_txid(&source, &missing).await.is_err(),
            "unknown txid must error"
        );
    }

    /// TS parity: `hashOutputScript` returns the plain (unreversed) sha256 of the
    /// script -- the "hashLE" convention -- and the single reversal into ElectrumX
    /// byte order happens once in `validate_script_hash`. Pre-fix, Rust reversed in
    /// BOTH places, so the value queried at WhatsOnChain had the opposite byte order
    /// from TS and `is_utxo` always returned false for real UTXOs.
    ///
    /// Vectors were computed independently (Python hashlib) for a fixed P2PKH script.
    #[test]
    fn hash_output_script_matches_ts_and_query_is_not_double_reversed() {
        use crate::services::providers::whats_on_chain::validate_script_hash;

        // 76a914 <20-byte zero pubkeyhash> 88ac
        let script = hex::decode("76a914000000000000000000000000000000000000000088ac").unwrap();
        // sha256(script), natural (big-endian) byte order == TS `toHex(sha256(script))`.
        let expected_hash = "75def5fcc8bd1a6e9718970604e2728eb114750f6cfd2a2e2cca9d319679b8ac";
        // The scripthash actually sent to WhatsOnChain: validate_script_hash treats a
        // 32-byte default input as hashLE and reverses it exactly once (ElectrumX order).
        let expected_woc_query = "acb87996319dca2c2e2afd6c0f7514b18e72e204069718976e1abdc8fcf5de75";

        let services = Services::from_chain(Chain::Main);
        let hash = services.hash_output_script(&script);
        assert_eq!(
            hash, expected_hash,
            "hash_output_script must return plain sha256(script) (hashLE convention), matching TS"
        );

        // The end-to-end invariant: feeding that hash through the provider's
        // validate_script_hash (default format -> hashLE) yields exactly one reversal,
        // matching TS's WhatsOnChain query -- not a double-reverse back to big-endian.
        let query = validate_script_hash(&hash, None).unwrap();
        assert_eq!(
            query, expected_woc_query,
            "the scripthash sent to WhatsOnChain must be reverse(sha256(script)), matching TS"
        );
    }
}
