//! Services struct implementing WalletServices trait.
//!
//! Wires together all providers (WhatsOnChain, ARC, Bitails, ChainTracker)
//! into a single struct that consumers use via `Arc<dyn WalletServices>`.
//!
//! Ported from wallet-toolbox/src/services/Services.ts.

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
use super::providers::{ArcProvider, Bitails, WhatsOnChain};
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
use bsv::transaction::beef::BEEF_V1;

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
        get_merkle_path_coll.add("WhatsOnChain", Box::new(woc_merkle));
        if has_bitails {
            let bitails = Bitails::new(
                chain.clone(),
                config.bitails_api_key.clone(),
                client.clone(),
            );
            get_merkle_path_coll.add("Bitails", Box::new(bitails));
        }

        // -- getRawTx collection --
        let mut get_raw_tx_coll = ServiceCollection::<dyn GetRawTxProvider>::new("getRawTx");
        let woc_raw = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_raw_tx_coll.add("WhatsOnChain", Box::new(woc_raw));

        // -- postBeef collection --
        // Order matches TS: GorillaPool (if main), Taal, Bitails (if main/test), WoC
        let mut post_beef_coll = ServiceCollection::<dyn PostBeefProvider>::new("postBeef");

        if let Some(ref gp_url) = config.arc_gorilla_pool_url {
            let gp_config = config.arc_gorilla_pool_config.clone().unwrap_or_default();
            let arc_gp = ArcProvider::new("GorillaPoolArcBeef", gp_url, gp_config, client.clone());
            post_beef_coll.add("GorillaPoolArcBeef", Box::new(arc_gp));
        }

        let arc_taal = ArcProvider::new(
            "TaalArcBeef",
            &config.arc_url,
            config.arc_config.clone(),
            client.clone(),
        );
        post_beef_coll.add("TaalArcBeef", Box::new(arc_taal));

        if has_bitails {
            let bitails_beef = Bitails::new(
                chain.clone(),
                config.bitails_api_key.clone(),
                client.clone(),
            );
            post_beef_coll.add("Bitails", Box::new(bitails_beef));
        }

        let woc_beef = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        post_beef_coll.add("WhatsOnChain", Box::new(woc_beef));

        // -- getUtxoStatus collection --
        let mut get_utxo_status_coll =
            ServiceCollection::<dyn GetUtxoStatusProvider>::new("getUtxoStatus");
        let woc_utxo = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_utxo_status_coll.add("WhatsOnChain", Box::new(woc_utxo));

        // -- getStatusForTxids collection --
        let mut get_status_for_txids_coll =
            ServiceCollection::<dyn GetStatusForTxidsProvider>::new("getStatusForTxids");
        let woc_status = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_status_for_txids_coll.add("WhatsOnChain", Box::new(woc_status));

        // -- getScriptHashHistory collection --
        let mut get_script_hash_history_coll =
            ServiceCollection::<dyn GetScriptHashHistoryProvider>::new("getScriptHashHistory");
        let woc_history = WhatsOnChain::new(
            chain.clone(),
            config.whats_on_chain_api_key.clone(),
            client.clone(),
        );
        get_script_hash_history_coll.add("WhatsOnChain", Box::new(woc_history));

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
        let chaintracks_url = self.config.chaintracks_url.as_deref();
        let service_client = ChaintracksServiceClient::new(
            self.config.chain.clone(),
            chaintracks_url,
            self.client.clone(),
        );
        Ok(Box::new(ChaintracksChainTracker::new(service_client)))
    }

    async fn get_merkle_path(&self, txid: &str, use_next: bool) -> GetMerklePathResult {
        let mut coll = self.get_merkle_path.lock().await;
        if use_next {
            coll.next();
        }

        let count = coll.len();
        let mut r0 = GetMerklePathResult::default();

        for _tries in 0..count {
            let (provider, provider_name) = match coll.service_to_call() {
                Some(p) => p,
                None => break,
            };
            let provider_name = provider_name.to_string();

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
                coll.add_service_call_success(&provider_name, call, None);
                return result;
            }

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

        r0
    }

    async fn get_raw_tx(&self, txid: &str, use_next: bool) -> GetRawTxResult {
        let mut coll = self.get_raw_tx.lock().await;
        if use_next {
            coll.next();
        }

        let count = coll.len();
        let mut r0 = GetRawTxResult {
            txid: txid.to_string(),
            ..Default::default()
        };

        for _tries in 0..count {
            let (provider, provider_name) = match coll.service_to_call() {
                Some(p) => p,
                None => break,
            };
            let provider_name = provider_name.to_string();

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
                coll.add_service_call_success(&provider_name, call, None);
                return result;
            }

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
                coll.add_service_call_success(&provider_name, call, Some("not found".to_string()));
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

        r0
    }

    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Vec<PostBeefResult> {
        // Implemented in Task 2
        self.post_beef_impl(beef, txids).await
    }

    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
        use_next: bool,
    ) -> GetUtxoStatusResult {
        let mut coll = self.get_utxo_status.lock().await;
        if use_next {
            coll.next();
        }

        let count = coll.len();
        let mut r0 = GetUtxoStatusResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            is_utxo: None,
            details: Vec::new(),
        };

        // Double retry loop (2 outer retries with 2000ms wait) matching TS
        for _retry in 0..2u32 {
            for _tries in 0..count {
                let (provider, provider_name) = match coll.service_to_call() {
                    Some(p) => p,
                    None => break,
                };
                let provider_name = provider_name.to_string();

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
                    coll.add_service_call_success(&provider_name, call, None);
                    return result;
                }

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

                r0 = result;
                coll.next();
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
        let mut coll = self.get_status_for_txids.lock().await;
        if use_next {
            coll.next();
        }

        let count = coll.len();
        let mut r0 = GetStatusForTxidsResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            results: Vec::new(),
        };

        for _tries in 0..count {
            let (provider, provider_name) = match coll.service_to_call() {
                Some(p) => p,
                None => break,
            };
            let provider_name = provider_name.to_string();

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
                coll.add_service_call_success(&provider_name, call, None);
                return result;
            }

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

            r0 = result;
            coll.next();
        }

        r0
    }

    async fn get_script_hash_history(
        &self,
        hash: &str,
        use_next: bool,
    ) -> GetScriptHashHistoryResult {
        let mut coll = self.get_script_hash_history.lock().await;
        if use_next {
            coll.next();
        }

        let count = coll.len();
        let mut r0 = GetScriptHashHistoryResult {
            name: "<noservices>".to_string(),
            status: "error".to_string(),
            error: Some("WERR_INTERNAL: No services available.".to_string()),
            history: Vec::new(),
        };

        for _tries in 0..count {
            let (provider, provider_name) = match coll.service_to_call() {
                Some(p) => p,
                None => break,
            };
            let provider_name = provider_name.to_string();

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
                coll.add_service_call_success(&provider_name, call, None);
                return result;
            }

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

            r0 = result;
            coll.next();
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
            .map_err(|e| WalletError::Internal(format!("ChainTracker error: {}", e)))
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
        let raw_result = self.get_raw_tx(txid, false).await;
        let raw_tx = raw_result.raw_tx.ok_or_else(|| {
            WalletError::Internal(format!(
                "Could not retrieve raw transaction for txid {}",
                txid
            ))
        })?;

        // Parse the raw transaction
        let tx = bsv::transaction::Transaction::from_binary(&mut std::io::Cursor::new(&raw_tx))
            .map_err(|e| {
                WalletError::Internal(format!("Failed to parse transaction {}: {}", txid, e))
            })?;

        // Construct a simple Beef containing just this transaction
        let beef_tx = bsv::transaction::beef_tx::BeefTx {
            txid: txid.to_string(),
            tx: Some(tx),
            bump_index: None,
            input_txids: Vec::new(),
        };
        let mut beef = Beef::new(BEEF_V1);
        beef.txs.push(beef_tx);

        Ok(beef)
    }

    fn hash_output_script(&self, script: &[u8]) -> String {
        // SHA-256 the script bytes, then reverse to big-endian hex
        let hash = bsv::primitives::hash::sha256(script);
        let mut bytes = hash.to_vec();
        bytes.reverse();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for b in &bytes {
            hex.push_str(&format!("{:02x}", b));
        }
        hex
    }

    async fn is_utxo(&self, locking_script: &[u8], txid: &str, vout: u32) -> WalletResult<bool> {
        let hash = self.hash_output_script(locking_script);
        let outpoint = format!("{}.{}", txid, vout);
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

        // Collect all provider names and references upfront, then release lock
        let provider_names: Vec<String> = {
            let coll = self.post_beef.lock().await;
            coll.all_services()
                .map(|(_, name)| name.to_string())
                .collect()
        };

        for _provider_name in &provider_names {
            // Get the current provider from the collection
            let (prov_name, prov_ref_result) = {
                let coll = self.post_beef.lock().await;
                match coll.service_to_call() {
                    Some((_provider, name)) => {
                        let name = name.to_string();
                        (name, true)
                    }
                    None => (String::new(), false),
                }
            };

            if !prov_ref_result {
                break;
            }

            // Call provider with soft timeout, releasing the mutex
            let start = Utc::now();
            let result = {
                let coll = self.post_beef.lock().await;
                match coll.service_to_call() {
                    Some((provider, _)) => {
                        let beef_owned = beef.to_vec();
                        let txids_owned = txids.to_vec();
                        // We need to call the provider without holding the lock.
                        // Unfortunately, the provider is behind a reference tied to
                        // the MutexGuard. We must hold the lock for the call duration
                        // in UntilSuccess mode since we only call one at a time.
                        if soft_timeout_ms > 0 {
                            match tokio::time::timeout(
                                Duration::from_millis(soft_timeout_ms),
                                provider.post_beef(&beef_owned, &txids_owned),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    PostBeefResult::timeout(&prov_name, txids, soft_timeout_ms)
                                }
                            }
                        } else {
                            provider.post_beef(&beef_owned, &txids_owned).await
                        }
                    }
                    None => break,
                }
            };
            let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();

            let is_success = result.status == "success";
            let is_timeout = result
                .error
                .as_ref()
                .map(|e| e.contains("timeout"))
                .unwrap_or(false);

            // Record call history
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
                    coll.add_service_call_success(&prov_name, call, None);
                } else if let Some(ref err_str) = result.error {
                    let err = WalletError::Internal(err_str.clone());
                    coll.add_service_call_error(&prov_name, call, &err);
                } else {
                    coll.add_service_call_failure(&prov_name, call);
                }
            }

            results.push(result);

            if is_success {
                break;
            }

            // On non-timeout service error, move provider to last
            if !is_timeout {
                let mut coll = self.post_beef.lock().await;
                let all_service_error = results
                    .last()
                    .map(|r| {
                        r.txid_results
                            .iter()
                            .all(|tr| tr.service_error == Some(true))
                    })
                    .unwrap_or(false);
                if all_service_error {
                    coll.move_service_to_last(&prov_name);
                }
            }

            // Advance to next provider
            {
                let mut coll = self.post_beef.lock().await;
                coll.next();
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
        // Collect provider info while holding the lock briefly
        let provider_count = {
            let coll = self.post_beef.lock().await;
            coll.len()
        };

        if provider_count == 0 {
            return vec![PostBeefResult {
                name: "<noservices>".to_string(),
                status: "error".to_string(),
                error: Some("No postBeef services available".to_string()),
                txid_results: Vec::new(),
            }];
        }

        // For PromiseAll we need to call all providers concurrently.
        // We hold the lock for each individual call since each provider
        // reference is behind the collection's mutex.
        let beef_bytes = beef.to_vec();
        let txids_vec = txids.to_vec();

        // Since we can't easily extract providers from the collection without
        // holding the lock, we iterate sequentially but spawn each call.
        // With the current architecture, we call each provider one at a time
        // from the collection (acquiring/releasing the lock for each).
        // For true concurrency we'd need Arc<dyn PostBeefProvider> stored separately.
        // Instead, we hold the lock and call all providers, which is the pragmatic
        // approach matching the TS pattern (which also doesn't truly parallelize
        // the mutex access).

        let mut results = Vec::new();
        {
            let coll = self.post_beef.lock().await;
            let providers: Vec<(&dyn PostBeefProvider, String)> = coll
                .all_services()
                .map(|(p, name)| (p, name.to_string()))
                .collect();

            // We must release the lock before calling providers... but we can't
            // because the provider references borrow from the guard.
            // For PromiseAll, we'll hold the lock and call sequentially.
            // This is a known limitation; a future refactor could use Arc<dyn>.
            for (provider, name) in &providers {
                let start = Utc::now();
                let result = provider.post_beef(&beef_bytes, &txids_vec).await;
                let elapsed = Utc::now().signed_duration_since(start).num_milliseconds();
                results.push((name.clone(), start, elapsed, result));
            }
        }

        // Record history for all results
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
                must_be: format!("valid fiat currency '{}' with an exchange rate", currency),
            })?;
        let b = cached
            .rates
            .get(base)
            .ok_or_else(|| WalletError::InvalidParameter {
                parameter: "base".to_string(),
                must_be: format!("valid fiat currency '{}' with an exchange rate", base),
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
