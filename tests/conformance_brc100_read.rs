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

use std::collections::BTreeSet;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use async_trait::async_trait;
use bsv::auth::certificates::{MasterCertificate, VerifiableCertificate};
use bsv::primitives::hash::sha256d;
use bsv::primitives::private_key::PrivateKey;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::{
    AcquireCertificateArgs, AcquisitionProtocol, Certificate as SdkCertificate, CertificateType,
    KeyringRevealer, ListActionsArgs, ListOutputsArgs, ProveCertificateArgs, SerialNumber,
    WalletInterface,
};
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types as services_types;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::tables::{Transaction, TxLabelMap};
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

/// Vectors satisfied by writing raw storage rows instead of (or in addition
/// to) BRC-100 calls. Backend-specific: only the local backend can run these.
const STORAGE_SEEDED_VECTORS: &[&str] = &["wallet.brc100.listactions.14"];

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
    assert_eq!(
        lo.vectors.len(),
        144,
        "listoutputs count changed on refresh"
    );
    assert_eq!(lo.vectors.iter().filter(|v| v.skip).count(), 0);
    assert_eq!(
        lo.vectors
            .iter()
            .filter(|v| v.skip_reason.is_some())
            .count(),
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
    assert_eq!(
        pc.vectors.len(),
        8,
        "provecertificate count changed on refresh"
    );
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
            .build(
                &VectorIdentity {
                    root_key_hex: DEFAULT_ROOT,
                },
                chain,
            )
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
            Err(e) => failures.push(format!(
                "{}: expected network {want:?}, got error {e}",
                v.id
            )),
        }
    }

    assert_eq!(executed, 5, "every getnetwork vector must execute");
    assert_known_divergences("getNetwork", &failures, &[]);
}

// ---------------------------------------------------------------------------
// listOutputs — 144 vectors, all empty-wallet reads
// ---------------------------------------------------------------------------

/// 36 of the 144 vectors use `basket: "default"`. The Rust SDK's
/// `validate_basket_name` (bsv-sdk 0.3.4 `src/wallet/validation.rs`) rejects
/// the literal basket name "default" (alongside `basket`-suffix / `admin`-
/// prefix rules), while the TS reference (`@bsv/sdk` validateBasket) only
/// checks 1..300 bytes and the reference run returned success for every one
/// of these. The TS toolbox treats "default" as the ordinary change basket —
/// it is the basket every wallet actually owns — so the Rust-side rejection
/// makes the default basket unreadable through the BRC-100 surface. Verdict:
/// the Rust SDK validation is wrong (an invented rule the reference does not
/// share); fix belongs in bsv-sdk, not this crate. Each entry asserts the
/// observed rejection until that fix lands.
const KNOWN_LISTOUTPUTS_DIVERGENCES: &[&str] = &[
    // Observed: every entry fails with
    //   "invalid parameter: basket: must be not 'default'"
    // (ids 1..36 are exactly the basket="default" quadrant of the matrix).
    "wallet.brc100.listoutputs.1:",
    "wallet.brc100.listoutputs.2:",
    "wallet.brc100.listoutputs.3:",
    "wallet.brc100.listoutputs.4:",
    "wallet.brc100.listoutputs.5:",
    "wallet.brc100.listoutputs.6:",
    "wallet.brc100.listoutputs.7:",
    "wallet.brc100.listoutputs.8:",
    "wallet.brc100.listoutputs.9:",
    "wallet.brc100.listoutputs.10:",
    "wallet.brc100.listoutputs.11:",
    "wallet.brc100.listoutputs.12:",
    "wallet.brc100.listoutputs.13:",
    "wallet.brc100.listoutputs.14:",
    "wallet.brc100.listoutputs.15:",
    "wallet.brc100.listoutputs.16:",
    "wallet.brc100.listoutputs.17:",
    "wallet.brc100.listoutputs.18:",
    "wallet.brc100.listoutputs.19:",
    "wallet.brc100.listoutputs.20:",
    "wallet.brc100.listoutputs.21:",
    "wallet.brc100.listoutputs.22:",
    "wallet.brc100.listoutputs.23:",
    "wallet.brc100.listoutputs.24:",
    "wallet.brc100.listoutputs.25:",
    "wallet.brc100.listoutputs.26:",
    "wallet.brc100.listoutputs.27:",
    "wallet.brc100.listoutputs.28:",
    "wallet.brc100.listoutputs.29:",
    "wallet.brc100.listoutputs.30:",
    "wallet.brc100.listoutputs.31:",
    "wallet.brc100.listoutputs.32:",
    "wallet.brc100.listoutputs.33:",
    "wallet.brc100.listoutputs.34:",
    "wallet.brc100.listoutputs.35:",
    "wallet.brc100.listoutputs.36:",
];

#[tokio::test]
async fn listoutputs_conformance() {
    let f = load(LISTOUTPUTS, "wallet.brc100.listoutputs");
    let backend = LocalSqliteBackend;

    // One wallet for all 144 vectors: every call is a read against an empty
    // wallet, so no vector can perturb another.
    let built = backend
        .build(
            &VectorIdentity {
                root_key_hex: DEFAULT_ROOT,
            },
            Chain::Main,
        )
        .await;

    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;

        let args: ListOutputsArgs = match serde_json::from_value(v.input.args.clone()) {
            Ok(a) => a,
            Err(e) => {
                failures.push(format!("{}: args failed to deserialize: {e}", v.id));
                continue;
            }
        };

        let want_total = v.expected["totalOutputs"]
            .as_u64()
            .unwrap_or_else(|| panic!("{}: expected.totalOutputs missing", v.id));

        match built
            .wallet
            .list_outputs(args, v.input.originator.as_deref())
            .await
        {
            Ok(r) => {
                if u64::from(r.total_outputs) != want_total {
                    failures.push(format!(
                        "{}: totalOutputs expected {want_total}, got {}",
                        v.id, r.total_outputs
                    ));
                }
                if r.outputs.len() as u64 != want_total {
                    failures.push(format!(
                        "{}: outputs len expected {want_total}, got {}",
                        v.id,
                        r.outputs.len()
                    ));
                }
            }
            Err(e) => failures.push(format!(
                "{}: expected success (totalOutputs {want_total}), got error {e}",
                v.id
            )),
        }
    }

    assert_eq!(
        executed, 144,
        "every listoutputs vector must execute — no silent filtering"
    );
    assert_known_divergences("listOutputs", &failures, KNOWN_LISTOUTPUTS_DIVERGENCES);
}

// ---------------------------------------------------------------------------
// listActions — 16 vectors (15 empty-wallet reads + 1 storage-seeded)
// ---------------------------------------------------------------------------

/// Pinned listActions divergences:
///
/// - `.11` (`labels: []`): Rust `validate_list_actions_args` (bsv-sdk 0.3.4)
///   rejects an empty labels array; the TS reference (`@bsv/sdk`
///   validateListActionsArgs) maps `labels ?? []` through unvalidated and the
///   reference run returned `{totalActions: 0}`. In TS an empty label list is
///   the ordinary "no label filter" query. Verdict: the Rust SDK invented a
///   non-emptiness rule the reference does not share — fix belongs in
///   bsv-sdk. Observed: "invalid parameter: labels: must be non-empty".
///
/// - `.14` labels: the vector's args omit `includeLabels`, and BOTH
///   implementations only attach labels when it is set (TS
///   listActionsKnex.ts:230 gates on `vargs.includeLabels`; Rust storage
///   list_actions does the same — every other field of the seeded action
///   matches). The corpus's hand-written expectation (`labels: ["payment"]`,
///   never executed by the reference — skip_reason on the vector) exceeds
///   both implementations. Verdict: corpus bug, flag upstream. Observed:
///   `actions[0].labels expected ["payment"], got []`.
const KNOWN_LISTACTIONS_DIVERGENCES: &[&str] = &[
    "wallet.brc100.listactions.11:",
    "wallet.brc100.listactions.14: actions[0].labels",
];

#[tokio::test]
async fn listactions_conformance() {
    let f = load(LISTACTIONS, "wallet.brc100.listactions");
    let backend = LocalSqliteBackend;

    // Shared wallet for the fresh-empty vectors (reads on an empty wallet
    // cannot interfere); the seeded vector gets its own.
    let fresh = backend
        .build(
            &VectorIdentity {
                root_key_hex: DEFAULT_ROOT,
            },
            Chain::Main,
        )
        .await;

    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;

        let built;
        let wallet = if v.id == "wallet.brc100.listactions.14" {
            built = backend
                .build(
                    &VectorIdentity {
                        root_key_hex: DEFAULT_ROOT,
                    },
                    Chain::Main,
                )
                .await;
            seed_listactions_14(&built).await;
            &built.wallet
        } else {
            &fresh.wallet
        };

        let args: ListActionsArgs = match serde_json::from_value(v.input.args.clone()) {
            Ok(a) => a,
            Err(e) => {
                failures.push(format!("{}: args failed to deserialize: {e}", v.id));
                continue;
            }
        };

        let want_total = v.expected["totalActions"]
            .as_u64()
            .unwrap_or_else(|| panic!("{}: expected.totalActions missing", v.id));

        match wallet
            .list_actions(args, v.input.originator.as_deref())
            .await
        {
            Ok(r) => {
                if u64::from(r.total_actions) != want_total {
                    failures.push(format!(
                        "{}: totalActions expected {want_total}, got {}",
                        v.id, r.total_actions
                    ));
                    continue;
                }
                if r.actions.len() as u64 != want_total {
                    failures.push(format!(
                        "{}: actions len expected {want_total}, got {}",
                        v.id,
                        r.actions.len()
                    ));
                    continue;
                }
                let want_actions = v.expected["actions"].as_array().expect("expected.actions");
                for (i, want) in want_actions.iter().enumerate() {
                    compare_action(&v.id, i, want, &r.actions[i], &mut failures);
                }
            }
            Err(e) => failures.push(format!(
                "{}: expected success (totalActions {want_total}), got error {e}",
                v.id
            )),
        }
    }

    assert_eq!(executed, 16, "every listactions vector must execute");
    assert_known_divergences("listActions", &failures, KNOWN_LISTACTIONS_DIVERGENCES);
}

/// Seed the exact action wallet.brc100.listactions.14 expects: one completed
/// outgoing transaction labeled "payment". Raw storage rows (see
/// STORAGE_SEEDED_VECTORS): the write-side path (createAction) is a different
/// conformance channel and would drag services/funding into a read test.
async fn seed_listactions_14(built: &BuiltWallet) {
    assert!(
        STORAGE_SEEDED_VECTORS.contains(&"wallet.brc100.listactions.14"),
        "storage-seeded vectors must be declared in STORAGE_SEEDED_VECTORS"
    );
    let storage = built
        .seed_storage
        .as_ref()
        .expect("storage-seeded vector requires the local backend");
    let now = chrono::Utc::now().naive_utc();

    let (user, _) =
        StorageReaderWriter::find_or_insert_user(storage.as_ref(), &built.identity_key, None)
            .await
            .expect("user");

    let tx_id = StorageReaderWriter::insert_transaction(
        storage.as_ref(),
        &Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id: user.user_id,
            proven_tx_id: None,
            status: TransactionStatus::Completed,
            reference: "brc100-listactions-14".to_string(),
            is_outgoing: true,
            satoshis: 1000,
            description: "test payment".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: Some("a".repeat(64)),
            input_beef: None,
            raw_tx: None,
        },
        None,
    )
    .await
    .expect("seed tx");

    let label = storage
        .find_or_insert_tx_label(user.user_id, "payment", None)
        .await
        .expect("label");
    storage
        .insert_tx_label_map(
            &TxLabelMap {
                created_at: now,
                updated_at: now,
                transaction_id: tx_id,
                tx_label_id: label.tx_label_id,
                is_deleted: false,
            },
            None,
        )
        .await
        .expect("label map");
}

/// Compare one returned action against the vector's expected object,
/// field by field, recording divergences by name.
fn compare_action(
    vector_id: &str,
    idx: usize,
    want: &Value,
    got: &bsv::wallet::interfaces::Action,
    failures: &mut Vec<String>,
) {
    let mut diff = |field: &str, want_s: String, got_s: String| {
        if want_s != got_s {
            failures.push(format!(
                "{vector_id}: actions[{idx}].{field} expected {want_s}, got {got_s}"
            ));
        }
    };
    if let Some(w) = want["txid"].as_str() {
        diff("txid", w.to_string(), got.txid.clone());
    }
    if let Some(w) = want["satoshis"].as_i64() {
        diff("satoshis", w.to_string(), got.satoshis.to_string());
    }
    if let Some(w) = want["status"].as_str() {
        diff("status", w.to_string(), got.status.as_str().to_string());
    }
    if let Some(w) = want["isOutgoing"].as_bool() {
        diff("isOutgoing", w.to_string(), got.is_outgoing.to_string());
    }
    if let Some(w) = want["description"].as_str() {
        diff("description", w.to_string(), got.description.clone());
    }
    if let Some(w) = want["version"].as_u64() {
        diff("version", w.to_string(), u64::from(got.version).to_string());
    }
    if let Some(w) = want["lockTime"].as_u64() {
        diff(
            "lockTime",
            w.to_string(),
            u64::from(got.lock_time).to_string(),
        );
    }
    if let Some(w) = want["labels"].as_array() {
        let want_labels: Vec<&str> = w.iter().filter_map(|x| x.as_str()).collect();
        let got_labels: Vec<&str> = got
            .labels
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|s| s.as_str())
            .collect();
        diff(
            "labels",
            format!("{want_labels:?}"),
            format!("{got_labels:?}"),
        );
    }
    if let Some(w) = want["inputs"].as_array() {
        diff(
            "inputs.len",
            w.len().to_string(),
            got.inputs.as_deref().unwrap_or(&[]).len().to_string(),
        );
    }
    if let Some(w) = want["outputs"].as_array() {
        diff(
            "outputs.len",
            w.len().to_string(),
            got.outputs.as_deref().unwrap_or(&[]).len().to_string(),
        );
    }
}

// ---------------------------------------------------------------------------
// proveCertificate — 8 vectors
// ---------------------------------------------------------------------------

/// Pinned proveCertificate divergences:
///
/// - Wire pins (ids 1,2,3,4,6,7,8): every corpus vector's certificate `type`
///   decodes to 34 bytes; `CertificateType([u8; 32])` rejects it during args
///   deserialization, so the 7 success vectors cannot execute verbatim. The
///   TS reference validates the type only as base64 (no length rule) and
///   passes it through; BRC-52 fixes certificate types at 32 bytes. Verdict:
///   corpus placeholder violates the spec the reference merely fails to
///   enforce — corpus bug, flag upstream. (.5 also fails at the wire but
///   expects an error, so it is not pinned.)
///
const KNOWN_PROVECERTIFICATE_DIVERGENCES: &[&str] = &[
    "wallet.brc100.provecertificate.1:",
    "wallet.brc100.provecertificate.2:",
    "wallet.brc100.provecertificate.3:",
    "wallet.brc100.provecertificate.4:",
    "wallet.brc100.provecertificate.6:",
    "wallet.brc100.provecertificate.7:",
    "wallet.brc100.provecertificate.8:",
];

/// The 32-byte certificate type used by the mechanism leg: the corpus type
/// truncated to the only length `CertificateType` can represent.
fn representable_cert_type(corpus_type_b64: &str) -> CertificateType {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(corpus_type_b64)
        .expect("corpus cert type is base64");
    assert_eq!(
        decoded.len(),
        34,
        "corpus cert type length changed — revisit the wire-divergence pins"
    );
    let mut t = [0u8; 32];
    t.copy_from_slice(&decoded[..32]);
    CertificateType(t)
}

#[tokio::test]
async fn provecertificate_conformance() {
    let f = load(PROVECERTIFICATE, "wallet.brc100.provecertificate");
    let backend = LocalSqliteBackend;
    let mut failures: Vec<String> = Vec::new();
    let mut executed: BTreeSet<String> = BTreeSet::new();

    // ---- Leg 1 — strict wire, corpus-verbatim args.
    //
    // Every vector's certificate `type` decodes to 34 bytes (0x00 "acerttype"
    // zero-padded). The TS reference validates it only as base64
    // (validateOptionalBase64String — no length rule), so 34 bytes flow
    // through; BRC-52 fixes certificate types at 32 bytes and this SDK's
    // `CertificateType([u8; 32])` enforces that, so `ProveCertificateArgs`
    // rejects the corpus args at deserialization. The corpus placeholder
    // violates the spec the reference merely fails to enforce — corpus bug,
    // flag upstream — but it is a real wire divergence from the reference
    // and each success vector is pinned below.
    //
    // wallet.brc100.provecertificate.5 expects an error and gets one at the
    // wire; the reference's own error came from the missing certificate (it
    // ran fresh — skip_reason on all others), not from the nonexistent field
    // the vector describes. The mechanism leg pins the field-subset error.
    for v in &f.vectors {
        executed.insert(v.id.clone());
        let expect_error = v.expected["error"].as_bool().unwrap_or(false);
        match serde_json::from_value::<ProveCertificateArgs>(v.input.args.clone()) {
            Err(_) if expect_error => {} // rejected at the wire — an error outcome
            Err(e) => failures.push(format!(
                "{}: args rejected at the wire (corpus 34-byte certificate type vs [u8; 32]): {e}",
                v.id
            )),
            Ok(args) => {
                // If a corpus refresh makes the args representable, run them
                // against a fresh wallet exactly as the reference did.
                let built = backend
                    .build(
                        &VectorIdentity {
                            root_key_hex: SUBJECT_ROOT,
                        },
                        Chain::Main,
                    )
                    .await;
                let outcome = built
                    .wallet
                    .prove_certificate(args, v.input.originator.as_deref())
                    .await;
                if expect_error && outcome.is_ok() {
                    failures.push(format!("{}: expected error, got success", v.id));
                }
            }
        }
    }

    // ---- Leg 2 — mechanism, seeded. The corpus's intent for the 7
    // skip_reason vectors is "wallet holds this certificate; prove reveals
    // these fields". The wallet is seeded through acquireCertificate (a
    // BRC-100 call — not backend-specific) with the exact corpus certificate
    // (serial, certifier, subject, plaintext fields, placeholder signature
    // and revocation outpoint) except for the one thing Rust cannot
    // represent: the type is truncated to 32 bytes, in the vector args and
    // the stored certificate alike. Failures here carry the "mechanism "
    // prefix so the wire pins above cannot mask them.
    //
    // The corpus's expected keyringForVerifier VALUES are literal placeholder
    // fills ("AAAA…", "BBBB…", "CCCC…"): the reference dispatcher never
    // executed these vectors, and a keyring entry is a fresh symmetric-key
    // encryption — nondeterministic, so no implementation can reproduce a
    // pinned byte string. What IS assertable: the revealed field SET must
    // equal the expected keyring's key set, and the keyring must actually
    // work — each entry must decrypt (as the verifier) back to the vector's
    // plaintext field value. Both are asserted for every success vector.
    {
        let built = backend
            .build(
                &VectorIdentity {
                    root_key_hex: SUBJECT_ROOT,
                },
                Chain::Main,
            )
            .await;
        let seeded = seed_certificate(&built, &f).await;

        let verifier_wallet = ProtoWallet::new(PrivateKey::from_hex(SUBJECT_ROOT).unwrap());

        for v in &f.vectors {
            let mut args_json = v.input.args.clone();
            args_json["certificate"]["type"] = Value::String(seeded.cert_type_b64.clone());
            let args: ProveCertificateArgs = serde_json::from_value(args_json)
                .unwrap_or_else(|e| panic!("{}: mechanism args must deserialize: {e}", v.id));

            let expect_error = v.expected["error"].as_bool().unwrap_or(false);
            let outcome = built
                .wallet
                .prove_certificate(args, v.input.originator.as_deref())
                .await;

            if expect_error {
                if let Ok(r) = outcome {
                    failures.push(format!(
                        "mechanism {}: expected error, got keyring {:?}",
                        v.id,
                        r.keyring_for_verifier.keys().collect::<Vec<_>>()
                    ));
                }
                continue;
            }

            let want_fields: BTreeSet<String> = v.expected["keyringForVerifier"]
                .as_object()
                .unwrap_or_else(|| panic!("{}: expected.keyringForVerifier missing", v.id))
                .keys()
                .cloned()
                .collect();

            let r = match outcome {
                Ok(r) => r,
                Err(e) => {
                    failures.push(format!(
                        "mechanism {}: expected keyring for {want_fields:?}, got error {e}",
                        v.id
                    ));
                    continue;
                }
            };

            let got_fields: BTreeSet<String> = r.keyring_for_verifier.keys().cloned().collect();
            if got_fields != want_fields {
                failures.push(format!(
                    "mechanism {}: revealed field set expected {want_fields:?}, got {got_fields:?}",
                    v.id
                ));
                continue;
            }

            // Round-trip: the produced keyring must decrypt, as the verifier,
            // back to the plaintext fields the vector's certificate carries.
            let vc = VerifiableCertificate::new(
                seeded.sdk_certificate.clone(),
                r.keyring_for_verifier.clone(),
            );
            match vc.decrypt_fields(&verifier_wallet).await {
                Ok(decrypted) => {
                    for field in &want_fields {
                        let want_plain = seeded.plaintext_fields.get(field).map(String::as_str);
                        let got_plain = decrypted.get(field).map(String::as_str);
                        if want_plain != got_plain {
                            failures.push(format!(
                                "mechanism {}: keyring field {field:?} decrypts to {got_plain:?}, vector plaintext is {want_plain:?}",
                                v.id
                            ));
                        }
                    }
                }
                Err(e) => failures.push(format!(
                    "mechanism {}: verifier could not decrypt the returned keyring: {e}",
                    v.id
                )),
            }
        }
    }

    let all_ids: BTreeSet<String> = f.vectors.iter().map(|v| v.id.clone()).collect();
    assert_eq!(
        executed, all_ids,
        "every provecertificate vector must execute the strict leg"
    );
    assert_known_divergences(
        "proveCertificate",
        &failures,
        KNOWN_PROVECERTIFICATE_DIVERGENCES,
    );
}

struct SeededCertificate {
    /// The stored certificate with ENCRYPTED field values, as a verifier
    /// receives it.
    sdk_certificate: SdkCertificate,
    /// The plaintext field values the vectors carry.
    plaintext_fields: IndexMap<String, String>,
    /// Base64 of the 32-byte type actually stored (corpus type truncated).
    cert_type_b64: String,
}

/// Issue the certificate the proveCertificate vectors describe (certifier =
/// key 1, subject = key 2, serial = 32 zero bytes, plaintext fields,
/// placeholder signature and revocation outpoint all from the vector args;
/// type truncated to the representable 32 bytes) and store it in the wallet
/// via BRC-100 acquireCertificate (direct protocol).
async fn seed_certificate(built: &BuiltWallet, f: &VectorFile) -> SeededCertificate {
    use base64::Engine as _;

    // Vector 1 carries the full certificate every vector shares.
    let v1 = &f.vectors[0].input.args;
    let cert_type = representable_cert_type(v1["certificate"]["type"].as_str().expect("type"));
    let cert_type_b64 = base64::engine::general_purpose::STANDARD.encode(cert_type.0);
    let serial: SerialNumber =
        serde_json::from_value(v1["certificate"]["serialNumber"].clone()).expect("serial");
    let plaintext_fields: IndexMap<String, String> =
        serde_json::from_value(v1["certificate"]["fields"].clone()).expect("fields");
    let vector_signature = hex::decode(v1["certificate"]["signature"].as_str().expect("signature"))
        .expect("signature hex");
    let vector_revocation = v1["certificate"]["revocationOutpoint"]
        .as_str()
        .expect("revocationOutpoint")
        .to_string();

    let certifier_key = PrivateKey::from_hex(CERTIFIER_ROOT).unwrap();
    let certifier_pub = certifier_key.to_public_key();
    let certifier_wallet = ProtoWallet::new(certifier_key);
    let subject_pub = PrivateKey::from_hex(SUBJECT_ROOT).unwrap().to_public_key();

    // Issuance produces the only parts that must be cryptographically real
    // for proveCertificate to work: encrypted field values plus the master
    // keyring encrypted certifier→subject. Type/signature/revocation are
    // stored as the corpus states them (storage does not verify them).
    let master = MasterCertificate::issue_certificate_for_subject(
        &cert_type,
        &subject_pub,
        plaintext_fields.clone(),
        &certifier_wallet,
        bsv::auth::certificates::master::default_get_revocation_outpoint,
        Some(serial.clone()),
    )
    .await
    .expect("issue certificate");

    let encrypted_fields = master
        .certificate
        .fields
        .clone()
        .expect("issued cert has fields");

    built
        .wallet
        .acquire_certificate(
            AcquireCertificateArgs {
                cert_type,
                certifier: certifier_pub,
                acquisition_protocol: AcquisitionProtocol::Direct,
                fields: encrypted_fields.clone(),
                serial_number: Some(serial.clone()),
                revocation_outpoint: Some(vector_revocation),
                signature: Some(vector_signature),
                certifier_url: None,
                keyring_revealer: Some(KeyringRevealer::Certifier),
                keyring_for_subject: Some(master.master_keyring.clone()),
                privileged: false,
                privileged_reason: None,
            },
            None,
        )
        .await
        .expect("acquire seeded certificate");

    SeededCertificate {
        sdk_certificate: master.certificate.clone(),
        plaintext_fields,
        cert_type_b64,
    }
}
