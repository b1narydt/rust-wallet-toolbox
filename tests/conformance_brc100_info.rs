//! BRC-100 authentication and chain-info conformance runner (28 vectors).
//!
//! These methods do not read wallet storage. A fresh in-memory wallet is used
//! only to construct the public [`WalletInterface`] surface; chain answers come
//! from [`FixtureServices`], and every unrelated service call is a panic.

#![cfg(feature = "sqlite")]

use std::sync::Arc;

use async_trait::async_trait;
use bsv::primitives::hash::sha256d;
use bsv::primitives::private_key::PrivateKey;
use bsv::transaction::beef::Beef;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::wallet::error::WalletError as SdkWalletError;
use bsv::wallet::interfaces::{GetHeaderArgs, WalletInterface};
use bsv_wallet_toolbox::error::{WalletError, WalletResult};
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::wallet::setup::WalletBuilder;
use bsv_wallet_toolbox::wallet::wallet::Wallet;
use serde::Deserialize;
use serde_json::{json, Value};

const GET_HEADER: &str =
    include_str!("../conformance/vectors/wallet/brc100/getheaderforheight.json");
const GET_VERSION: &str = include_str!("../conformance/vectors/wallet/brc100/getversion.json");
const GET_HEIGHT: &str = include_str!("../conformance/vectors/wallet/brc100/getheight.json");
const IS_AUTHENTICATED: &str =
    include_str!("../conformance/vectors/wallet/brc100/isauthenticated.json");
const WAIT_FOR_AUTHENTICATION: &str =
    include_str!("../conformance/vectors/wallet/brc100/waitforauthentication.json");

const ROOT: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const GENESIS_HEADER: &str = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";

#[derive(Deserialize)]
struct VectorFile {
    id: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: Input,
    expected: Value,
    #[serde(default)]
    skip: bool,
}

#[derive(Deserialize)]
struct Input {
    args: Value,
    #[serde(default)]
    originator: Option<String>,
}

fn load(corpus: &str, want_id: &str) -> VectorFile {
    let file: VectorFile = serde_json::from_str(corpus).expect("corpus JSON must parse");
    assert_eq!(file.id, want_id);
    file
}

struct FixtureServices;

impl FixtureServices {
    fn tripped(method: &str) -> ! {
        panic!("info conformance unexpectedly called WalletServices::{method}")
    }
}

#[async_trait]
impl WalletServices for FixtureServices {
    fn chain(&self) -> Chain {
        Chain::Main
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        Self::tripped("get_chain_tracker")
    }

    async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> types::GetMerklePathResult {
        Self::tripped("get_merkle_path")
    }

    async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> types::GetRawTxResult {
        Self::tripped("get_raw_tx")
    }

    async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> Vec<types::PostBeefResult> {
        Self::tripped("post_beef")
    }

    async fn get_utxo_status(
        &self,
        _output: &str,
        _output_format: Option<types::GetUtxoStatusOutputFormat>,
        _outpoint: Option<&str>,
        _use_next: bool,
    ) -> types::GetUtxoStatusResult {
        Self::tripped("get_utxo_status")
    }

    async fn get_status_for_txids(
        &self,
        _txids: &[String],
        _use_next: bool,
    ) -> types::GetStatusForTxidsResult {
        Self::tripped("get_status_for_txids")
    }

    async fn get_script_hash_history(
        &self,
        _hash: &str,
        _use_next: bool,
    ) -> types::GetScriptHashHistoryResult {
        Self::tripped("get_script_hash_history")
    }

    async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
        Self::tripped("hash_to_header")
    }

    async fn get_header_for_height(&self, height: u32) -> WalletResult<Vec<u8>> {
        if height == i32::MAX as u32 {
            return Err(WalletError::NetworkChain(
                "header not found at requested height".to_string(),
            ));
        }
        if height == 0 {
            return hex::decode(GENESIS_HEADER)
                .map_err(|error| WalletError::Internal(error.to_string()));
        }
        Ok(vec![0; 80])
    }

    async fn get_height(&self) -> WalletResult<u32> {
        Ok(1)
    }

    async fn n_lock_time_is_final(&self, _input: types::NLockTimeInput) -> WalletResult<bool> {
        Self::tripped("n_lock_time_is_final")
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
        Self::tripped("get_bsv_exchange_rate")
    }

    async fn get_fiat_exchange_rate(
        &self,
        _currency: &str,
        _base: Option<&str>,
    ) -> WalletResult<f64> {
        Self::tripped("get_fiat_exchange_rate")
    }

    async fn get_fiat_exchange_rates(
        &self,
        _target_currencies: &[String],
    ) -> WalletResult<types::FiatExchangeRates> {
        Self::tripped("get_fiat_exchange_rates")
    }

    fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
        types::ServicesCallHistory { services: vec![] }
    }

    async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
        Self::tripped("get_beef_for_txid")
    }

    fn hash_output_script(&self, script: &[u8]) -> String {
        let mut hash = sha256d(script);
        hash.reverse();
        hex::encode(hash)
    }

    async fn is_utxo(&self, _locking_script: &[u8], _txid: &str, _vout: u32) -> WalletResult<bool> {
        Self::tripped("is_utxo")
    }
}

async fn build_wallet() -> Wallet {
    WalletBuilder::new()
        .chain(Chain::Main)
        .root_key(PrivateKey::from_hex(ROOT).expect("fixture root"))
        .with_sqlite_memory()
        .with_services(Arc::new(FixtureServices))
        .without_monitor()
        .build()
        .await
        .expect("build fixture wallet")
        .wallet
}

fn assert_known_divergences(channel: &str, failures: &[String], known: &[&str]) {
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|failure| !known.iter().any(|id| failure.starts_with(*id)))
        .collect();
    let resolved: Vec<&&str> = known
        .iter()
        .filter(|id| !failures.iter().any(|failure| failure.starts_with(**id)))
        .collect();
    assert!(
        unexpected.is_empty() && resolved.is_empty(),
        "{channel}: divergence ledger out of date.\nUnexpected failures:\n{}\nResolved (remove from ledger):\n{}\nAll failures:\n{}",
        unexpected.iter().map(|failure| format!("  {failure}")).collect::<Vec<_>>().join("\n"),
        resolved.iter().map(|id| format!("  {id}")).collect::<Vec<_>>().join("\n"),
        failures.join("\n")
    );
}

/// `SPEC_AMBIGUOUS`: the vectors require the genesis header at height zero,
/// while BRC-100 declares `GetHeaderArgs.height` a PositiveInteger excluding
/// zero. The official wallet.ts stub (lines 584-595) nevertheless returns and
/// exactly asserts genesis. Both vector IDs remain individually pinned.
const SPEC_AMBIGUOUS_ZERO_HEIGHT: &[&str] = &[
    "wallet.brc100.getheaderforheight.2:",
    "wallet.brc100.getheaderforheight.8:",
];

#[test]
fn corpus_shape() {
    for (corpus, id, count) in [
        (GET_HEADER, "wallet.brc100.getheaderforheight", 8),
        (GET_VERSION, "wallet.brc100.getversion", 5),
        (GET_HEIGHT, "wallet.brc100.getheight", 5),
        (IS_AUTHENTICATED, "wallet.brc100.isauthenticated", 5),
        (
            WAIT_FOR_AUTHENTICATION,
            "wallet.brc100.waitforauthentication",
            5,
        ),
    ] {
        let file = load(corpus, id);
        assert_eq!(file.vectors.len(), count, "{id}: vector count changed");
        assert_eq!(file.vectors.iter().filter(|v| v.skip).count(), 0);
    }
}

#[tokio::test]
async fn getheaderforheight_conformance() {
    let file = load(GET_HEADER, "wallet.brc100.getheaderforheight");
    let wallet = build_wallet().await;
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let expects_error = vector.expected.get("error").and_then(Value::as_bool) == Some(true);
        let outcome = match serde_json::from_value::<GetHeaderArgs>(vector.input.args.clone()) {
            Ok(args) => wallet
                .get_header_for_height(args, vector.input.originator.as_deref())
                .await
                .map(|result| json!({ "header": hex::encode(result.header) })),
            Err(error) => Err(SdkWalletError::Internal(format!(
                "args failed to deserialize: {error}"
            ))),
        };
        // wallet.ts dispatchGetHeaderForHeight lines 582-595: error fixtures
        // are scenario stubs with no assertion; success compares the exact
        // header selected for the requested height.
        match (expects_error, outcome) {
            (true, _) => {}
            (false, Ok(actual)) if actual == vector.expected => {}
            (false, Ok(actual)) => failures.push(format!(
                "{}: expected {}, got {}",
                vector.id, vector.expected, actual
            )),
            (false, Err(error)) => failures.push(format!(
                "{}: expected {}, got error {error}",
                vector.id, vector.expected
            )),
        }
    }

    assert_eq!(file.vectors.len(), 8, "every vector must execute");
    assert_known_divergences("getHeaderForHeight", &failures, SPEC_AMBIGUOUS_ZERO_HEIGHT);
}

#[tokio::test]
async fn getversion_conformance() {
    let file = load(GET_VERSION, "wallet.brc100.getversion");
    let wallet = build_wallet().await;
    for vector in &file.vectors {
        assert_eq!(vector.input.args, json!({}), "{}: args changed", vector.id);
        if vector.expected.get("error").and_then(Value::as_bool) == Some(true) {
            // wallet.ts dispatchGetVersion line 620: service-unavailable is a
            // state-stub scenario, so the official dispatcher makes no call
            // and no assertion for this vector.
            continue;
        }
        let result = wallet
            .get_version(vector.input.originator.as_deref())
            .await
            .unwrap_or_else(|error| panic!("{}: getVersion failed: {error}", vector.id));
        // wallet.ts dispatchGetVersion lines 617-623: each fixture carries a
        // different string, so conformance is property presence and len >= 7.
        assert!(
            result.version.len() >= 7,
            "{}: version must contain at least seven bytes",
            vector.id
        );
    }

    assert_eq!(file.vectors.len(), 5, "every vector must execute");
}

#[tokio::test]
async fn getheight_conformance() {
    let file = load(GET_HEIGHT, "wallet.brc100.getheight");
    let normal_wallet = build_wallet().await;
    let mut failures = Vec::new();

    for vector in &file.vectors {
        assert_eq!(vector.input.args, json!({}), "{}: args changed", vector.id);
        if vector.expected.get("error").and_then(Value::as_bool) == Some(true) {
            // wallet.ts dispatchGetHeight line 571: ProtoWallet has no state
            // layer, so the official dispatcher does not assert error scenarios.
            continue;
        }
        let actual = normal_wallet
            .get_height(vector.input.originator.as_deref())
            .await
            .map(|result| json!({ "height": result.height }));
        // wallet.ts dispatchGetHeight lines 572-575 asserts height >= 1 and
        // exact equality to the vector's stated height.
        match actual {
            Ok(actual)
                if actual["height"].as_u64().is_some_and(|height| height >= 1)
                    && actual == vector.expected => {}
            Ok(actual) => failures.push(format!(
                "{}: expected positive height {}, got {}",
                vector.id, vector.expected, actual
            )),
            Err(error) => failures.push(format!(
                "{}: expected {}, got error {error}",
                vector.id, vector.expected
            )),
        }
    }

    assert_eq!(file.vectors.len(), 5, "every vector must execute");
    assert!(
        failures.is_empty(),
        "{} getHeight vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn isauthenticated_conformance() {
    let file = load(IS_AUTHENTICATED, "wallet.brc100.isauthenticated");
    let wallet = build_wallet().await;
    for vector in &file.vectors {
        assert_eq!(vector.input.args, json!({}), "{}: args changed", vector.id);
        let result = wallet
            .is_authenticated(vector.input.originator.as_deref())
            .await
            .unwrap_or_else(|error| panic!("{}: isAuthenticated failed: {error}", vector.id));
        // wallet.ts dispatchIsAuthenticated lines 545-553: the locked-wallet
        // vector asserts boolean shape only; all other vectors assert true.
        if vector.expected["authenticated"] == Value::Bool(false) {
            assert!(
                json!(result.authenticated).is_boolean(),
                "{}: authenticated must be boolean",
                vector.id
            );
        } else {
            assert!(
                result.authenticated,
                "{}: expected authenticated",
                vector.id
            );
        }
    }

    assert_eq!(file.vectors.len(), 5, "every vector must execute");
}

#[tokio::test]
async fn waitforauthentication_conformance() {
    let file = load(
        WAIT_FOR_AUTHENTICATION,
        "wallet.brc100.waitforauthentication",
    );
    let wallet = build_wallet().await;

    for vector in &file.vectors {
        assert_eq!(vector.input.args, json!({}), "{}: args changed", vector.id);
        if vector.expected.get("error").and_then(Value::as_bool) == Some(true) {
            // wallet.ts dispatchWaitForAuthentication lines 560-562: timeout
            // and process-close scenarios cannot be reproduced by ProtoWallet,
            // so the official dispatcher intentionally makes no assertion.
            continue;
        }
        let result = wallet
            .wait_for_authentication(vector.input.originator.as_deref())
            .await
            .unwrap_or_else(|error| panic!("{}: waitForAuthentication failed: {error}", vector.id));
        // wallet.ts dispatchWaitForAuthentication line 564 requires true on
        // every successful vector.
        assert!(
            result.authenticated,
            "{}: expected authenticated",
            vector.id
        );
    }

    assert_eq!(file.vectors.len(), 5, "every vector must execute");
}
