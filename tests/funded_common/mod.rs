//! Shared harness for the funded BRC-100 conformance vectors.
//!
//! Both the recorder (`tests/record_funded_vectors.rs`, network, spends real
//! satoshis, `#[ignore]`d) and the offline replayer
//! (`tests/conformance_brc100_funded.rs`, hermetic) drive vectors through the
//! functions in this module, so the execution path that produced a recorded
//! `expected` block is the same path that replays it. A vector run is:
//!
//! 1. fresh in-memory wallet from `input.root_key`
//! 2. internalize the funding payments (real mainnet BEEFs) in order
//! 3. seed the deterministic entropy stream from `input.entropy_seed`
//! 4. run the setup actions (signAction vectors only), then `input.args`
//! 5. capture txid / tx bytes / change / errors
//!
//! Determinism comes from three facts: change derivation entropy is seeded
//! (`conformance_entropy`), ECDSA nonces are RFC 6979, and UTXO selection is a
//! pure function of storage state, which steps 1–2 fix exactly.

// Shared across three test targets; each target uses a different subset.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::public_key::PublicKey;
use bsv::transaction::beef::Beef;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::TransactionError;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionResult, InternalizeActionArgs, InternalizeOutput, Payment,
    SignActionArgs, SignActionResult, WalletInterface,
};
use bsv::wallet::types::BooleanDefaultTrue;

use bsv_wallet_toolbox::error::{WalletError, WalletResult};
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::utility::conformance_entropy;
use bsv_wallet_toolbox::wallet::setup::{SetupWallet, WalletBuilder};

// ---------------------------------------------------------------------------
// Vector file schema
// ---------------------------------------------------------------------------

/// A `*-funded.json` vector file. Same top-level keys as the vendored
/// upstream files, plus the recording provenance, the funding fixtures, and
/// the pinned merkle roots that make offline replay self-contained.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FundedFile {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub id: String,
    pub name: String,
    pub brc: Vec<String>,
    pub version: String,
    pub reference_impl: String,
    pub parity_class: String,
    pub recording: serde_json::Value,
    /// Funding fixture sets, referenced by `input.funding_set`.
    pub funding_sets: BTreeMap<String, FundingSet>,
    /// height -> merkle root for every bump in every fixture BEEF, verified
    /// against mainnet at recording time. The offline chain tracker answers
    /// from this map and nothing else.
    pub merkle_roots: BTreeMap<u32, String>,
    pub vectors: Vec<FundedVector>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FundingSet {
    pub payments: Vec<FundingPayment>,
}

/// One real mainnet payment, carried as the full internalize fixture.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FundingPayment {
    /// AtomicBEEF for the payment transaction, hex.
    pub beef: String,
    pub output_index: u32,
    /// BRC-29 derivation prefix (base64 string, as carried on the wire).
    pub derivation_prefix: String,
    pub derivation_suffix: String,
    /// Sender identity key, compressed DER hex.
    pub sender_identity_key: String,
    pub satoshis: u64,
    pub txid: String,
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FundedVector {
    pub id: String,
    pub description: String,
    pub input: FundedInput,
    pub expected: serde_json::Value,
    pub broadcast: BroadcastRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The upstream vector this one records reality for, with its fabricated
    /// expected values kept legible next to the recorded ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FundedInput {
    pub root_key: String,
    pub funding_set: String,
    pub entropy_seed: String,
    /// Precursor actions (signAction vectors): each is a createAction run
    /// before `args`, with its observed reference/txid pinned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<SetupStep>,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetupStep {
    pub create_args: serde_json::Value,
    /// Reference string returned in `signableTransaction`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Txid, for fully-processed setup actions (noSend precursors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BroadcastRecord {
    pub sent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Hex helpers
// ---------------------------------------------------------------------------

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Offline services: pinned chain tracker + network tripwire
// ---------------------------------------------------------------------------

/// Chain tracker that answers only from merkle roots pinned at recording
/// time. An unknown height or a mismatched root fails validation — there is
/// no fallback, which is exactly the property that makes a passing replay
/// meaningful.
pub struct PinnedChainTracker {
    pub roots: BTreeMap<u32, String>,
}

#[async_trait]
impl ChainTracker for PinnedChainTracker {
    async fn is_valid_root_for_height(
        &self,
        root: &str,
        height: u32,
    ) -> Result<bool, TransactionError> {
        Ok(self.roots.get(&height).map(|r| r == root).unwrap_or(false))
    }
}

/// How `post_beef` behaves during a vector run.
pub enum PostMode {
    /// Any broadcast attempt is a bug in the replay: panic.
    Tripwire,
    /// Replay the network response recorded when the vector was recorded
    /// (vectors whose recorded behavior includes an inline broadcast).
    Recorded(Vec<types::PostBeefResult>),
}

/// WalletServices for offline replay. The chain tracker answers from pinned
/// roots; every other network-touching method panics, so a replay that
/// reaches for the network fails the test rather than quietly fetching.
pub struct FixtureServices {
    pub roots: BTreeMap<u32, String>,
    pub post: PostMode,
}

fn tripwire(method: &str) -> ! {
    panic!("NETWORK TRIPWIRE: offline replay called WalletServices::{method}");
}

#[async_trait]
impl WalletServices for FixtureServices {
    fn chain(&self) -> Chain {
        Chain::Main
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        Ok(Box::new(PinnedChainTracker {
            roots: self.roots.clone(),
        }))
    }

    async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> types::GetMerklePathResult {
        tripwire("get_merkle_path")
    }

    async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> types::GetRawTxResult {
        tripwire("get_raw_tx")
    }

    async fn post_beef(&self, _beef: &[u8], txids: &[String]) -> Vec<types::PostBeefResult> {
        match &self.post {
            PostMode::Tripwire => tripwire("post_beef"),
            PostMode::Recorded(results) => {
                let _ = txids;
                results.clone()
            }
        }
    }

    async fn get_utxo_status(
        &self,
        _output: &str,
        _output_format: Option<types::GetUtxoStatusOutputFormat>,
        _outpoint: Option<&str>,
        _use_next: bool,
    ) -> types::GetUtxoStatusResult {
        tripwire("get_utxo_status")
    }

    async fn get_status_for_txids(
        &self,
        _txids: &[String],
        _use_next: bool,
    ) -> types::GetStatusForTxidsResult {
        tripwire("get_status_for_txids")
    }

    async fn get_script_hash_history(
        &self,
        _hash: &str,
        _use_next: bool,
    ) -> types::GetScriptHashHistoryResult {
        tripwire("get_script_hash_history")
    }

    async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
        tripwire("hash_to_header")
    }

    async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
        tripwire("get_header_for_height")
    }

    async fn get_height(&self) -> WalletResult<u32> {
        tripwire("get_height")
    }

    async fn n_lock_time_is_final(&self, _input: types::NLockTimeInput) -> WalletResult<bool> {
        tripwire("n_lock_time_is_final")
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
        tripwire("get_bsv_exchange_rate")
    }

    async fn get_fiat_exchange_rate(
        &self,
        _currency: &str,
        _base: Option<&str>,
    ) -> WalletResult<f64> {
        tripwire("get_fiat_exchange_rate")
    }

    async fn get_fiat_exchange_rates(
        &self,
        _target_currencies: &[String],
    ) -> WalletResult<types::FiatExchangeRates> {
        tripwire("get_fiat_exchange_rates")
    }

    fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
        types::ServicesCallHistory { services: vec![] }
    }

    async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
        tripwire("get_beef_for_txid")
    }

    fn hash_output_script(&self, _script: &[u8]) -> String {
        String::new()
    }

    async fn is_utxo(&self, _locking_script: &[u8], _txid: &str, _vout: u32) -> WalletResult<bool> {
        tripwire("is_utxo")
    }
}

// ---------------------------------------------------------------------------
// Vector execution
// ---------------------------------------------------------------------------

/// Serializes seeded-entropy sections. The conformance entropy override is
/// process-global and consumed in call order, so two vector runs (or test
/// threads) interleaving would corrupt each other's streams. Hold this for
/// the whole seeded section.
pub static ENTROPY_SESSION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Build a fresh in-memory wallet for a vector run.
pub async fn build_vector_wallet(
    root_key_hex: &str,
    services: Arc<dyn WalletServices>,
) -> WalletResult<SetupWallet> {
    let root = PrivateKey::from_hex(root_key_hex)
        .map_err(|e| WalletError::Internal(format!("bad root key: {e:?}")))?;
    let setup = WalletBuilder::new()
        .chain(Chain::Main)
        .root_key(root)
        .with_sqlite_memory()
        .with_services(services)
        .without_monitor()
        .build()
        .await?;
    setup
        .storage
        .find_or_insert_user(&setup.identity_key)
        .await?;
    Ok(setup)
}

/// Internalize one funding payment fixture into the wallet.
pub async fn internalize_funding(
    setup: &SetupWallet,
    payment: &FundingPayment,
) -> Result<(), String> {
    use base64::Engine as _;
    let sender = PublicKey::from_string(&payment.sender_identity_key)
        .map_err(|e| format!("bad sender identity key: {e:?}"))?;
    // The fixture carries the derivation prefix/suffix as the base64 STRINGS
    // the BRC-29 lock was built with. `Payment` wants the raw bytes — the
    // storage layer re-encodes them to base64 text, and that stored string is
    // what the signer feeds back into the BRC-29 template. Passing the
    // string's UTF-8 bytes here would double-encode and derive a key that
    // cannot spend the output (internalize rejects the mismatch).
    let prefix = base64::engine::general_purpose::STANDARD
        .decode(&payment.derivation_prefix)
        .map_err(|e| format!("derivation_prefix not base64: {e}"))?;
    let suffix = base64::engine::general_purpose::STANDARD
        .decode(&payment.derivation_suffix)
        .map_err(|e| format!("derivation_suffix not base64: {e}"))?;
    setup
        .wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: from_hex(&payment.beef),
                description: payment.description.clone(),
                labels: vec!["funded-conformance".to_string()],
                seek_permission: BooleanDefaultTrue(Some(false)),
                outputs: vec![InternalizeOutput::WalletPayment {
                    output_index: payment.output_index,
                    payment: Payment {
                        derivation_prefix: prefix,
                        derivation_suffix: suffix,
                        sender_identity_key: sender,
                    },
                }],
            },
            None,
        )
        .await
        .map_err(|e| format!("internalize {} failed: {e}", payment.txid))?;
    Ok(())
}

/// Informational change-output record captured per created transaction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangeInfo {
    pub vout: u32,
    pub satoshis: u64,
    pub derivation_prefix: String,
    pub derivation_suffix: String,
}

/// Query the change outputs the wallet created for `txid`.
pub async fn change_outputs_for_txid(
    setup: &SetupWallet,
    txid: &str,
) -> WalletResult<Vec<ChangeInfo>> {
    use bsv_wallet_toolbox::storage::find_args::{FindOutputsArgs, OutputPartial};
    let (user, _) = setup
        .storage
        .find_or_insert_user(&setup.identity_key)
        .await?;
    let outputs = setup
        .storage
        .find_outputs(&FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user.user_id),
                txid: Some(txid.to_string()),
                change: Some(true),
                ..Default::default()
            },
            ..Default::default()
        })
        .await?;
    let mut infos: Vec<ChangeInfo> = outputs
        .iter()
        .map(|o| ChangeInfo {
            vout: o.vout as u32,
            satoshis: o.satoshis as u64,
            derivation_prefix: o.derivation_prefix.clone().unwrap_or_default(),
            derivation_suffix: o.derivation_suffix.clone().unwrap_or_default(),
        })
        .collect();
    infos.sort_by_key(|c| c.vout);
    Ok(infos)
}

/// The observable outcome of a vector's final action, in the exact shape
/// stored under `expected`. Byte-level fields are hex strings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordedOutcome {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// Full AtomicBEEF bytes returned by the wallet, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
    /// The raw signed transaction extracted from the BEEF, hex (readability;
    /// the byte-for-byte assertion is on `tx`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_tx: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_send_change: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub send_with_results: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signable_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub change: Vec<ChangeInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn raw_tx_from_beef(beef_bytes: &[u8], txid: &str) -> Option<String> {
    let mut cursor = std::io::Cursor::new(beef_bytes);
    let beef = Beef::from_binary(&mut cursor).ok()?;
    let btx = beef.find_txid(txid)?;
    let tx = btx.tx.as_ref()?;
    let mut buf = Vec::new();
    tx.to_binary(&mut buf).ok()?;
    Some(to_hex(&buf))
}

async fn outcome_from_create(
    setup: &SetupWallet,
    result: Result<CreateActionResult, impl std::fmt::Display>,
) -> RecordedOutcome {
    match result {
        Ok(r) => {
            let txid = r.txid.clone();
            let tx_hex = r.tx.as_ref().map(|b| to_hex(b));
            let raw_tx = match (&r.tx, &txid) {
                (Some(b), Some(t)) => raw_tx_from_beef(b, t),
                _ => None,
            };
            let change = match &txid {
                Some(t) => change_outputs_for_txid(setup, t).await.unwrap_or_default(),
                None => vec![],
            };
            RecordedOutcome {
                status: "success".to_string(),
                txid,
                tx: tx_hex,
                raw_tx,
                no_send_change: r.no_send_change.clone(),
                send_with_results: r
                    .send_with_results
                    .iter()
                    .map(|s| serde_json::to_value(s).unwrap_or_default())
                    .collect(),
                signable_reference: r.signable_transaction.as_ref().map(|s| {
                    // The result carries RAW reference bytes; the storage key
                    // (and the recorded corpus value) is the base64 text.
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&s.reference)
                }),
                change,
                error: None,
                message: None,
            }
        }
        Err(e) => RecordedOutcome {
            status: "error".to_string(),
            txid: None,
            tx: None,
            raw_tx: None,
            no_send_change: vec![],
            send_with_results: vec![],
            signable_reference: None,
            change: vec![],
            error: Some(true),
            message: Some(e.to_string()),
        },
    }
}

async fn outcome_from_sign(
    setup: &SetupWallet,
    result: Result<SignActionResult, impl std::fmt::Display>,
) -> RecordedOutcome {
    match result {
        Ok(r) => {
            let txid = r.txid.clone();
            let tx_hex = r.tx.as_ref().map(|b| to_hex(b));
            let raw_tx = match (&r.tx, &txid) {
                (Some(b), Some(t)) => raw_tx_from_beef(b, t),
                _ => None,
            };
            let change = match &txid {
                Some(t) => change_outputs_for_txid(setup, t).await.unwrap_or_default(),
                None => vec![],
            };
            RecordedOutcome {
                status: "success".to_string(),
                txid,
                tx: tx_hex,
                raw_tx,
                no_send_change: vec![],
                send_with_results: r
                    .send_with_results
                    .iter()
                    .map(|s| serde_json::to_value(s).unwrap_or_default())
                    .collect(),
                signable_reference: None,
                change,
                error: None,
                message: None,
            }
        }
        Err(e) => RecordedOutcome {
            status: "error".to_string(),
            txid: None,
            tx: None,
            raw_tx: None,
            no_send_change: vec![],
            send_with_results: vec![],
            signable_reference: None,
            change: vec![],
            error: Some(true),
            message: Some(e.to_string()),
        },
    }
}

/// Everything a createAction vector run produces.
#[derive(Debug)]
pub struct CreateRunResult {
    pub outcome: RecordedOutcome,
}

/// Run a createAction vector: fresh wallet, internalize funding, seed
/// entropy, execute args.
pub async fn run_create_vector(
    services: Arc<dyn WalletServices>,
    root_key_hex: &str,
    entropy_seed: &str,
    funding: &[FundingPayment],
    args_json: &serde_json::Value,
) -> Result<CreateRunResult, String> {
    let _entropy_guard = ENTROPY_SESSION.lock().await;
    let setup = build_vector_wallet(root_key_hex, services)
        .await
        .map_err(|e| format!("wallet build failed: {e}"))?;
    for p in funding {
        internalize_funding(&setup, p).await?;
    }

    conformance_entropy::set_conformance_entropy(entropy_seed);
    let parsed: Result<CreateActionArgs, _> = serde_json::from_value(args_json.clone());
    let outcome = match parsed {
        Ok(args) => {
            let result = setup.wallet.create_action(args, None).await;
            outcome_from_create(&setup, result).await
        }
        Err(e) => RecordedOutcome {
            status: "error".to_string(),
            txid: None,
            tx: None,
            raw_tx: None,
            no_send_change: vec![],
            send_with_results: vec![],
            signable_reference: None,
            change: vec![],
            error: Some(true),
            message: Some(format!("args failed to deserialize: {e}")),
        },
    };
    conformance_entropy::clear_conformance_entropy();

    Ok(CreateRunResult { outcome })
}

/// Everything a signAction vector run produces.
#[derive(Debug)]
pub struct SignRunResult {
    pub setup_outcomes: Vec<RecordedOutcome>,
    pub outcome: RecordedOutcome,
}

/// Run a signAction vector: fresh wallet, funding, seeded entropy, setup
/// createActions, then the signAction args.
pub async fn run_sign_vector(
    services: Arc<dyn WalletServices>,
    root_key_hex: &str,
    entropy_seed: &str,
    funding: &[FundingPayment],
    setup_steps: &[serde_json::Value],
    args_json: &serde_json::Value,
) -> Result<SignRunResult, String> {
    let _entropy_guard = ENTROPY_SESSION.lock().await;
    let setup = build_vector_wallet(root_key_hex, services)
        .await
        .map_err(|e| format!("wallet build failed: {e}"))?;
    for p in funding {
        internalize_funding(&setup, p).await?;
    }

    conformance_entropy::set_conformance_entropy(entropy_seed);
    let mut setup_outcomes = Vec::new();
    for (i, step_args) in setup_steps.iter().enumerate() {
        let args: CreateActionArgs = match serde_json::from_value(step_args.clone()) {
            Ok(a) => a,
            Err(e) => {
                conformance_entropy::clear_conformance_entropy();
                return Err(format!("setup[{i}] args failed to deserialize: {e}"));
            }
        };
        let result = setup.wallet.create_action(args, None).await;
        let outcome = outcome_from_create(&setup, result).await;
        if outcome.status == "error" {
            let msg = outcome.message.clone().unwrap_or_default();
            conformance_entropy::clear_conformance_entropy();
            return Err(format!("setup[{i}] createAction failed: {msg}"));
        }
        setup_outcomes.push(outcome);
    }

    let parsed: Result<SignActionArgs, _> = serde_json::from_value(args_json.clone());
    let outcome = match parsed {
        Ok(args) => {
            let result = setup.wallet.sign_action(args, None).await;
            outcome_from_sign(&setup, result).await
        }
        Err(e) => RecordedOutcome {
            status: "error".to_string(),
            txid: None,
            tx: None,
            raw_tx: None,
            no_send_change: vec![],
            send_with_results: vec![],
            signable_reference: None,
            change: vec![],
            error: Some(true),
            message: Some(format!("args failed to deserialize: {e}")),
        },
    };
    conformance_entropy::clear_conformance_entropy();

    Ok(SignRunResult {
        setup_outcomes,
        outcome,
    })
}

// ---------------------------------------------------------------------------
// Caller-input signing (signAction vectors)
// ---------------------------------------------------------------------------

/// P2PKH locking script for a key.
pub fn p2pkh_lock(key: &PrivateKey) -> Vec<u8> {
    use bsv::script::templates::p2pkh::P2PKH;
    use bsv::script::templates::ScriptTemplateLock;
    let pubkey = key.to_public_key();
    let hash_vec = pubkey.to_hash();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&hash_vec);
    P2PKH::from_public_key_hash(hash)
        .lock()
        .expect("p2pkh lock")
        .to_binary()
}

/// Compute the real P2PKH unlocking script for caller input `vin` of the
/// signable transaction inside `signable_beef` (RFC 6979, so deterministic).
pub async fn caller_unlock_script(
    signable_beef: &[u8],
    vin: usize,
    source_sats: u64,
    caller_key: &PrivateKey,
) -> Vec<u8> {
    use bsv::primitives::transaction_signature::{SIGHASH_ALL, SIGHASH_FORKID};
    use bsv::script::locking_script::LockingScript;
    use bsv::script::templates::p2pkh::P2PKH;
    let mut cursor = std::io::Cursor::new(signable_beef);
    let beef = Beef::from_binary(&mut cursor).expect("signable beef parse");
    let txid = beef.atomic_txid.clone().expect("signable atomic txid");
    let btx = beef.find_txid(&txid).expect("signable tx in beef");
    let mut tx = btx.tx.clone().expect("signable full tx");

    let template = P2PKH::from_private_key(caller_key.clone());
    let lock = LockingScript::from_binary(&p2pkh_lock(caller_key));
    tx.sign(
        vin,
        &template,
        SIGHASH_ALL | SIGHASH_FORKID,
        source_sats,
        &lock,
    )
    .await
    .expect("caller input sign");
    tx.inputs[vin]
        .unlocking_script
        .as_ref()
        .expect("unlock script set")
        .to_binary()
}

/// Collect (height, merkle root) for every bump in a BEEF.
pub fn bump_roots(beef_bytes: &[u8]) -> Result<BTreeMap<u32, String>, String> {
    let mut cursor = std::io::Cursor::new(beef_bytes);
    let beef = Beef::from_binary(&mut cursor).map_err(|e| format!("BEEF parse failed: {e:?}"))?;
    let mut roots = BTreeMap::new();
    for bump in &beef.bumps {
        let level0 = match bump.path.first() {
            Some(l) => l,
            None => continue,
        };
        let leaf_txid = level0
            .iter()
            .find_map(|l| l.hash.as_ref().filter(|h| !h.is_empty()));
        let Some(txid) = leaf_txid else { continue };
        let root = bump
            .compute_root(Some(txid))
            .map_err(|e| format!("compute_root failed: {e:?}"))?;
        if let Some(prev) = roots.insert(bump.block_height, root.clone()) {
            if prev != root {
                return Err(format!(
                    "conflicting roots for height {}: {prev} vs {root}",
                    bump.block_height
                ));
            }
        }
    }
    Ok(roots)
}
