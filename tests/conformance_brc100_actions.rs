//! BRC-100 action/write-side conformance runner: `createAction`,
//! `signAction`, `internalizeAction`, `relinquishOutput` — 116 vectors.
//!
//! Consumes the vendored corpus embedded at compile time from
//! `conformance/vectors/wallet/brc100/*.json` (pinned via `conformance/SOURCE`
//! to bsv-blockchain/ts-stack @ 1920a9c1; reference impl
//! `@bsv/sdk@2.0.14 + wallet-toolbox`).
//!
//! # What this corpus actually is — read before touching a failure
//!
//! The upstream TS reference NEVER executed these channels. Its dispatcher
//! (ts-stack `conformance/runner/ts/dispatchers/wallet.ts`) demoted every
//! success vector to intended-skip "pending a funded mock-chain harness" and
//! asserts only expectation shape. The expected values are generator
//! artifacts, provably so:
//!
//! - createaction: every `expected.txid` is sha256d of a synthetic
//!   ZERO-INPUT transaction (version 1, the requested outputs in args order,
//!   locktime 0). `synthetic_expectation_formula_holds` reproduces all 90
//!   from the args — an executable proof that no real wallet (which must add
//!   funding inputs and change) can ever match them. `expected.noSendTxid`
//!   is a field no BRC-100 `CreateActionResult` carries in TS, Go, or Rust.
//! - signaction: expected txids are one hex character repeated 64 times;
//!   `expected.tx` on vector 8 is 200 zero bytes.
//! - internalizeaction: success vectors carry a 12-byte `tx` that is not
//!   valid BEEF (not even a valid transaction).
//! - relinquishoutput: outpoints are placeholder txids (aaa…/bbb…), which
//!   ARE satisfiable because this runner controls storage.
//!
//! This runner therefore does what the reference could not: builds the
//! funded/stateful harness, executes every vector through the real
//! `WalletInterface` methods, and asserts the reproducible contract — the
//! transaction that comes out — while pinning each corpus fiction with an
//! executable characterization that fails loudly if a refresh replaces the
//! synthetic expectations with real ones.
//!
//! # Corpus accommodations (each narrow, each asserted)
//!
//! 1. createaction args carry `noSend`/`acceptDelayedBroadcast` at the TOP
//!    level of `args`; BRC-100 nests them under `options`. Every
//!    implementation's deserializer (TS validateCreateActionArgs, Go and
//!    Rust serde) would silently drop them there. The runner lifts exactly
//!    those two keys into `options` and panics on any other unknown key.
//! 2. signaction success vectors: the placeholder `reference` is replaced
//!    with the real reference returned by a deferred `createAction` that
//!    seeds the in-flight action the vector describes. Placeholder
//!    `sendWith` txids are replaced with real noSend txids.
//! 3. internalizeaction success vectors: the 12-byte placeholder `tx` is
//!    replaced with a real AtomicBEEF whose output 0 matches the vector's
//!    remittance (BRC-29 lock derived from the vector's own
//!    derivation/sender fields). Error vectors run unmodified.
//! 4. relinquishoutput success vectors: storage is seeded with an output at
//!    the vector's exact outpoint and basket; args run unmodified.
//!
//! # Backend seam
//!
//! Wallet construction is behind [`ConformanceBackend`]. The corpus
//! identifies a wallet by `input.root_key`; [`LocalRootKeyBackend`] maps
//! that to a `WalletBuilder` root key. An MPC-backed wallet (provisioned
//! vault, no root key) substitutes its own impl without touching the runner
//! or assertions, which are on `WalletInterface` results. Setup that writes
//! storage rows directly (seeded funding, relinquish outpoints) is listed in
//! `STORAGE_SEEDED_SETUP` so a future backend knows what it must provide its
//! own equivalent for.
//!
//! # Network isolation
//!
//! [`ConformanceServices`] panics by name on any method that would leave the
//! process. The calls a legitimate action path makes are answered locally:
//! `chain`, `hash_output_script`, `get_services_call_history`,
//! `get_chain_tracker` (always-valid local tracker), `n_lock_time_is_final`
//! (const), and `post_beef`, which records the call and returns a canned
//! acceptance — the no-network stand-in for "the network accepted". The
//! runner asserts `post_beef` fires exactly when BRC-100 semantics say an
//! undelayed send happens, and never otherwise.

#![cfg(feature = "sqlite")]

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use bsv::primitives::hash::sha256d;
use bsv::primitives::private_key::PrivateKey;
use bsv::script::{LockingScript, UnlockingScript};
use bsv::transaction::beef::{Beef, BEEF_V1};
use bsv::transaction::beef_tx::BeefTx;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::merkle_path::{MerklePath, MerklePathLeaf};
use bsv::transaction::{
    Transaction as BsvTransaction, TransactionError, TransactionInput, TransactionOutput,
};
use bsv::wallet::interfaces::{
    CreateActionArgs, InternalizeActionArgs, RelinquishOutputArgs, SignActionArgs, WalletInterface,
};

use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::tables::{Output, OutputBasket, Transaction};
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
use bsv_wallet_toolbox::wallet::setup::{SetupWallet, WalletBuilder};

const CREATEACTION: &str = include_str!("../conformance/vectors/wallet/brc100/createaction.json");
const SIGNACTION: &str = include_str!("../conformance/vectors/wallet/brc100/signaction.json");
const INTERNALIZE: &str =
    include_str!("../conformance/vectors/wallet/brc100/internalizeaction.json");
const RELINQUISH: &str = include_str!("../conformance/vectors/wallet/brc100/relinquishoutput.json");

/// Vectors whose SETUP writes storage rows directly instead of arriving at
/// the state through BRC-100 calls. The assertions stay interface-level; a
/// non-local backend (MPC vault) reusing this runner must provide its own
/// equivalent seeding for exactly these:
/// - all 90 createaction vectors: spendable change UTXOs are inserted as
///   rows locked by the backend's own change derivation
///   (`ConformanceBackend::funded_wallet`);
/// - relinquishoutput 1, 2, 3, 6, 7, 8: an output row at the vector's
///   literal outpoint is inserted into the vector's basket.
///
/// signaction and internalizeaction setup goes through the public surface
/// (createAction / fabricated inbound BEEF) apart from the change funding
/// above.
#[allow(dead_code)]
const STORAGE_SEEDED_SETUP: &[&str] = &[
    "wallet.brc100.createaction.*",
    "wallet.brc100.relinquishoutput.1",
    "wallet.brc100.relinquishoutput.2",
    "wallet.brc100.relinquishoutput.3",
    "wallet.brc100.relinquishoutput.6",
    "wallet.brc100.relinquishoutput.7",
    "wallet.brc100.relinquishoutput.8",
];

// ---------------------------------------------------------------------------
// Corpus model
// ---------------------------------------------------------------------------

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
    #[serde(default)]
    skip_reason: Option<String>,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    root_key: Option<String>,
    args: Value,
    #[serde(default)]
    originator: Option<String>,
}

fn load(corpus: &str, want_id: &str) -> VectorFile {
    let f: VectorFile = serde_json::from_str(corpus).expect("corpus JSON must parse");
    assert_eq!(f.id, want_id);
    f
}

fn expects_error(v: &Vector) -> bool {
    v.expected.get("error").and_then(Value::as_bool) == Some(true)
}

const DEFAULT_ROOT: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn root_hex(v: &Vector) -> &str {
    v.input.root_key.as_deref().unwrap_or(DEFAULT_ROOT)
}

// ---------------------------------------------------------------------------
// Network seam: local answers only, everything else panics by name
// ---------------------------------------------------------------------------

struct AlwaysValidTracker;

#[async_trait]
impl ChainTracker for AlwaysValidTracker {
    async fn is_valid_root_for_height(
        &self,
        _root: &str,
        _height: u32,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}

/// Panics on any service method that would leave the process; answers the
/// calls a legitimate offline action path makes locally; records `post_beef`
/// so the runner can assert exactly when a broadcast was attempted.
struct ConformanceServices {
    /// txid lists handed to `post_beef`, in call order.
    posted: Mutex<Vec<Vec<String>>>,
}

impl ConformanceServices {
    fn new() -> Self {
        Self {
            posted: Mutex::new(Vec::new()),
        }
    }

    fn posted_count(&self) -> usize {
        self.posted.lock().unwrap().len()
    }

    fn tripped(method: &str) -> ! {
        panic!(
            "network isolation violated: conformance path called WalletServices::{method}. \
             Vector execution must not require the network."
        );
    }
}

#[async_trait]
impl WalletServices for ConformanceServices {
    fn chain(&self) -> Chain {
        Chain::Test
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        // Local always-valid tracker: merkle roots in fabricated BEEF are
        // accepted without any header source. No network.
        Ok(Box::new(AlwaysValidTracker))
    }

    async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> types::GetMerklePathResult {
        Self::tripped("get_merkle_path")
    }

    async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> types::GetRawTxResult {
        Self::tripped("get_raw_tx")
    }

    async fn post_beef(&self, _beef: &[u8], txids: &[String]) -> Vec<types::PostBeefResult> {
        // Recorded, answered locally with acceptance. The runner asserts this
        // fires exactly for undelayed sends and never otherwise.
        self.posted.lock().unwrap().push(txids.to_vec());
        vec![types::PostBeefResult {
            name: "conformance-local".to_string(),
            status: "success".to_string(),
            error: None,
            txid_results: txids
                .iter()
                .map(|t| types::PostTxResultForTxid {
                    txid: t.clone(),
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
        }]
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

    async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
        Self::tripped("get_header_for_height")
    }

    async fn get_height(&self) -> WalletResult<u32> {
        Self::tripped("get_height")
    }

    async fn n_lock_time_is_final(&self, _input: types::NLockTimeInput) -> WalletResult<bool> {
        // Pure locktime arithmetic in the TS reference; const here (the
        // corpus never sets a locktime in the finality-sensitive range).
        Ok(true)
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
        let mut h = sha256d(script);
        h.reverse();
        hex::encode(h)
    }

    async fn is_utxo(&self, _locking_script: &[u8], _txid: &str, _vout: u32) -> WalletResult<bool> {
        Self::tripped("is_utxo")
    }
}

/// The tripwire fires, so green channels mean the paths stayed off it.
#[tokio::test]
#[should_panic(expected = "network isolation violated")]
async fn the_network_tripwire_fires() {
    let _ = ConformanceServices::new().get_height().await;
}

// ---------------------------------------------------------------------------
// Backend seam
// ---------------------------------------------------------------------------

/// A wallet under conformance plus the service seam handle the runner
/// asserts broadcast behavior through.
struct BackendWallet {
    setup: SetupWallet,
    services: Arc<ConformanceServices>,
}

/// One wallet backend under conformance.
///
/// The runner never constructs a wallet inline: everything goes through this
/// trait so a second custody backend (the MPC-backed enterprise wallet,
/// where signing is a multi-party ceremony over a provisioned vault) can be
/// substituted without touching the vector loop or the assertions.
/// `root_key_hex` is how THIS corpus identifies a wallet; a vault backend
/// maps it to a provisioned vault of its own and may ignore the scalar.
#[async_trait]
trait ConformanceBackend {
    /// A wallet with empty storage.
    async fn fresh_wallet(&self, root_key_hex: &str) -> BackendWallet;

    /// A wallet holding `count` spendable change UTXOs of `satoshis` each,
    /// locked so that THIS backend's signer can spend them.
    async fn funded_wallet(&self, root_key_hex: &str, count: usize, satoshis: i64)
        -> BackendWallet;
}

/// The local custody model: a root private key held in-process, change
/// locked and inputs signed by BRC-42/BRC-29 derivation from that key.
struct LocalRootKeyBackend;

const SEED_PREFIX: &str = "Y29uZm9ybWFuY2U=";
const SEED_SUFFIX: &str = "ZnVuZGluZw==";

#[async_trait]
impl ConformanceBackend for LocalRootKeyBackend {
    async fn fresh_wallet(&self, root_key_hex: &str) -> BackendWallet {
        let services = Arc::new(ConformanceServices::new());
        let root = PrivateKey::from_hex(root_key_hex).expect("vector root_key");
        let setup = WalletBuilder::new()
            .chain(Chain::Test)
            .root_key(root)
            .with_sqlite_memory()
            .with_services(services.clone())
            .without_monitor()
            .build()
            .await
            .expect("build conformance wallet");
        setup
            .storage
            .find_or_insert_user(&setup.identity_key)
            .await
            .expect("ensure user");
        BackendWallet { setup, services }
    }

    async fn funded_wallet(
        &self,
        root_key_hex: &str,
        count: usize,
        satoshis: i64,
    ) -> BackendWallet {
        let w = self.fresh_wallet(root_key_hex).await;
        let root = PrivateKey::from_hex(root_key_hex).expect("vector root_key");
        let lock = ScriptTemplateBRC29::new(SEED_PREFIX.to_string(), SEED_SUFFIX.to_string())
            .lock(&root, &root.to_public_key())
            .expect("BRC-29 funding lock");
        seed_spendable_change(&w.setup, count, satoshis, lock).await;
        w
    }
}

/// Find-or-insert an output basket by name for one user.
async fn ensure_basket(setup: &SetupWallet, user_id: i64, name: &str) -> i64 {
    use bsv_wallet_toolbox::storage::find_args::{FindOutputBasketsArgs, OutputBasketPartial};
    let now = Utc::now().naive_utc();
    let existing = setup
        .storage
        .find_output_baskets(&FindOutputBasketsArgs {
            partial: OutputBasketPartial {
                user_id: Some(user_id),
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .expect("find baskets");
    if let Some(b) = existing.first() {
        return b.basket_id;
    }
    setup
        .storage
        .insert_output_basket(&OutputBasket {
            created_at: now,
            updated_at: now,
            basket_id: 0,
            user_id,
            name: name.to_string(),
            number_of_desired_utxos: 10,
            minimum_desired_utxo_value: 1000,
            is_deleted: false,
        })
        .await
        .expect("insert basket")
}

/// Insert `count` spendable change UTXOs backed by a real serialized funding
/// transaction, so BEEF assembly for later spends works entirely from local
/// storage. Mirrors the fixture in `wallet_signing_provider_tests.rs`.
async fn seed_spendable_change(
    setup: &SetupWallet,
    count: usize,
    satoshis: i64,
    locking_script: Vec<u8>,
) {
    let now = Utc::now().naive_utc();
    let storage = &setup.storage;
    let (user, _) = storage
        .find_or_insert_user(&setup.identity_key)
        .await
        .expect("find_or_insert_user");

    let basket_id = ensure_basket(setup, user.user_id, "default").await;

    let mut funding = BsvTransaction::new();
    for _ in 0..count {
        funding.add_output(TransactionOutput {
            satoshis: Some(satoshis as u64),
            locking_script: LockingScript::from_binary(&locking_script),
            change: false,
        });
    }
    let mut funding_raw = Vec::new();
    funding
        .to_binary(&mut funding_raw)
        .expect("serialize funding tx");
    let funding_txid = funding.id().expect("funding txid");

    let tx_id = storage
        .insert_transaction(&Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id: user.user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: format!("conf-seed-{}", rand::random::<u32>()),
            is_outgoing: false,
            satoshis: satoshis * count as i64,
            description: "conformance funding".to_string(),
            version: Some(funding.version as i32),
            lock_time: Some(funding.lock_time as i32),
            txid: Some(funding_txid.clone()),
            input_beef: None,
            raw_tx: Some(funding_raw),
        })
        .await
        .expect("insert funding tx");

    for i in 0..count {
        storage
            .insert_output(&Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id: user.user_id,
                transaction_id: tx_id,
                basket_id: Some(basket_id),
                spendable: true,
                change: true,
                output_description: Some(format!("conformance change {i}")),
                vout: i as i32,
                satoshis,
                provided_by: StorageProvidedBy::Storage,
                purpose: "change".to_string(),
                output_type: "P2PKH".to_string(),
                txid: Some(funding_txid.clone()),
                sender_identity_key: None,
                derivation_prefix: Some(SEED_PREFIX.to_string()),
                derivation_suffix: Some(SEED_SUFFIX.to_string()),
                custom_instructions: None,
                spent_by: None,
                sequence_number: None,
                spending_description: None,
                script_length: Some(locking_script.len() as i64),
                script_offset: None,
                locking_script: Some(locking_script.clone()),
            })
            .await
            .expect("insert change utxo");
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn txid_hex(raw: &[u8]) -> String {
    let mut h = sha256d(raw);
    h.reverse();
    hex::encode(h)
}

/// The subject transaction of an AtomicBEEF result.
fn subject_tx(beef_bytes: &[u8]) -> BsvTransaction {
    Beef::from_binary(&mut Cursor::new(beef_bytes))
        .expect("parse result BEEF")
        .into_transaction()
        .expect("subject transaction")
}

/// The generator's synthetic expectation: a ZERO-INPUT transaction holding
/// exactly the requested outputs in args order, version 1, locktime 0. This
/// is what every createaction `expected.txid`/`expected.tx` hashes back to —
/// and what no real implementation can produce.
fn synthetic_inputless_tx(args: &Value) -> Vec<u8> {
    let outputs = args["outputs"].as_array().expect("args.outputs");
    assert!(outputs.len() < 0xfd, "varint shortcut only valid < 0xfd");
    let mut raw = vec![0x01, 0x00, 0x00, 0x00]; // version 1
    raw.push(0x00); // zero inputs
    raw.push(outputs.len() as u8);
    for o in outputs {
        let sats = o["satoshis"].as_u64().expect("output.satoshis");
        raw.extend_from_slice(&sats.to_le_bytes());
        let script = hex::decode(o["lockingScript"].as_str().expect("output.lockingScript"))
            .expect("lockingScript hex");
        assert!(script.len() < 0xfd);
        raw.push(script.len() as u8);
        raw.extend_from_slice(&script);
    }
    raw.extend_from_slice(&[0, 0, 0, 0]); // locktime
    raw
}

// ---------------------------------------------------------------------------
// createaction — 90 vectors
// ---------------------------------------------------------------------------

/// 90 vectors, 3 root keys, no corpus skips honoured (the demotions exist
/// because upstream lacked this harness). If a refresh changes the shape,
/// re-verify by hand.
#[test]
fn createaction_corpus_shape() {
    let f = load(CREATEACTION, "wallet.brc100.createaction");
    assert_eq!(f.vectors.len(), 90, "vector count changed on refresh");
    assert_eq!(f.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(
        f.vectors.iter().filter(|v| v.skip_reason.is_some()).count(),
        90,
        "all 90 are upstream-demoted for want of a funded harness; this runner has one"
    );
}

/// Executable proof that the createaction expectations are generator
/// fictions: every expected txid is the hash of a synthetic inputless
/// transaction built from the args, `expected.tx` (60 vectors) is that raw
/// transaction, and `expected.noSendTxid` (30 vectors) merely repeats the
/// txid. A corpus refresh that lands REAL reference outputs breaks this test
/// and must flip the real-result assertions below to exact-match.
#[test]
fn createaction_expected_values_are_synthetic() {
    let f = load(CREATEACTION, "wallet.brc100.createaction");
    let mut with_tx = 0usize;
    let mut with_no_send_txid = 0usize;
    for v in &f.vectors {
        let raw = synthetic_inputless_tx(&v.input.args);
        let want = v.expected["txid"].as_str().expect("expected.txid");
        assert_eq!(
            txid_hex(&raw),
            want,
            "{}: expected.txid is no longer the synthetic inputless-tx hash — \
             the corpus has real expectations now; rework the runner assertions",
            v.id
        );
        if let Some(tx) = v.expected.get("tx") {
            with_tx += 1;
            assert_eq!(
                tx.as_str().expect("expected.tx hex string"),
                hex::encode(&raw),
                "{}: expected.tx is no longer the synthetic inputless tx",
                v.id
            );
        }
        if let Some(nst) = v.expected.get("noSendTxid") {
            with_no_send_txid += 1;
            assert_eq!(
                nst.as_str().unwrap(),
                want,
                "{}: noSendTxid stopped mirroring txid",
                v.id
            );
        }
        assert_eq!(v.expected["status"].as_str(), Some("success"));
    }
    assert_eq!(with_tx, 60);
    assert_eq!(with_no_send_txid, 30);
}

/// Corpus accommodation 1: lift the flat `noSend` / `acceptDelayedBroadcast`
/// into `options` where BRC-100 defines them. Panics on any key this
/// accommodation was not written for, so it cannot silently widen.
fn lift_flat_options(args: &Value, id: &str) -> Value {
    let obj = args.as_object().expect("args object");
    for k in obj.keys() {
        assert!(
            matches!(
                k.as_str(),
                "description" | "outputs" | "labels" | "noSend" | "acceptDelayedBroadcast"
            ),
            "{id}: unexpected createaction args key {k:?} — extend the accommodation deliberately"
        );
    }
    let mut lifted = obj.clone();
    let mut options = serde_json::Map::new();
    if let Some(ns) = lifted.remove("noSend") {
        options.insert("noSend".to_string(), ns);
    }
    if let Some(adb) = lifted.remove("acceptDelayedBroadcast") {
        options.insert("acceptDelayedBroadcast".to_string(), adb);
    }
    lifted.insert("options".to_string(), Value::Object(options));
    Value::Object(lifted)
}

/// PINNED DIVERGENCE — every createaction vector, as vendored.
///
/// All 90 vectors put their outputs in basket "default". The Rust SDK's
/// `validate_basket` (bsv-sdk 0.3.4, validation.rs) enforces the BRC-100
/// reserved-basket rule — `default` is the wallet-managed change basket and
/// may not be named by apps — so `createAction` rejects every vector before
/// construction starts. The TS reference SDK's `validateBasket` is a plain
/// identifier check that ACCEPTS "default" (@bsv/sdk validationHelpers.ts),
/// which is how the generator emitted these args in the first place. Spec
/// says Rust is right; the reference implementation and the corpus disagree
/// with the spec. Phase A pins the rejection; phase B re-runs the vector
/// with only the output `basket` field removed so the entire
/// construction/signing surface is still conformance-tested. If the Rust
/// SDK ever relaxes the rule, phase A fails and this split must collapse
/// back into a single as-vendored run.
const DEFAULT_BASKET_REJECTION: &str = "not 'default'";

#[tokio::test]
async fn createaction_conformance() {
    let backend = LocalRootKeyBackend;
    let f = load(CREATEACTION, "wallet.brc100.createaction");
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;
        run_createaction_vector(&backend, v, &mut failures).await;
    }

    assert_eq!(
        executed, 90,
        "every vector must execute — no silent filtering"
    );
    assert!(
        failures.is_empty(),
        "{} of 90 createAction vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

async fn run_createaction_vector(
    backend: &dyn ConformanceBackend,
    v: &Vector,
    failures: &mut Vec<String>,
) {
    let no_send = v.input.args["noSend"].as_bool().unwrap_or(false);
    let delayed = v.input.args["acceptDelayedBroadcast"]
        .as_bool()
        .unwrap_or(true);

    let lifted = lift_flat_options(&v.input.args, &v.id);
    let as_vendored: CreateActionArgs = match serde_json::from_value(lifted.clone()) {
        Ok(a) => a,
        Err(e) => {
            failures.push(format!("{}: args failed to deserialize: {e}", v.id));
            return;
        }
    };

    // Phase A: as vendored, the pinned default-basket rejection must hold.
    {
        let w = backend.funded_wallet(root_hex(v), 2, 50_000).await;
        match w
            .setup
            .wallet
            .create_action(as_vendored, v.input.originator.as_deref())
            .await
        {
            Ok(_) => failures.push(format!(
                "{}: as-vendored args were ACCEPTED — the pinned default-basket \
                 divergence has resolved; collapse phase A/B back into one run",
                v.id
            )),
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains(DEFAULT_BASKET_REJECTION) {
                    failures.push(format!(
                        "{}: as-vendored rejection changed shape: {msg}",
                        v.id
                    ));
                }
            }
        }
    }

    // Phase B: identical args minus each output's `basket`, so the
    // construction and signing surface actually runs.
    let mut neutralized = lifted;
    for o in neutralized["outputs"].as_array_mut().expect("outputs") {
        o.as_object_mut().unwrap().remove("basket");
    }
    let args: CreateActionArgs = match serde_json::from_value(neutralized) {
        Ok(a) => a,
        Err(e) => {
            failures.push(format!(
                "{}: neutralized args failed to deserialize: {e}",
                v.id
            ));
            return;
        }
    };
    let requested: Vec<(u64, Vec<u8>)> = args
        .outputs
        .iter()
        .map(|o| {
            (
                o.satoshis,
                o.locking_script.clone().expect("vector output script"),
            )
        })
        .collect();

    // Funding: 2 × 50k sats covers the corpus maximum (4600 sats + fee) and
    // leaves change, so the change-derivation path is always exercised.
    let w = backend.funded_wallet(root_hex(v), 2, 50_000).await;

    let result = match w
        .setup
        .wallet
        .create_action(args, v.input.originator.as_deref())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            failures.push(format!(
                "{}: expected status success, createAction failed: {e}",
                v.id
            ));
            return;
        }
    };

    // Result surface: txid and tx must be present in every corpus
    // configuration (signAndProcess defaults true; returnTXIDOnly unset).
    let Some(txid) = result.txid.clone() else {
        failures.push(format!("{}: result carries no txid", v.id));
        return;
    };
    let Some(beef_bytes) = result.tx.clone() else {
        failures.push(format!("{}: result carries no tx (AtomicBEEF)", v.id));
        return;
    };
    if result.signable_transaction.is_some() {
        failures.push(format!(
            "{}: signAndProcess defaulted true but a signableTransaction came back",
            v.id
        ));
        return;
    }

    let tx = subject_tx(&beef_bytes);
    let mut tx_raw = Vec::new();
    tx.to_binary(&mut tx_raw).expect("serialize subject tx");
    if txid_hex(&tx_raw) != txid {
        failures.push(format!(
            "{}: result.txid {txid} is not the hash of the subject tx in result.tx",
            v.id
        ));
        return;
    }

    // The consequential claim: the transaction pays exactly what was asked.
    // Every requested (satoshis, lockingScript) appears; order is not
    // asserted because BRC-100 randomizeOutputs defaults true.
    let mut actual: Vec<(u64, Vec<u8>)> = tx
        .outputs
        .iter()
        .map(|o| (o.satoshis.unwrap_or(0), o.locking_script.to_binary()))
        .collect();
    for (sats, script) in &requested {
        match actual.iter().position(|(s, l)| s == sats && l == script) {
            Some(i) => {
                actual.remove(i);
            }
            None => failures.push(format!(
                "{}: requested output ({} sats, {}) missing from the built transaction",
                v.id,
                sats,
                hex::encode(script)
            )),
        }
    }
    // What remains must be change: non-empty funding means at least one
    // input, and outputs cannot exceed the seeded funding (2 × 50k).
    if tx.inputs.is_empty() {
        failures.push(format!(
            "{}: built transaction has no inputs — cannot be a funded action",
            v.id
        ));
    }
    let out_total: u64 = tx.outputs.iter().map(|o| o.satoshis.unwrap_or(0)).sum();
    if out_total > 100_000 {
        failures.push(format!(
            "{}: outputs {out_total} exceed the 100000 sats of seeded funding",
            v.id
        ));
    }

    // Broadcast semantics: post_beef fires exactly when the send is neither
    // suppressed (noSend) nor deferred to the monitor (acceptDelayedBroadcast).
    let want_posts = usize::from(!no_send && !delayed);
    let got_posts = w.services.posted_count();
    if got_posts != want_posts {
        failures.push(format!(
            "{}: post_beef called {got_posts} times, want {want_posts} (noSend={no_send}, delayed={delayed})",
            v.id
        ));
    }
}

// ---------------------------------------------------------------------------
// signaction — 8 vectors
// ---------------------------------------------------------------------------

#[test]
fn signaction_corpus_shape() {
    let f = load(SIGNACTION, "wallet.brc100.signaction");
    assert_eq!(f.vectors.len(), 8, "vector count changed on refresh");
    assert_eq!(f.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(f.vectors.iter().filter(|v| expects_error(v)).count(), 2);
}

/// The signaction success expectations are placeholders: each txid is one
/// hex character repeated 64 times, and vector 8's `tx` is all zeros.
#[test]
fn signaction_expected_values_are_placeholders() {
    let f = load(SIGNACTION, "wallet.brc100.signaction");
    for v in &f.vectors {
        if expects_error(v) {
            continue;
        }
        let txid = v.expected["txid"].as_str().expect("expected.txid");
        let c = txid.chars().next().unwrap();
        assert!(
            txid.len() == 64 && txid.chars().all(|x| x == c),
            "{}: expected.txid is no longer a repeated-character placeholder — \
             the corpus has real expectations now; rework the runner assertions",
            v.id
        );
        if let Some(tx) = v.expected.get("tx") {
            assert!(
                tx.as_array().unwrap().iter().all(|b| b.as_u64() == Some(0)),
                "{}: expected.tx is no longer an all-zero placeholder",
                v.id
            );
        }
    }
}

/// The in-flight state a signaction vector references: a deferred
/// createAction with `caller_inputs` caller-supplied inputs (fabricated
/// OP_TRUE prevouts carried in inputBEEF) plus one payment output.
/// Returns the real reference (base64) the vector's placeholder stands for.
async fn seed_pending_action(w: &BackendWallet, caller_inputs: usize) -> String {
    // A "proven" source transaction with OP_TRUE outputs, wrapped in a BEEF
    // with a single-leaf bump. The always-valid local tracker accepts it.
    let mut source = BsvTransaction::new();
    source.version = 1;
    source.add_input(TransactionInput {
        source_transaction: None,
        source_txid: Some("e".repeat(64)),
        source_output_index: 0,
        unlocking_script: Some(UnlockingScript::from_binary(&[0x00])),
        sequence: 0xFFFF_FFFF,
    });
    for _ in 0..caller_inputs {
        source.add_output(TransactionOutput {
            satoshis: Some(10_000),
            // OP_DROP OP_TRUE: consumes the corpus's placeholder-signature
            // push and leaves a clean single true, so the spliced spend
            // passes the interpreter's clean-stack rule in
            // verify_unlock_scripts (a bare OP_TRUE lock does not).
            locking_script: LockingScript::from_binary(&[0x75, 0x51]),
            change: false,
        });
    }
    let source_txid = source.id().expect("source txid");

    let bump = MerklePath::new(
        800_000,
        vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(source_txid.clone()),
            txid: true,
            duplicate: false,
        }]],
    )
    .expect("bump");
    let mut beef = Beef::new(BEEF_V1);
    beef.bumps.push(bump);
    beef.txs
        .push(BeefTx::from_tx(source, Some(0)).expect("beef tx"));
    let mut input_beef = Vec::new();
    beef.to_binary(&mut input_beef)
        .expect("serialize inputBEEF");

    // The corpus placeholder spends carry a 72-byte unlocking script; the
    // declared length must match for the deferred action to accept them.
    let inputs = (0..caller_inputs)
        .map(|i| {
            serde_json::json!({
                "outpoint": format!("{source_txid}.{i}"),
                "inputDescription": format!("conformance caller input {i}"),
                "unlockingScriptLength": 72,
            })
        })
        .collect::<Vec<_>>();

    let args: CreateActionArgs = serde_json::from_value(serde_json::json!({
        "description": "conformance pending action",
        "inputs": inputs,
        "outputs": [{
            "lockingScript": "76a914bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb88ac",
            "satoshis": 1000,
            "outputDescription": "payment",
        }],
        "options": { "signAndProcess": false },
    }))
    .expect("pending action args");
    // inputBEEF is bytes; attach after JSON parse to avoid a giant array literal.
    let mut args = args;
    args.input_beef = Some(input_beef);

    let created = w
        .setup
        .wallet
        .create_action(args, None)
        .await
        .expect("deferred createAction must produce a pending action");
    let signable = created
        .signable_transaction
        .expect("deferred createAction returns signableTransaction");
    base64::engine::general_purpose::STANDARD.encode(signable.reference)
}

/// signaction.5 (sendWith batch) FAILS in Rust: `storage_process_action`
/// has no port of the TS sendWith mechanism (processAction.ts
/// `shareReqsWithWorld` — req classification, batch assignment, nosend→
/// unsent transitions, `aggregateActionResults`), so `sendWithResults` is
/// always empty and the batched noSend transactions are never released. The
/// TS reference returns one entry per batched txid plus the subject
/// transaction, status 'sending' on the delayed path. A real missing
/// feature in this crate, not a corpus defect — porting it is its own work
/// item, recorded here rather than papered over.
const KNOWN_SIGNACTION_DIVERGENCES: &[&str] = &["wallet.brc100.signaction.5"];

#[tokio::test]
async fn signaction_conformance() {
    let backend = LocalRootKeyBackend;
    let f = load(SIGNACTION, "wallet.brc100.signaction");
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;
        run_signaction_vector(&backend, v, &mut failures).await;
    }

    assert_eq!(
        executed, 8,
        "every vector must execute — no silent filtering"
    );
    assert_known_divergences("signaction", &failures, KNOWN_SIGNACTION_DIVERGENCES);
}

async fn run_signaction_vector(
    backend: &dyn ConformanceBackend,
    v: &Vector,
    failures: &mut Vec<String>,
) {
    let w = backend.funded_wallet(root_hex(v), 2, 50_000).await;

    // How many caller inputs the vector's spends map implies (vector 7's
    // empty map still needs a pending action with one caller input so the
    // mismatch is real).
    let spends_len = v.input.args["spends"].as_object().map_or(0, |m| m.len());
    let caller_inputs = spends_len.max(1);

    // Error vector 6 references an unknown action on purpose: run its args
    // untouched. Everything else gets the real reference its placeholder
    // stands for (accommodation 2).
    let mut args_json = v.input.args.clone();
    let expect_error = expects_error(v);
    let unknown_reference = v.id == "wallet.brc100.signaction.6";
    if !unknown_reference {
        let reference = seed_pending_action(&w, caller_inputs).await;
        args_json["reference"] = Value::String(reference);
    }

    // Accommodation 2 (sendWith): vector 5's placeholder batch txids are
    // replaced with real noSend transactions created on this wallet.
    let send_with_placeholders = args_json["options"]["sendWith"]
        .as_array()
        .map_or(0, |a| a.len());
    if send_with_placeholders > 0 {
        let mut real = Vec::new();
        for i in 0..send_with_placeholders {
            let args: CreateActionArgs = serde_json::from_value(serde_json::json!({
                "description": format!("conformance nosend batch {i}"),
                "outputs": [{
                    "lockingScript": "76a914cccccccccccccccccccccccccccccccccccccccc88ac",
                    "satoshis": 500,
                    "outputDescription": "batched payment",
                }],
                "options": { "noSend": true },
            }))
            .expect("noSend batch args");
            let r = w
                .setup
                .wallet
                .create_action(args, None)
                .await
                .expect("noSend createAction for sendWith batch");
            real.push(Value::String(r.txid.expect("noSend txid")));
        }
        args_json["options"]["sendWith"] = Value::Array(real);
    }

    let posts_before = w.services.posted_count();

    let args: SignActionArgs = match serde_json::from_value(args_json) {
        Ok(a) => a,
        Err(e) => {
            if expect_error {
                return; // malformed by design; rejection at the wire is the contract
            }
            failures.push(format!("{}: args failed to deserialize: {e}", v.id));
            return;
        }
    };
    let spends = args.spends.clone();
    let no_send = args
        .options
        .as_ref()
        .and_then(|o| o.no_send.0)
        .unwrap_or(false);
    let delayed = args
        .options
        .as_ref()
        .and_then(|o| o.accept_delayed_broadcast.0)
        .unwrap_or(true);
    let return_txid_only = args
        .options
        .as_ref()
        .and_then(|o| o.return_txid_only.0)
        .unwrap_or(false);
    let send_with_count = args.options.as_ref().map_or(0, |o| o.send_with.len());

    let outcome = w
        .setup
        .wallet
        .sign_action(args, v.input.originator.as_deref())
        .await;

    if expect_error {
        if let Ok(r) = outcome {
            failures.push(format!(
                "{}: expected error ({}), but signAction succeeded with txid {:?}",
                v.id,
                v.expected
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unspecified"),
                r.txid
            ));
        }
        return;
    }

    let result = match outcome {
        Ok(r) => r,
        Err(e) => {
            failures.push(format!("{}: signAction failed: {e}", v.id));
            return;
        }
    };

    let Some(txid) = result.txid.clone() else {
        failures.push(format!("{}: result carries no txid", v.id));
        return;
    };

    // returnTXIDOnly must suppress tx; otherwise the completed AtomicBEEF
    // comes back and must hash to the returned txid with every caller spend
    // spliced in verbatim.
    if return_txid_only {
        if result.tx.is_some() {
            failures.push(format!(
                "{}: returnTXIDOnly=true but result.tx present",
                v.id
            ));
        }
    } else {
        let Some(beef_bytes) = result.tx.clone() else {
            failures.push(format!("{}: result carries no tx (AtomicBEEF)", v.id));
            return;
        };
        let tx = subject_tx(&beef_bytes);
        let mut raw = Vec::new();
        tx.to_binary(&mut raw).expect("serialize signed tx");
        if txid_hex(&raw) != txid {
            failures.push(format!(
                "{}: result.txid {txid} is not the hash of the tx in result.tx",
                v.id
            ));
        }
        for (vin, spend) in &spends {
            let Some(input) = tx.inputs.get(*vin as usize) else {
                failures.push(format!("{}: spend index {vin} not in final tx", v.id));
                continue;
            };
            let got = input
                .unlocking_script
                .as_ref()
                .map(|s| s.to_binary())
                .unwrap_or_default();
            if got != spend.unlocking_script {
                failures.push(format!(
                    "{}: input {vin} unlocking script was not spliced verbatim (got {}, want {})",
                    v.id,
                    hex::encode(&got),
                    hex::encode(&spend.unlocking_script)
                ));
            }
            if let Some(seq) = spend.sequence_number {
                if input.sequence != seq {
                    failures.push(format!(
                        "{}: input {vin} sequence {} != requested {seq}",
                        v.id, input.sequence
                    ));
                }
            }
        }
    }

    // sendWith semantics: every batched noSend txid comes back with a status.
    if send_with_count > 0 {
        if result.send_with_results.len() != send_with_count {
            failures.push(format!(
                "{}: sendWithResults has {} entries, want {send_with_count}",
                v.id,
                result.send_with_results.len()
            ));
        }
        for r in &result.send_with_results {
            if r.status.as_str() != "sending" && r.status.as_str() != "unproven" {
                failures.push(format!(
                    "{}: sendWith txid {} has status {:?}, want sending/unproven",
                    v.id,
                    r.txid,
                    r.status.as_str()
                ));
            }
        }
    }

    // Broadcast semantics, same rule as createAction.
    let want_posts = usize::from(!no_send && !delayed);
    let got_posts = w.services.posted_count() - posts_before;
    if got_posts != want_posts {
        failures.push(format!(
            "{}: post_beef called {got_posts} times, want {want_posts} (noSend={no_send}, delayed={delayed})",
            v.id
        ));
    }
}

// ---------------------------------------------------------------------------
// internalizeaction — 10 vectors
// ---------------------------------------------------------------------------

#[test]
fn internalizeaction_corpus_shape() {
    let f = load(INTERNALIZE, "wallet.brc100.internalizeaction");
    assert_eq!(f.vectors.len(), 10, "vector count changed on refresh");
    assert_eq!(f.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(f.vectors.iter().filter(|v| expects_error(v)).count(), 2);
    // The 8 success vectors all carry the same 12-byte placeholder that is
    // not valid BEEF — the defect accommodation 3 substitutes around.
    for v in &f.vectors {
        if !expects_error(v) {
            assert_eq!(
                v.input.args["tx"].as_array().map(|a| a.len()),
                Some(12),
                "{}: success vector no longer carries the 12-byte placeholder tx — \
                 the corpus has real BEEF now; drop the substitution",
                v.id
            );
        }
    }
}

/// Build a real AtomicBEEF whose output 0 satisfies the vector's remittance:
/// for wallet payments, a BRC-29 lock derived from the vector's own
/// derivation fields and sender key (privkey 1 — the corpus sender identity
/// is the secp256k1 generator point) to the receiving wallet's identity; for
/// basket insertions, an arbitrary script.
fn build_internalize_beef(outputs: &Value, receiver_root_hex: &str) -> Vec<u8> {
    let receiver_pub = PrivateKey::from_hex(receiver_root_hex)
        .expect("receiver root")
        .to_public_key();

    // One transaction output per distinct outputIndex, highest index decides count.
    let specs = outputs.as_array().expect("outputs array");
    let max_index = specs
        .iter()
        .map(|o| o["outputIndex"].as_u64().unwrap_or(0).min(3) as usize)
        .max()
        .unwrap_or(0);

    let mut scripts: Vec<Vec<u8>> = vec![vec![0x51]; max_index + 1];
    for o in specs {
        let idx = o["outputIndex"].as_u64().unwrap_or(0) as usize;
        if idx > max_index {
            continue; // out-of-range vector (7): leave the tx small on purpose
        }
        if o["protocol"].as_str() == Some("wallet payment") {
            let rem = &o["paymentRemittance"];
            let prefix = rem["derivationPrefix"].as_str().expect("derivationPrefix");
            let suffix = rem["derivationSuffix"].as_str().expect("derivationSuffix");
            let sender_pub_hex = rem["senderIdentityKey"]
                .as_str()
                .expect("senderIdentityKey");
            // The corpus's sender identity is G — privkey 1. The lock must be
            // built exactly as that sender would.
            let sender_priv = PrivateKey::from_hex(
                "0000000000000000000000000000000000000000000000000000000000000001",
            )
            .unwrap();
            assert_eq!(
                sender_priv.to_public_key().to_der_hex(),
                sender_pub_hex,
                "corpus senderIdentityKey is no longer privkey-1; rework the fixture"
            );
            scripts[idx] = ScriptTemplateBRC29::new(prefix.to_string(), suffix.to_string())
                .lock(&sender_priv, &receiver_pub)
                .expect("BRC-29 sender lock");
        }
    }

    let mut source = BsvTransaction::new();
    source.version = 1;
    source.add_input(TransactionInput {
        source_transaction: None,
        source_txid: Some("d".repeat(64)),
        source_output_index: 0,
        unlocking_script: Some(UnlockingScript::from_binary(&[0x00])),
        sequence: 0xFFFF_FFFF,
    });
    for script in &scripts {
        source.add_output(TransactionOutput {
            satoshis: Some(4321),
            locking_script: LockingScript::from_binary(script),
            change: false,
        });
    }
    let txid = source.id().expect("txid");

    let bump = MerklePath::new(
        800_001,
        vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(txid.clone()),
            txid: true,
            duplicate: false,
        }]],
    )
    .expect("bump");
    let mut beef = Beef::new(BEEF_V1);
    beef.bumps.push(bump);
    beef.txs
        .push(BeefTx::from_tx(source, Some(0)).expect("beef tx"));
    beef.to_binary_atomic(&txid).expect("serialize atomic beef")
}

/// internalizeaction.3 ("mixed internalization") supplies BOTH a wallet
/// payment and a basket insertion for the SAME outputIndex 0 and expects
/// accepted:true. Both implementations reject duplicate output indexes —
/// the TS reference by design (storage/methods/internalizeAction.ts:
/// `throw new WERR_INVALID_PARAMETER('outputs', 'unique outputIndex
/// values')`), Rust by accident (no duplicate check; the second row insert
/// dies on the outputs UNIQUE(transactionId, vout, userId) constraint as a
/// WERR_INTERNAL, after the first row was already written). The corpus
/// expectation exceeds both implementations and should be fixed upstream;
/// the Rust error class and the partial write are a real parity gap worth
/// fixing in this crate even so.
const KNOWN_INTERNALIZE_DIVERGENCES: &[&str] = &["wallet.brc100.internalizeaction.3"];

fn assert_known_divergences(channel: &str, failures: &[String], known: &[&str]) {
    let mut unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| !known.iter().any(|k| f.starts_with(*k)))
        .collect();
    let mut resolved: Vec<&&str> = known
        .iter()
        .filter(|k| !failures.iter().any(|f| f.starts_with(**k)))
        .collect();
    assert!(
        unexpected.is_empty() && resolved.is_empty(),
        "{channel}: divergence ledger out of date.\nUnexpected failures:\n{}\nResolved (remove from ledger):\n{}\nAll failures:\n{}",
        unexpected.drain(..).map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"),
        resolved.drain(..).map(|k| format!("  {k}")).collect::<Vec<_>>().join("\n"),
        failures.join("\n"),
    );
}

#[tokio::test]
async fn internalizeaction_conformance() {
    let backend = LocalRootKeyBackend;
    let f = load(INTERNALIZE, "wallet.brc100.internalizeaction");
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;
        run_internalize_vector(&backend, v, &mut failures).await;
    }

    assert_eq!(
        executed, 10,
        "every vector must execute — no silent filtering"
    );
    assert_known_divergences(
        "internalizeaction",
        &failures,
        KNOWN_INTERNALIZE_DIVERGENCES,
    );
}

async fn run_internalize_vector(
    backend: &dyn ConformanceBackend,
    v: &Vector,
    failures: &mut Vec<String>,
) {
    let w = backend.fresh_wallet(root_hex(v)).await;
    let expect_error = expects_error(v);

    let mut args_json = v.input.args.clone();
    // Vector 6's tx is deliberately invalid BEEF — that IS the vector. Every
    // other vector's 12-byte placeholder is the corpus defect accommodation 3
    // substitutes a real AtomicBEEF for (including error vector 7, whose
    // out-of-range outputIndex must then fail for the REAL reason).
    if v.id != "wallet.brc100.internalizeaction.6" {
        let beef = build_internalize_beef(&args_json["outputs"], root_hex(v));
        args_json["tx"] = Value::Array(beef.into_iter().map(|b| Value::from(b as u64)).collect());
    }

    let args: InternalizeActionArgs = match serde_json::from_value(args_json) {
        Ok(a) => a,
        Err(e) => {
            failures.push(format!("{}: args failed to deserialize: {e}", v.id));
            return;
        }
    };

    let outcome = w
        .setup
        .wallet
        .internalize_action(args, v.input.originator.as_deref())
        .await;

    if expect_error {
        if outcome.is_ok() {
            failures.push(format!(
                "{}: expected error ({}), but internalizeAction accepted",
                v.id,
                v.expected
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unspecified")
            ));
        }
        return;
    }

    match outcome {
        Ok(r) => {
            if !r.accepted {
                failures.push(format!("{}: accepted=false, want true", v.id));
            }
        }
        Err(e) => failures.push(format!("{}: expected accepted=true, got error: {e}", v.id)),
    }
}

// ---------------------------------------------------------------------------
// relinquishoutput — 8 vectors
// ---------------------------------------------------------------------------

#[test]
fn relinquishoutput_corpus_shape() {
    let f = load(RELINQUISH, "wallet.brc100.relinquishoutput");
    assert_eq!(f.vectors.len(), 8, "vector count changed on refresh");
    assert_eq!(f.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(f.vectors.iter().filter(|v| expects_error(v)).count(), 2);
}

/// Seed one spendable output at the vector's literal outpoint, inside the
/// vector's basket, so the success vectors run with args unmodified.
async fn seed_output_at(setup: &SetupWallet, basket_name: &str, txid: &str, vout: i32) {
    let now = Utc::now().naive_utc();
    let storage = &setup.storage;
    let (user, _) = storage
        .find_or_insert_user(&setup.identity_key)
        .await
        .expect("user");
    let basket_id = ensure_basket(setup, user.user_id, basket_name).await;
    let tx_id = storage
        .insert_transaction(&Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id: user.user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: format!("conf-relinquish-{}", rand::random::<u32>()),
            is_outgoing: false,
            satoshis: 2000,
            description: "relinquish seed".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some(txid.to_string()),
            input_beef: None,
            raw_tx: None,
        })
        .await
        .expect("insert tx");
    storage
        .insert_output(&Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id: user.user_id,
            transaction_id: tx_id,
            basket_id: Some(basket_id),
            spendable: true,
            change: false,
            output_description: Some("relinquish seed".to_string()),
            vout,
            satoshis: 2000,
            provided_by: StorageProvidedBy::You,
            purpose: "".to_string(),
            output_type: "custom".to_string(),
            txid: Some(txid.to_string()),
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: None,
            script_offset: None,
            locking_script: None,
        })
        .await
        .expect("insert output");
}

#[tokio::test]
async fn relinquishoutput_conformance() {
    let backend = LocalRootKeyBackend;
    let f = load(RELINQUISH, "wallet.brc100.relinquishoutput");
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;
        run_relinquish_vector(&backend, v, &mut failures).await;
    }

    assert_eq!(
        executed, 8,
        "every vector must execute — no silent filtering"
    );
    assert!(
        failures.is_empty(),
        "{} of 8 relinquishOutput vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

async fn run_relinquish_vector(
    backend: &dyn ConformanceBackend,
    v: &Vector,
    failures: &mut Vec<String>,
) {
    let w = backend.fresh_wallet(root_hex(v)).await;
    let expect_error = expects_error(v);

    // Success vectors describe an output that exists at the args outpoint —
    // seed exactly that (accommodation 4), args untouched.
    if !expect_error {
        let outpoint = v.input.args["output"].as_str().expect("args.output");
        let (txid, vout) = outpoint.rsplit_once('.').expect("outpoint txid.vout");
        seed_output_at(
            &w.setup,
            v.input.args["basket"].as_str().expect("args.basket"),
            txid,
            vout.parse().expect("numeric vout"),
        )
        .await;
    }

    // Vectors 1, 6, 8 name basket "default": phase A pins the same Rust-SDK
    // reserved-basket rejection documented at `DEFAULT_BASKET_REJECTION`
    // (the TS reference accepts "default" here too), then phase B renames
    // the basket — a field the lookup provably ignores in BOTH
    // implementations (TS StorageProvider.ts relinquishOutput finds by
    // {userId, txid, vout} only; the Rust port is line-for-line the same) —
    // so the storage path still runs.
    let default_basket = v.input.args["basket"].as_str() == Some("default");
    let mut args_json = v.input.args.clone();
    if default_basket && !expect_error {
        let as_vendored: RelinquishOutputArgs =
            serde_json::from_value(args_json.clone()).expect("relinquish args");
        match w
            .setup
            .wallet
            .relinquish_output(as_vendored, v.input.originator.as_deref())
            .await
        {
            Ok(_) => failures.push(format!(
                "{}: as-vendored basket 'default' was ACCEPTED — the pinned \
                 default-basket divergence has resolved; collapse phase A/B",
                v.id
            )),
            Err(e) => {
                let msg = e.to_string();
                if !msg.contains(DEFAULT_BASKET_REJECTION) {
                    failures.push(format!(
                        "{}: as-vendored rejection changed shape: {msg}",
                        v.id
                    ));
                }
            }
        }
        args_json["basket"] = Value::String("seeded".to_string());
    }

    let args: RelinquishOutputArgs = match serde_json::from_value(args_json) {
        Ok(a) => a,
        Err(e) => {
            if expect_error {
                return; // malformed by design; wire rejection is the contract
            }
            failures.push(format!("{}: args failed to deserialize: {e}", v.id));
            return;
        }
    };

    let outcome = w
        .setup
        .wallet
        .relinquish_output(args, v.input.originator.as_deref())
        .await;

    if expect_error {
        if outcome.is_ok() {
            failures.push(format!(
                "{}: expected error ({}), but relinquishOutput succeeded",
                v.id,
                v.expected
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unspecified")
            ));
        }
        return;
    }

    // The corpus asserts only `relinquished: true`. No dispossession check
    // is layered on top: relinquishOutput is idempotent in the TS reference
    // (the lookup ignores the basket and a cleared basketId still matches),
    // and the Rust port matches that behavior exactly.
    match outcome {
        Ok(r) => {
            if !r.relinquished {
                failures.push(format!("{}: relinquished=false, want true", v.id));
            }
        }
        Err(e) => failures.push(format!(
            "{}: expected relinquished=true, got error: {e}",
            v.id
        )),
    }
}

// ---------------------------------------------------------------------------
// rust-mpc#300 action-arc seams — TS-reference parity pins
//
// The atlas-certifier conformance suite drives the enterprise box's BRC-100
// surface with the flow below (deferred noSend probe → empty-spends
// signAction → listActions by label → abortAction by the returned
// reference). Three of its reds were THIS crate's divergences from the TS
// reference (@bsv/wallet-toolbox + @bsv/sdk):
//
// 1. `validateSignActionArgs` has no "at least one spend" rule — empty
//    `spends` legitimately completes an action whose inputs are all
//    wallet-signed. The vendored Rust SDK invented one.
// 2. `signableTransaction.reference` is base64 TEXT on the wire (TS types it
//    Base64String). Handing the text's UTF-8 bytes to the SDK's
//    `bytes_as_base64` field double-encoded the wire value, and
//    `abortAction`'s re-encoding lookup could then never find the row.
// 3. A listActions row must carry a txid; the unsigned tx's computed id (the
//    value TS itself computes for noSendChange outpoints) is persisted at
//    createAction so an 'unsigned' row is never txid-less.
// ---------------------------------------------------------------------------

/// One deferred noSend action shaped like the atlas conformance probe.
async fn deferred_nosend_probe(w: &BackendWallet) -> bsv::wallet::interfaces::CreateActionResult {
    let label_hex: String = "atlas conformance"
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let args: CreateActionArgs = serde_json::from_value(serde_json::json!({
        "description": "atlas conformance action probe",
        "labels": ["atlas conformance"],
        "outputs": [{
            "satoshis": 1,
            "lockingScript": format!("006a{label_hex}"),
            "outputDescription": "conformance action probe",
            "tags": ["atlas conformance"],
        }],
        "options": {
            "noSend": true,
            "signAndProcess": false,
            "randomizeOutputs": false,
        },
    }))
    .expect("probe args");
    w.setup
        .wallet
        .create_action(args, None)
        .await
        .expect("deferred noSend createAction")
}

/// The wire value of the reference is the STORED reference (no double
/// encoding), and signAction with EMPTY spends completes the wallet-signed
/// action — both exactly as the TS reference behaves.
#[tokio::test]
async fn empty_spends_sign_action_completes_a_wallet_signed_deferred_action() {
    let w = LocalRootKeyBackend
        .funded_wallet(&"6a".repeat(32), 2, 50_000)
        .await;
    let created = deferred_nosend_probe(&w).await;
    let signable = created
        .signable_transaction
        .expect("deferred createAction returns signableTransaction");

    // Wire parity: serde encodes the reference bytes back to base64, and that
    // string must BE the stored reference text (TS Base64String round-trip).
    let wire = serde_json::to_value(&signable).expect("serialize signable");
    let wire_reference = wire["reference"]
        .as_str()
        .expect("reference string")
        .to_string();
    let rows = w
        .setup
        .storage
        .find_transactions(
            &bsv_wallet_toolbox::storage::find_args::FindTransactionsArgs {
                partial: bsv_wallet_toolbox::storage::find_args::TransactionPartial {
                    reference: Some(wire_reference.clone()),
                    ..Default::default()
                },
                no_raw_tx: true,
                ..Default::default()
            },
        )
        .await
        .expect("find by wire reference");
    assert_eq!(
        rows.len(),
        1,
        "the wire reference must be the stored reference — a double-encoded \
         wire value can never find its own action"
    );

    // TS validateSignActionArgs imposes NO minimum on spends: the wallet
    // signs its own funding inputs.
    let args: SignActionArgs = serde_json::from_value(serde_json::json!({
        "reference": wire_reference,
        "spends": {},
        "options": { "noSend": true },
    }))
    .expect("signAction args");
    let signed = w
        .setup
        .wallet
        .sign_action(args, None)
        .await
        .expect("signAction with empty spends must complete a wallet-signed action");
    let txid = signed.txid.expect("signAction returns txid");
    assert_eq!(txid.len(), 64);
    let beef = signed.tx.expect("signAction returns AtomicBEEF");
    let tx = subject_tx(&beef);
    let mut raw = Vec::new();
    tx.to_binary(&mut raw).expect("serialize signed tx");
    assert_eq!(txid_hex(&raw), txid, "returned txid hashes the returned tx");
    assert_eq!(w.services.posted_count(), 0, "noSend must stay unbroadcast");
}

/// abortAction must find the action by the very reference createAction
/// returned — the typed round-trip the double encoding broke.
#[tokio::test]
async fn abort_action_finds_the_action_by_the_reference_create_action_returned() {
    let w = LocalRootKeyBackend
        .funded_wallet(&"6b".repeat(32), 2, 50_000)
        .await;
    let created = deferred_nosend_probe(&w).await;
    let signable = created
        .signable_transaction
        .expect("deferred createAction returns signableTransaction");

    let aborted = w
        .setup
        .wallet
        .abort_action(
            bsv::wallet::interfaces::AbortActionArgs {
                reference: signable.reference.clone(),
            },
            None,
        )
        .await
        .expect("abortAction by the returned reference");
    assert!(aborted.aborted);
}

/// listActions rows always satisfy the BRC-100 row contract: a deferred
/// action lists with the UNSIGNED transaction's 64-hex txid while status is
/// 'unsigned', and with the final txid once signAction completes it.
#[tokio::test]
async fn list_actions_reports_a_real_txid_for_a_deferred_action() {
    let w = LocalRootKeyBackend
        .funded_wallet(&"6c".repeat(32), 2, 50_000)
        .await;
    let created = deferred_nosend_probe(&w).await;
    let signable = created
        .signable_transaction
        .expect("deferred createAction returns signableTransaction");
    let unsigned_tx = subject_tx(&signable.tx);
    let mut unsigned_raw = Vec::new();
    unsigned_tx
        .to_binary(&mut unsigned_raw)
        .expect("serialize unsigned tx");
    let unsigned_txid = txid_hex(&unsigned_raw);

    let list_args = || -> bsv::wallet::interfaces::ListActionsArgs {
        serde_json::from_value(serde_json::json!({
            "labels": ["atlas conformance"],
            "labelQueryMode": "any",
            "includeLabels": true,
            "limit": 100,
        }))
        .expect("listActions args")
    };
    let listed = w
        .setup
        .wallet
        .list_actions(list_args(), None)
        .await
        .expect("listActions");
    let row = listed
        .actions
        .iter()
        .find(|a| a.description == "atlas conformance action probe")
        .expect("the labeled probe must appear under its label");
    assert_eq!(
        row.txid, unsigned_txid,
        "an 'unsigned' row carries the unsigned transaction's computed txid \
         (64 hex), never an empty string"
    );
    assert_eq!(row.txid.len(), 64);
    assert!(row.txid.chars().all(|c| c.is_ascii_hexdigit()));

    // Completing the action replaces the provisional txid with the final one.
    let wire = serde_json::to_value(&signable).expect("serialize signable");
    let args: SignActionArgs = serde_json::from_value(serde_json::json!({
        "reference": wire["reference"],
        "spends": {},
        "options": { "noSend": true },
    }))
    .expect("signAction args");
    let signed = w
        .setup
        .wallet
        .sign_action(args, None)
        .await
        .expect("signAction");
    let final_txid = signed.txid.expect("txid");

    let listed = w
        .setup
        .wallet
        .list_actions(list_args(), None)
        .await
        .expect("listActions after sign");
    let row = listed
        .actions
        .iter()
        .find(|a| a.description == "atlas conformance action probe")
        .expect("the probe still lists after signing");
    assert_eq!(
        row.txid, final_txid,
        "the final txid replaces the provisional one"
    );
}
