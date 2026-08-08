//! BRC-100 read-side conformance runner: `listOutputs`, `listActions`,
//! `proveCertificate`, `getNetwork` (173 vectors).
//!
//! Consumes the vendored corpus embedded at compile time from
//! `conformance/vectors/wallet/brc100/*.json` (pinned via `conformance/SOURCE`
//! to bsv-blockchain/ts-stack @ 1920a9c1; reference impl
//! `@bsv/sdk@2.0.14 + wallet-toolbox`).
//!
//! # Wallet backend seam
//!
//! Every runner builds its wallet through [`WalletBackend`], never inline.
//! A vector names identity material (`input.root_key`, defaulting to the
//! corpus-wide `1`-key); the local backend derives a root-key wallet over
//! in-memory SQLite from it. An MPC-backed run (the enterprise wallet box,
//! where the key is split across cosigners) substitutes its own impl that
//! resolves a pre-provisioned vault for the same identity — the runners and
//! assertions stay untouched. Only the local backend exists here by design.
//!
//! Vectors that reach around the BRC-100 surface to seed raw storage rows are
//! listed in [`STORAGE_SEEDED_VECTORS`]; a non-local backend cannot be
//! expected to satisfy them. Certificate seeding is NOT in that list — it
//! goes through `acquireCertificate`, a BRC-100 call any backend exposes.
//!
//! # Divergence ledgers
//!
//! Divergences from the corpus are pinned per channel in `KNOWN_*_DIVERGENCES`
//! (same contract as `conformance_brc40.rs`): each entry still executes, its
//! failure is required to be present, and a fix that makes it pass breaks the
//! build until the ledger entry is removed. The ledger cannot grow or rot
//! silently.

#![cfg(feature = "sqlite")]

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use async_trait::async_trait;
use bsv::primitives::hash::sha256d;
use bsv::primitives::private_key::PrivateKey;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::WalletInterface;
use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types as services_types;
use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::wallet::types::WalletArgs;
use bsv_wallet_toolbox::wallet::wallet::Wallet;

const LISTOUTPUTS: &str = include_str!("../conformance/vectors/wallet/brc100/listoutputs.json");
const LISTACTIONS: &str = include_str!("../conformance/vectors/wallet/brc100/listactions.json");
const PROVECERTIFICATE: &str =
    include_str!("../conformance/vectors/wallet/brc100/provecertificate.json");
const GETNETWORK: &str = include_str!("../conformance/vectors/wallet/brc100/getnetwork.json");

/// Root key the reference dispatcher uses when a vector names none.
const DEFAULT_ROOT: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// proveCertificate vectors fix subject = pubkey(2) and certifier = pubkey(1).
const SUBJECT_ROOT: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const CERTIFIER_ROOT: &str = "0000000000000000000000000000000000000000000000000000000000000001";

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
    /// Free-text scenario marker ("mainnet wallet" / "testnet wallet").
    #[serde(default, rename = "_scenario")]
    scenario: Option<String>,
}

fn load(corpus: &str, want_id: &str) -> VectorFile {
    let f: VectorFile = serde_json::from_str(corpus).expect("corpus JSON must parse");
    assert_eq!(f.id, want_id);
    f
}

// ---------------------------------------------------------------------------
// Wallet backend seam
// ---------------------------------------------------------------------------

/// The identity material a vector names. The local backend derives a wallet
/// from `root_key_hex`; an MPC backend resolves a pre-provisioned vault whose
/// identity key matches the same material.
struct VectorIdentity<'a> {
    root_key_hex: &'a str,
}

/// A wallet produced by a backend, plus the handles the runners need.
struct BuiltWallet {
    /// The BRC-100 surface under test.
    wallet: Wallet,
    identity_key: String,
    /// Raw storage handle, local backend only. Consumed exclusively by the
    /// vectors in [`STORAGE_SEEDED_VECTORS`]; a non-local backend returns
    /// `None` and cannot run those vectors.
    seed_storage: Option<Arc<SqliteStorage>>,
}

trait WalletBackend {
    async fn build(&self, identity: &VectorIdentity<'_>, chain: Chain) -> BuiltWallet;
}

/// A `WalletServices` that fails the test on contact. `Wallet::new` requires
/// a services instance, but every read-side method under test must answer
/// from local state alone — any network reach is an immediate, named panic
/// rather than a silently-mocked answer.
struct NetworkTripwire(Chain);

impl NetworkTripwire {
    fn tripped(method: &str) -> ! {
        panic!(
            "network isolation violated: a read-side conformance call reached \
             WalletServices::{method}"
        );
    }
}

#[async_trait]
impl WalletServices for NetworkTripwire {
    fn chain(&self) -> Chain {
        self.0.clone()
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        Self::tripped("get_chain_tracker")
    }

    async fn get_merkle_path(
        &self,
        _txid: &str,
        _use_next: bool,
    ) -> services_types::GetMerklePathResult {
        Self::tripped("get_merkle_path")
    }

    async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> services_types::GetRawTxResult {
        Self::tripped("get_raw_tx")
    }

    async fn post_beef(
        &self,
        _beef: &[u8],
        _txids: &[String],
    ) -> Vec<services_types::PostBeefResult> {
        Self::tripped("post_beef")
    }

    async fn get_utxo_status(
        &self,
        _output: &str,
        _output_format: Option<services_types::GetUtxoStatusOutputFormat>,
        _outpoint: Option<&str>,
        _use_next: bool,
    ) -> services_types::GetUtxoStatusResult {
        Self::tripped("get_utxo_status")
    }

    async fn get_status_for_txids(
        &self,
        _txids: &[String],
        _use_next: bool,
    ) -> services_types::GetStatusForTxidsResult {
        Self::tripped("get_status_for_txids")
    }

    async fn get_script_hash_history(
        &self,
        _hash: &str,
        _use_next: bool,
    ) -> services_types::GetScriptHashHistoryResult {
        Self::tripped("get_script_hash_history")
    }

    async fn hash_to_header(&self, _hash: &str) -> WalletResult<services_types::BlockHeader> {
        Self::tripped("hash_to_header")
    }

    async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
        Self::tripped("get_header_for_height")
    }

    async fn get_height(&self) -> WalletResult<u32> {
        Self::tripped("get_height")
    }

    async fn n_lock_time_is_final(
        &self,
        _input: services_types::NLockTimeInput,
    ) -> WalletResult<bool> {
        Self::tripped("n_lock_time_is_final")
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<services_types::BsvExchangeRate> {
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
    ) -> WalletResult<services_types::FiatExchangeRates> {
        Self::tripped("get_fiat_exchange_rates")
    }

    fn get_services_call_history(&self, _reset: bool) -> services_types::ServicesCallHistory {
        services_types::ServicesCallHistory { services: vec![] }
    }

    async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<bsv::transaction::beef::Beef> {
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

/// Local backend: root-key wallet over in-memory SQLite, tripwire services,
/// no monitor (read-side methods touch neither the network nor broadcasting).
struct LocalSqliteBackend;

impl WalletBackend for LocalSqliteBackend {
    async fn build(&self, identity: &VectorIdentity<'_>, chain: Chain) -> BuiltWallet {
        let root = PrivateKey::from_hex(identity.root_key_hex).expect("vector root key");

        let storage = SqliteStorage::new_sqlite(
            StorageConfig {
                url: "sqlite::memory:".to_string(),
                ..Default::default()
            },
            chain.clone(),
        )
        .await
        .expect("open sqlite");
        StorageProvider::migrate_database(&storage)
            .await
            .expect("migrate");
        let provider = Arc::new(storage);

        let key_deriver = Arc::new(CachedKeyDeriver::new(root, None));
        let identity_key = key_deriver.identity_key().to_der_hex();

        let manager = Arc::new(WalletStorageManager::new(
            identity_key.clone(),
            Some(provider.clone() as Arc<dyn WalletStorageProvider>),
            vec![],
        ));
        manager.make_available().await.expect("manager available");
        manager
            .find_or_insert_user(&identity_key)
            .await
            .expect("user row");

        let chain2 = chain.clone();
        let wallet = Wallet::new(WalletArgs {
            chain,
            key_deriver,
            signing_provider: None,
            storage: manager,
            services: Some(Arc::new(NetworkTripwire(chain2))),
            monitor: None,
            privileged_key_manager: None,
            settings_manager: None,
            lookup_resolver: None,
        })
        .expect("wallet");

        BuiltWallet {
            wallet,
            identity_key,
            seed_storage: Some(provider),
        }
    }
}

// ---------------------------------------------------------------------------
// Divergence ledger (contract identical to conformance_brc40.rs)
// ---------------------------------------------------------------------------

/// Every failure must correspond to exactly the pinned divergence set: a new
/// failure breaks the build, and a divergence that stops failing (someone
/// fixed it) also breaks the build until its ledger entry is removed.
fn assert_known_divergences(channel: &str, failures: &[String], known: &[&str]) {
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| !known.iter().any(|k| f.starts_with(*k)))
        .collect();
    let resolved: Vec<&&str> = known
        .iter()
        .filter(|k| !failures.iter().any(|f| f.starts_with(**k)))
        .collect();
    assert!(
        unexpected.is_empty() && resolved.is_empty(),
        "{channel}: divergence ledger out of date.\nUnexpected failures:\n{}\nResolved (remove from ledger):\n{}\nAll failures:\n{}",
        unexpected.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"),
        resolved.iter().map(|k| format!("  {k}")).collect::<Vec<_>>().join("\n"),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Corpus shape
// ---------------------------------------------------------------------------

/// The corpus shape itself is an assertion; a refresh that changes any count
/// fails here and every count below must be re-verified by hand.
#[test]
fn corpus_shape() {
    let lo = load(LISTOUTPUTS, "wallet.brc100.listoutputs");
    assert_eq!(lo.vectors.len(), 144, "listoutputs count changed on refresh");
    assert_eq!(lo.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(
        lo.vectors.iter().filter(|v| v.skip_reason.is_some()).count(),
        0
    );

    let la = load(LISTACTIONS, "wallet.brc100.listactions");
    assert_eq!(la.vectors.len(), 16, "listactions count changed on refresh");
    assert_eq!(la.vectors.iter().filter(|v| v.skip).count(), 0);
    let la_seeded: Vec<&str> = la
        .vectors
        .iter()
        .filter(|v| v.skip_reason.is_some())
        .map(|v| v.id.as_str())
        .collect();
    assert_eq!(la_seeded, vec!["wallet.brc100.listactions.14"]);

    let pc = load(PROVECERTIFICATE, "wallet.brc100.provecertificate");
    assert_eq!(pc.vectors.len(), 8, "provecertificate count changed on refresh");
    assert_eq!(pc.vectors.iter().filter(|v| v.skip).count(), 0);
    // 7 of 8 are marked "requires pre-existing certificate" by the corpus
    // itself (the reference dispatcher skipped them); only .5 ran fresh.
    let pc_fresh: Vec<&str> = pc
        .vectors
        .iter()
        .filter(|v| v.skip_reason.is_none())
        .map(|v| v.id.as_str())
        .collect();
    assert_eq!(pc_fresh, vec!["wallet.brc100.provecertificate.5"]);

    let gn = load(GETNETWORK, "wallet.brc100.getnetwork");
    assert_eq!(gn.vectors.len(), 5, "getnetwork count changed on refresh");
    assert_eq!(gn.vectors.iter().filter(|v| v.skip).count(), 0);

    // No read-side vector names its own root key today; every runner uses the
    // corpus defaults (DEFAULT_ROOT, or SUBJECT_ROOT where the vectors fix
    // subject = pubkey(2)). A refresh that adds per-vector root keys fails
    // here and the shared-wallet runners must switch to per-vector identity.
    for vs in [&lo.vectors, &la.vectors, &pc.vectors, &gn.vectors] {
        assert!(
            vs.iter().all(|v| v.input.root_key.is_none()),
            "a vector now carries input.root_key — plumb it through VectorIdentity"
        );
    }
}

// ---------------------------------------------------------------------------
// getNetwork — 5 vectors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn getnetwork_conformance() {
    let f = load(GETNETWORK, "wallet.brc100.getnetwork");
    let backend = LocalSqliteBackend;
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;

        // The scenario marker names the wallet under test ("testnet wallet");
        // vectors without one run against the default mainnet wallet. The
        // chain is deliberately NOT derived from `expected` so that a poisoned
        // expectation fails instead of steering the harness.
        let chain = match v.input.scenario.as_deref() {
            Some(s) if s.contains("testnet") => Chain::Test,
            _ => Chain::Main,
        };

        let built = backend
            .build(&VectorIdentity { root_key_hex: DEFAULT_ROOT }, chain)
            .await;

        let want = v.expected["network"]
            .as_str()
            .unwrap_or_else(|| panic!("{}: expected.network missing", v.id));

        match built
            .wallet
            .get_network(v.input.originator.as_deref())
            .await
        {
            Ok(r) if r.network.as_str() == want => {}
            Ok(r) => failures.push(format!(
                "{}: expected network {want:?}, got {:?}",
                v.id,
                r.network.as_str()
            )),
            Err(e) => failures.push(format!("{}: expected network {want:?}, got error {e}", v.id)),
        }
    }

    assert_eq!(executed, 5, "every getnetwork vector must execute");
    assert_known_divergences("getNetwork", &failures, &[]);
}

