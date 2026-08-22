//! Recorder for the funded BRC-100 conformance vectors.
//!
//! SPENDS REAL SATOSHIS ON MAINNET. `#[ignore]`d and double-gated behind
//! `BSV_FUNDED_RECORD=1`; run deliberately:
//!
//! ```sh
//! BSV_FUNDED_RECORD=1 cargo test --features sqlite \
//!     --test record_funded_vectors -- --ignored --nocapture
//! ```
//!
//! What it does, in phases (idempotent — state persists in
//! `conformance/vectors/wallet/brc100/funded-work/state.json`, so a crashed
//! run resumes rather than re-requesting funding):
//!
//! 1. Mint 5 burner identities; ask the PeerPay faucet on
//!    `message.b1nary.cloud` for 1000 sats each (the faucet pays each
//!    identity exactly once; every request and payment is written to the
//!    ledger before it is acted on).
//! 2. Sweep the funding into wallet(root=…01) as five BRC-29 payments —
//!    funding set S1 — broadcast immediately and verified on-network.
//! 3. Record the 30 root-…01 createAction vectors against S1 (fresh
//!    in-memory wallet per vector, seeded entropy, no broadcast).
//! 4. Sweep S1 → wallet(root=…02) (set S2), record 30; sweep → …03 (S3),
//!    record 28.
//! 5. Broadcast a chain of real recorded vector transactions from root …03:
//!    vector 61 (from S3), then vector 62 recorded against 61's change
//!    (set S3B) — both accepted on mainnet.
//! 6. From 62's change, build the signAction fixture transaction (two P2PKH
//!    caller inputs plus a BRC-29 wallet output — set SIG), record the 8
//!    signAction vectors, and broadcast vector 8's transaction for real.
//! 7. Emit `createaction-funded.json`, `signaction-funded.json`, and
//!    `funded-ledger.json`.
//!
//! Every vector is produced by the same `funded_common` run functions the
//! offline replayer uses, so a recorded `expected` is by construction the
//! output of the replay path.

mod funded_common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::public_key::PublicKey;
use bsv::transaction::beef::Beef;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionInput, CreateActionOptions, CreateActionOutput, SignActionArgs,
    SignActionOptions, SignActionSpend, WalletInterface,
};
use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue};

use bsv_messagebox_client::client::MessageBoxClient;

use bsv_wallet_toolbox::services::services::Services;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
use bsv_wallet_toolbox::wallet::setup::WalletBuilder;

use funded_common::{
    caller_unlock_script, from_hex, p2pkh_lock, run_create_vector, run_sign_vector, to_hex,
    BroadcastRecord, FixtureServices, FundedFile, FundedInput, FundedVector, FundingPayment,
    FundingSet, PostMode, SetupStep,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MESSAGEBOX_HOST: &str = "https://message.b1nary.cloud";
const FAUCET_PEER: &str = "021cf797ea5fd23b13ccff69d595a09f194e7f35ba4e2f1957e488e2365091eb37";
const FAUCET_SATS: u64 = 1000;
/// Payment-request expiry in MILLISECONDS — the faucet reader compares
/// against now_ms and silently discards anything it judges expired; send
/// seconds and the request looks like it expired in 1970.
const REQUEST_TTL_MS: u64 = 60 * 60 * 1000;
const MAX_IDENTITIES: usize = 5;

const ROOT_1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const ROOT_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const ROOT_3: &str = "0000000000000000000000000000000000000000000000000000000000000003";
/// Caller key for signAction P2PKH inputs — a documented test constant in the
/// same spirit as root keys 1/2/3.
const CALLER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000005";

fn work_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/vectors/wallet/brc100/funded-work")
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/vectors/wallet/brc100")
}

fn db_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/funded-dbs");
    std::fs::create_dir_all(&d).expect("create db dir");
    d
}

// ---------------------------------------------------------------------------
// Persistent state
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct IdentityState {
    name: String,
    private_key: String,
    identity_key: String,
    #[serde(default)]
    faucet_requested_at_ms: Option<u64>,
    #[serde(default)]
    faucet_txid: Option<String>,
    #[serde(default)]
    faucet_sats: Option<u64>,
    #[serde(default)]
    internalized: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BroadcastEntry {
    txid: String,
    purpose: String,
    from_identity: String,
    sats_out: u64,
    accepted_by: String,
    verified_on_network: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct State {
    identities: Vec<IdentityState>,
    #[serde(default)]
    funding_sets: BTreeMap<String, FundingSet>,
    #[serde(default)]
    merkle_roots: BTreeMap<u32, String>,
    #[serde(default)]
    broadcasts: Vec<BroadcastEntry>,
    /// Recorded vectors keyed by id.
    #[serde(default)]
    create_vectors: BTreeMap<String, FundedVector>,
    #[serde(default)]
    sign_vectors: BTreeMap<String, FundedVector>,
    /// The signAction fixture transaction (txid) once built.
    #[serde(default)]
    sig_tx: Option<SigTxInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SigTxInfo {
    txid: String,
    beef: String,
    caller_outpoint_a: String,
    caller_outpoint_b: String,
    caller_sats: u64,
}

fn state_path() -> PathBuf {
    work_dir().join("state.json")
}

fn load_state() -> State {
    match std::fs::read_to_string(state_path()) {
        Ok(s) => serde_json::from_str(&s).expect("state.json parse"),
        Err(_) => State::default(),
    }
}

fn save_state(state: &State) {
    std::fs::create_dir_all(work_dir()).expect("create work dir");
    let tmp = state_path().with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(state).expect("state serialize"),
    )
    .expect("state write");
    std::fs::rename(&tmp, state_path()).expect("state rename");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_base64(n: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

// ---------------------------------------------------------------------------
// Wallet plumbing
// ---------------------------------------------------------------------------

fn real_services() -> Arc<dyn WalletServices> {
    Arc::new(Services::from_chain(Chain::Main))
}

/// Wraps real services, capturing post_beef responses so a vector whose
/// recorded run broadcasts inline can carry the network's actual answer for
/// offline replay.
struct RecordingServices {
    inner: Arc<dyn WalletServices>,
    captured: std::sync::Mutex<Vec<bsv_wallet_toolbox::services::types::PostBeefResult>>,
}

#[async_trait::async_trait]
impl WalletServices for RecordingServices {
    fn chain(&self) -> Chain {
        self.inner.chain()
    }
    async fn get_chain_tracker(
        &self,
    ) -> bsv_wallet_toolbox::error::WalletResult<
        Box<dyn bsv::transaction::chain_tracker::ChainTracker>,
    > {
        self.inner.get_chain_tracker().await
    }
    async fn get_merkle_path(
        &self,
        txid: &str,
        use_next: bool,
    ) -> bsv_wallet_toolbox::services::types::GetMerklePathResult {
        self.inner.get_merkle_path(txid, use_next).await
    }
    async fn get_raw_tx(
        &self,
        txid: &str,
        use_next: bool,
    ) -> bsv_wallet_toolbox::services::types::GetRawTxResult {
        self.inner.get_raw_tx(txid, use_next).await
    }
    async fn post_beef(
        &self,
        beef: &[u8],
        txids: &[String],
    ) -> Vec<bsv_wallet_toolbox::services::types::PostBeefResult> {
        let results = self.inner.post_beef(beef, txids).await;
        self.captured
            .lock()
            .expect("captured lock")
            .extend(results.clone());
        results
    }
    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<bsv_wallet_toolbox::services::types::GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
        use_next: bool,
    ) -> bsv_wallet_toolbox::services::types::GetUtxoStatusResult {
        self.inner
            .get_utxo_status(output, output_format, outpoint, use_next)
            .await
    }
    async fn get_status_for_txids(
        &self,
        txids: &[String],
        use_next: bool,
    ) -> bsv_wallet_toolbox::services::types::GetStatusForTxidsResult {
        self.inner.get_status_for_txids(txids, use_next).await
    }
    async fn get_script_hash_history(
        &self,
        hash: &str,
        use_next: bool,
    ) -> bsv_wallet_toolbox::services::types::GetScriptHashHistoryResult {
        self.inner.get_script_hash_history(hash, use_next).await
    }
    async fn hash_to_header(
        &self,
        hash: &str,
    ) -> bsv_wallet_toolbox::error::WalletResult<bsv_wallet_toolbox::services::types::BlockHeader>
    {
        self.inner.hash_to_header(hash).await
    }
    async fn get_header_for_height(
        &self,
        height: u32,
    ) -> bsv_wallet_toolbox::error::WalletResult<Vec<u8>> {
        self.inner.get_header_for_height(height).await
    }
    async fn get_height(&self) -> bsv_wallet_toolbox::error::WalletResult<u32> {
        self.inner.get_height().await
    }
    async fn n_lock_time_is_final(
        &self,
        input: bsv_wallet_toolbox::services::types::NLockTimeInput,
    ) -> bsv_wallet_toolbox::error::WalletResult<bool> {
        self.inner.n_lock_time_is_final(input).await
    }
    async fn get_bsv_exchange_rate(
        &self,
    ) -> bsv_wallet_toolbox::error::WalletResult<bsv_wallet_toolbox::services::types::BsvExchangeRate>
    {
        self.inner.get_bsv_exchange_rate().await
    }
    async fn get_fiat_exchange_rate(
        &self,
        currency: &str,
        base: Option<&str>,
    ) -> bsv_wallet_toolbox::error::WalletResult<f64> {
        self.inner.get_fiat_exchange_rate(currency, base).await
    }
    async fn get_fiat_exchange_rates(
        &self,
        target_currencies: &[String],
    ) -> bsv_wallet_toolbox::error::WalletResult<
        bsv_wallet_toolbox::services::types::FiatExchangeRates,
    > {
        self.inner.get_fiat_exchange_rates(target_currencies).await
    }
    fn get_services_call_history(
        &self,
        reset: bool,
    ) -> bsv_wallet_toolbox::services::types::ServicesCallHistory {
        self.inner.get_services_call_history(reset)
    }
    async fn get_beef_for_txid(&self, txid: &str) -> bsv_wallet_toolbox::error::WalletResult<Beef> {
        self.inner.get_beef_for_txid(txid).await
    }
    fn hash_output_script(&self, script: &[u8]) -> String {
        self.inner.hash_output_script(script)
    }
    async fn is_utxo(
        &self,
        locking_script: &[u8],
        txid: &str,
        vout: u32,
    ) -> bsv_wallet_toolbox::error::WalletResult<bool> {
        self.inner.is_utxo(locking_script, txid, vout).await
    }
}

/// Poll until the network serves the raw tx back, proving acceptance.
async fn verify_on_network(services: &Arc<dyn WalletServices>, txid: &str) -> bool {
    for attempt in 0..30 {
        let r = services.get_raw_tx(txid, false).await;
        if r.raw_tx.is_some() {
            return true;
        }
        eprintln!("  [verify {txid}] not yet visible (attempt {attempt})");
        tokio::time::sleep(Duration::from_secs(4)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Phase 1: identities + faucet
// ---------------------------------------------------------------------------

async fn ensure_identities(state: &mut State) {
    let names = ["A", "B", "C", "D", "E"];
    while state.identities.len() < MAX_IDENTITIES {
        let key = PrivateKey::from_random().expect("random key");
        let ident = IdentityState {
            name: names[state.identities.len()].to_string(),
            private_key: key.to_hex(),
            identity_key: key.to_public_key().to_der_hex(),
            ..Default::default()
        };
        eprintln!("minted identity {} = {}", ident.name, ident.identity_key);
        state.identities.push(ident);
        save_state(state);
    }
}

async fn ensure_faucet_funding(state: &mut State) {
    for i in 0..state.identities.len() {
        if state.identities[i].internalized {
            continue;
        }
        let ident = state.identities[i].clone();
        eprintln!(
            "--- faucet funding for identity {} ({})",
            ident.name, ident.identity_key
        );

        let key = PrivateKey::from_hex(&ident.private_key).expect("identity key parse");
        let db = db_dir().join(format!("faucet-{}.db", ident.name));
        let setup = WalletBuilder::new()
            .chain(Chain::Main)
            .root_key(key)
            .with_sqlite(db.to_str().expect("db path"))
            .with_default_services()
            .without_monitor()
            .build()
            .await
            .expect("faucet wallet build");
        setup
            .storage
            .find_or_insert_user(&setup.identity_key)
            .await
            .expect("user");
        let wallet = Arc::new(setup.wallet);
        let client = MessageBoxClient::new_mainnet(
            MESSAGEBOX_HOST.to_string(),
            wallet.clone(),
            Some("funded-conformance-recorder".to_string()),
        );

        // Ask exactly once per identity — the faucet's issuance is persisted,
        // so a second ask can never yield more, only confusion.
        if state.identities[i].faucet_requested_at_ms.is_none() {
            let expires = now_ms() + REQUEST_TTL_MS;
            state.identities[i].faucet_requested_at_ms = Some(now_ms());
            save_state(state);
            client
                .request_payment(
                    FAUCET_PEER,
                    FAUCET_SATS,
                    &format!(
                        "rust-wallet-toolbox funded BRC-100 conformance vectors ({})",
                        ident.name
                    ),
                    expires,
                )
                .await
                .expect("faucet request_payment");
            eprintln!("  request sent ({FAUCET_SATS} sats), waiting for the faucet…");
        } else {
            eprintln!("  request already sent earlier; polling inbox…");
        }

        let deadline = SystemTime::now() + Duration::from_secs(900);
        loop {
            let payments = client
                .list_incoming_payments()
                .await
                .expect("list_incoming_payments");
            if let Some(p) = payments.first() {
                // Persist the token before accepting: internalize
                // acknowledges (deletes) the message, and the derivation
                // info in the token is the only way to spend the payment.
                let token_path = work_dir().join(format!("token-{}.json", ident.name));
                std::fs::write(
                    &token_path,
                    serde_json::to_string_pretty(&serde_json::json!({
                        "sender": p.sender,
                        "message_id": p.message_id,
                        "amount": p.token.amount,
                        "derivation_prefix": p.token.custom_instructions.derivation_prefix,
                        "derivation_suffix": p.token.custom_instructions.derivation_suffix,
                        "transaction_beef_hex": to_hex(&p.token.transaction),
                    }))
                    .expect("token json"),
                )
                .expect("token write");

                let mut cursor = std::io::Cursor::new(&p.token.transaction);
                let beef = Beef::from_binary(&mut cursor).expect("faucet beef parse");
                let txid = beef.atomic_txid.clone().unwrap_or_default();

                client.accept_payment(p).await.expect("accept_payment");
                state.identities[i].faucet_txid = Some(txid.clone());
                state.identities[i].faucet_sats = Some(p.token.amount);
                state.identities[i].internalized = true;
                save_state(state);
                eprintln!("  funded: {} sats, txid {txid}", p.token.amount);
                break;
            }
            if SystemTime::now() > deadline {
                panic!(
                    "faucet did not fund identity {} within the timeout — stopping rather than \
                     re-requesting (the faucet pays each identity exactly once)",
                    ident.name
                );
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Transfers (BRC-29, immediate broadcast)
// ---------------------------------------------------------------------------

/// Create and broadcast a BRC-29 payment from `wallet` (whose root key is
/// `sender_root`) to `receiver_identity`, returning the funding fixture.
#[allow(clippy::too_many_arguments)]
async fn send_brc29<W: WalletInterface>(
    wallet: &W,
    services: &Arc<dyn WalletServices>,
    sender_root: &PrivateKey,
    sender_identity: &str,
    receiver_identity: &str,
    amount: u64,
    description: &str,
    extra_outputs: Vec<CreateActionOutput>,
) -> Result<(FundingPayment, Vec<serde_json::Value>), String> {
    let prefix = random_base64(8);
    let suffix = random_base64(8);
    let receiver_pub =
        PublicKey::from_string(receiver_identity).map_err(|e| format!("receiver key: {e:?}"))?;
    let template = ScriptTemplateBRC29::new(prefix.clone(), suffix.clone());
    let lock_script = template
        .lock(sender_root, &receiver_pub)
        .map_err(|e| format!("brc29 lock: {e}"))?;

    // A transfer whose derivation is lost strands the sats: the receiver can
    // only derive the spending key from (sender, prefix, suffix). Persist the
    // derivation BEFORE broadcasting, and the full fixture (with BEEF) right
    // after, so no crash window can orphan a broadcast payment.
    let pending_log = work_dir().join("pending-transfers.jsonl");
    let log_line = |v: &serde_json::Value| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&pending_log)
            .expect("open pending-transfers.jsonl");
        writeln!(f, "{v}").expect("append pending transfer");
    };
    log_line(&serde_json::json!({
        "stage": "pre-broadcast",
        "description": description,
        "sender_identity_key": sender_identity,
        "receiver_identity_key": receiver_identity,
        "derivation_prefix": prefix,
        "derivation_suffix": suffix,
        "amount": amount,
    }));

    let n_extra = extra_outputs.len();
    let mut outputs = extra_outputs;
    outputs.push(CreateActionOutput {
        locking_script: Some(lock_script),
        satoshis: amount,
        output_description: description.to_string(),
        basket: None,
        custom_instructions: None,
        tags: vec![],
    });

    let result = wallet
        .create_action(
            CreateActionArgs {
                description: description.to_string(),
                input_beef: None,
                inputs: vec![],
                outputs,
                lock_time: None,
                version: None,
                labels: vec!["funded-conformance".to_string()],
                options: Some(CreateActionOptions {
                    accept_delayed_broadcast: BooleanDefaultTrue(Some(false)),
                    randomize_outputs: BooleanDefaultTrue(Some(false)),
                    ..Default::default()
                }),
                reference: None,
            },
            None,
        )
        .await
        .map_err(|e| format!("transfer create_action: {e}"))?;

    let txid = result.txid.clone().ok_or("transfer returned no txid")?;
    let beef_bytes = result.tx.clone().ok_or("transfer returned no tx")?;

    let fixture = FundingPayment {
        beef: to_hex(&beef_bytes),
        output_index: n_extra as u32,
        derivation_prefix: prefix,
        derivation_suffix: suffix,
        sender_identity_key: sender_identity.to_string(),
        satoshis: amount,
        txid: txid.clone(),
        description: description.to_string(),
    };
    log_line(&serde_json::json!({
        "stage": "broadcast",
        "fixture": serde_json::to_value(&fixture).expect("fixture json"),
    }));

    if !verify_on_network(services, &txid).await {
        return Err(format!(
            "transfer {txid} not visible on the network (fixture saved in pending-transfers.jsonl)"
        ));
    }

    Ok((
        fixture,
        result
            .send_with_results
            .iter()
            .map(|s| serde_json::to_value(s).unwrap_or_default())
            .collect(),
    ))
}

/// Try amounts descending until the wallet accepts one (fee headroom).
async fn send_max_brc29<W: WalletInterface>(
    wallet: &W,
    services: &Arc<dyn WalletServices>,
    sender_root: &PrivateKey,
    sender_identity: &str,
    receiver_identity: &str,
    balance: u64,
    description: &str,
) -> Result<FundingPayment, String> {
    let mut last_err = String::new();
    // Walk down from balance in small steps; the first amount the wallet can
    // fund (outputs + fee <= available) wins.
    let mut amount = balance.saturating_sub(2);
    for _ in 0..60 {
        if amount == 0 {
            break;
        }
        match send_brc29(
            wallet,
            services,
            sender_root,
            sender_identity,
            receiver_identity,
            amount,
            description,
            vec![],
        )
        .await
        {
            Ok((p, _)) => return Ok(p),
            Err(e) if e.to_uppercase().contains("INSUFFICIENT") => {
                last_err = e;
                amount = amount.saturating_sub(5);
            }
            Err(e) => return Err(e),
        }
    }
    Err(format!(
        "could not size transfer below balance {balance}: {last_err}"
    ))
}

// ---------------------------------------------------------------------------
// Vector args transformation
// ---------------------------------------------------------------------------

/// Normalize one upstream createAction vector's args into spec-shaped args,
/// returning the corrected args and the notes describing each correction.
fn transform_create_args(upstream: &serde_json::Value) -> (serde_json::Value, Vec<String>) {
    let mut args = upstream.clone();
    let mut notes = Vec::new();
    let obj = args.as_object_mut().expect("args object");

    // Upstream places noSend/acceptDelayedBroadcast at args top level, where
    // every BRC-100 implementation's CreateActionArgs silently drops them
    // (they belong in `options`). Move them where the generator meant them.
    let mut options = serde_json::Map::new();
    if let Some(v) = obj.remove("noSend") {
        options.insert("noSend".to_string(), v);
        notes.push(
            "upstream put noSend at args top level (silently dropped by every impl's parser); \
             moved into options where BRC-100 defines it"
                .to_string(),
        );
    }
    if let Some(v) = obj.remove("acceptDelayedBroadcast") {
        options.insert("acceptDelayedBroadcast".to_string(), v);
        notes.push(
            "upstream put acceptDelayedBroadcast at args top level; moved into options".to_string(),
        );
    }
    if !options.is_empty() {
        obj.insert("options".to_string(), serde_json::Value::Object(options));
    }

    // BRC-100 reserves the basket name `default` (the wallet's own change
    // basket); claiming it on an action output is a spec violation the
    // upstream generator emitted anyway. Renamed to `corpus`.
    if let Some(outputs) = obj.get_mut("outputs").and_then(|o| o.as_array_mut()) {
        let mut renamed = false;
        for out in outputs {
            if out.get("basket").and_then(|b| b.as_str()) == Some("default") {
                out["basket"] = serde_json::Value::String("corpus".to_string());
                renamed = true;
            }
        }
        if renamed {
            notes.push(
                "upstream claimed the reserved basket 'default' on an output (BRC-100 reserves \
                 it for wallet change; the SDK validator rejects it); renamed to 'corpus'"
                    .to_string(),
            );
        }
    }

    (args, notes)
}

/// The corpus has 30 vectors with noSend=false AND acceptDelayedBroadcast=
/// false — faithful normalization makes them broadcast inline, and 30 real
/// broadcasts from one funding state are mutually double-spending (and far
/// over budget). All but a designated representative are recorded as delayed
/// sends; the representative (vector 62) records the true inline-broadcast
/// behavior against the real network.
fn force_delayed_if_immediate(args: &mut serde_json::Value, notes: &mut Vec<String>) {
    let Some(options) = args.get_mut("options").and_then(|o| o.as_object_mut()) else {
        return;
    };
    let no_send = options
        .get("noSend")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let adb = options
        .get("acceptDelayedBroadcast")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !no_send && !adb {
        options.insert(
            "acceptDelayedBroadcast".to_string(),
            serde_json::Value::Bool(true),
        );
        notes.push(
            "upstream intent was an immediate broadcast (noSend=false,              acceptDelayedBroadcast=false); recorded as a delayed send because 30 immediate              broadcasts from one funding state are mutually double-spending. The true              inline-broadcast path is recorded (and network-verified) by vector 62 and              signaction vector 8"
                .to_string(),
        );
    }
}

// ---------------------------------------------------------------------------
// Upstream corpus loading
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct UpstreamFile {
    vectors: Vec<UpstreamVector>,
}

#[derive(serde::Deserialize)]
struct UpstreamVector {
    id: String,
    description: String,
    input: UpstreamInput,
    expected: serde_json::Value,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(serde::Deserialize)]
struct UpstreamInput {
    #[serde(default)]
    root_key: Option<String>,
    args: serde_json::Value,
}

fn load_upstream(name: &str) -> UpstreamFile {
    let path = corpus_dir().join(name);
    let text = std::fs::read_to_string(&path).expect("upstream corpus read");
    serde_json::from_str(&text).expect("upstream corpus parse")
}

// ---------------------------------------------------------------------------
// createAction recording
// ---------------------------------------------------------------------------

fn fixture_services(state: &State) -> Arc<FixtureServices> {
    Arc::new(FixtureServices {
        roots: state.merkle_roots.clone(),
        post: PostMode::Tripwire,
    })
}

async fn record_create_vector(
    state: &mut State,
    upstream: &UpstreamVector,
    funding_set: &str,
    args: serde_json::Value,
    notes: Vec<String>,
    recorded_id: String,
) {
    record_create_vector_inner(state, upstream, funding_set, args, notes, recorded_id, None).await;
}

/// `broadcast_via`: real services for a vector whose recorded run performs a
/// real inline broadcast (noSend=false, acceptDelayedBroadcast=false); the
/// network's response is captured into `expected.postBeefResponse` so offline
/// replay can serve it back instead of the tripwire.
async fn record_create_vector_inner(
    state: &mut State,
    upstream: &UpstreamVector,
    funding_set: &str,
    args: serde_json::Value,
    notes: Vec<String>,
    recorded_id: String,
    broadcast_via: Option<Arc<dyn WalletServices>>,
) {
    if state.create_vectors.contains_key(&recorded_id) {
        return;
    }
    let root_key = upstream
        .input
        .root_key
        .clone()
        .expect("upstream vector has root_key");
    let seed = recorded_id.clone();
    let funding = state.funding_sets[funding_set].payments.clone();

    let (services, recording): (Arc<dyn WalletServices>, Option<Arc<RecordingServices>>) =
        match &broadcast_via {
            Some(real) => {
                let rec = Arc::new(RecordingServices {
                    inner: real.clone(),
                    captured: std::sync::Mutex::new(vec![]),
                });
                (rec.clone(), Some(rec))
            }
            None => (fixture_services(state), None),
        };

    let run = run_create_vector(services, &root_key, &seed, &funding, &args)
        .await
        .unwrap_or_else(|e| panic!("{recorded_id}: recording run failed: {e}"));

    eprintln!(
        "recorded {recorded_id}: {} txid={:?}",
        run.outcome.status, run.outcome.txid
    );

    let mut expected = serde_json::to_value(&run.outcome).expect("outcome serialize");
    let mut broadcast = BroadcastRecord {
        sent: false,
        txid: None,
        accepted_by: None,
    };
    if let Some(rec) = recording {
        assert_eq!(
            run.outcome.status, "success",
            "{recorded_id}: inline-broadcast vector failed"
        );
        let captured = rec.captured.lock().expect("captured").clone();
        let accepted = captured
            .iter()
            .find(|r| r.status == "success")
            .unwrap_or_else(|| panic!("{recorded_id}: inline broadcast rejected: {captured:?}"));
        let txid = run.outcome.txid.clone().expect("txid");
        let real = broadcast_via.expect("real services");
        assert!(
            verify_on_network(&real, &txid).await,
            "{recorded_id}: {txid} not visible on network"
        );
        broadcast = BroadcastRecord {
            sent: true,
            txid: Some(txid.clone()),
            accepted_by: Some(accepted.name.clone()),
        };
        expected["postBeefResponse"] =
            serde_json::to_value(&captured).expect("postBeefResponse serialize");
        state.broadcasts.push(BroadcastEntry {
            txid,
            purpose: format!("{recorded_id} inline broadcast (acceptDelayedBroadcast=false)"),
            from_identity: root_key.clone(),
            sats_out: 0,
            accepted_by: accepted.name.clone(),
            verified_on_network: true,
        });
    }

    let vector = FundedVector {
        id: recorded_id.clone(),
        description: upstream.description.clone(),
        input: FundedInput {
            root_key,
            funding_set: funding_set.to_string(),
            entropy_seed: seed,
            setup: vec![],
            args,
        },
        expected,
        broadcast,
        notes,
        upstream: Some(serde_json::json!({
            "id": upstream.id,
            "expected": upstream.expected,
            "note": "upstream expected values are fabrications: a zero-input tx no wallet can produce",
        })),
        tags: upstream.tags.clone(),
    };
    state.create_vectors.insert(recorded_id, vector);
    save_state(state);
}

/// Record the two vectors that pin upstream corpus defects verbatim.
async fn record_defect_vectors(state: &mut State, first_upstream: &UpstreamVector) {
    // 1. Verbatim upstream args: the reserved basket 'default' must be
    //    rejected at validation.
    let id1 = "wallet.brc100.createaction-funded.defect.reserved-basket".to_string();
    if !state.create_vectors.contains_key(&id1) {
        let args = first_upstream.input.args.clone();
        let root_key = first_upstream.input.root_key.clone().expect("root key");
        let funding = state.funding_sets["S1"].payments.clone();
        let run = run_create_vector(fixture_services(state), &root_key, &id1, &funding, &args)
            .await
            .expect("defect vector run");
        assert_eq!(
            run.outcome.status, "error",
            "expected the reserved basket 'default' to be rejected"
        );
        eprintln!("recorded {id1}: {:?}", run.outcome.message);
        state.create_vectors.insert(
            id1.clone(),
            FundedVector {
                id: id1.clone(),
                description: "verbatim upstream args: output claiming reserved basket 'default' \
                              is rejected at validation (BRC-100 reserves it)"
                    .to_string(),
                input: FundedInput {
                    root_key,
                    funding_set: "S1".to_string(),
                    entropy_seed: id1.clone(),
                    setup: vec![],
                    args,
                },
                expected: serde_json::to_value(&run.outcome).expect("serialize"),
                broadcast: BroadcastRecord {
                    sent: false,
                    txid: None,
                    accepted_by: None,
                },
                notes: vec![
                    "pins the upstream corpus defect: all 90 upstream createaction vectors claim \
                     basket 'default' on an output, which a conforming implementation rejects"
                        .to_string(),
                ],
                upstream: Some(serde_json::json!({"id": first_upstream.id})),
                tags: vec!["error".to_string(), "corpus-defect".to_string()],
            },
        );
        save_state(state);
    }

    // 2. Basket fixed but noSend/acceptDelayedBroadcast left at top level:
    //    parsers drop them silently, so a vector meant as noSend runs as a
    //    delayed send (empty noSendChange proves the flag never arrived).
    let id2 = "wallet.brc100.createaction-funded.defect.toplevel-flags".to_string();
    if !state.create_vectors.contains_key(&id2) {
        let mut args = first_upstream.input.args.clone();
        if let Some(outputs) = args.get_mut("outputs").and_then(|o| o.as_array_mut()) {
            for out in outputs {
                if out.get("basket").and_then(|b| b.as_str()) == Some("default") {
                    out["basket"] = serde_json::Value::String("corpus".to_string());
                }
            }
        }
        let root_key = first_upstream.input.root_key.clone().expect("root key");
        let funding = state.funding_sets["S1"].payments.clone();
        let run = run_create_vector(fixture_services(state), &root_key, &id2, &funding, &args)
            .await
            .expect("defect vector run");
        assert_eq!(run.outcome.status, "success");
        assert!(
            run.outcome.no_send_change.is_empty(),
            "top-level noSend was expected to be dropped by the parser"
        );
        eprintln!("recorded {id2}: txid={:?}", run.outcome.txid);
        state.create_vectors.insert(
            id2.clone(),
            FundedVector {
                id: id2.clone(),
                description: "upstream's top-level noSend/acceptDelayedBroadcast are silently \
                              dropped (BRC-100 defines them inside options): the action records \
                              as a delayed send, not a noSend"
                    .to_string(),
                input: FundedInput {
                    root_key,
                    funding_set: "S1".to_string(),
                    entropy_seed: id2.clone(),
                    setup: vec![],
                    args,
                },
                expected: serde_json::to_value(&run.outcome).expect("serialize"),
                broadcast: BroadcastRecord {
                    sent: false,
                    txid: None,
                    accepted_by: None,
                },
                notes: vec![
                    "pins the upstream corpus defect: options flags at args top level never \
                     reach any implementation's options parsing"
                        .to_string(),
                ],
                upstream: Some(serde_json::json!({"id": first_upstream.id})),
                tags: vec!["corpus-defect".to_string()],
            },
        );
        save_state(state);
    }
}

// ---------------------------------------------------------------------------
// Broadcast of recorded vectors
// ---------------------------------------------------------------------------

/// Post a recorded vector's transaction to the network and mark the record.
async fn broadcast_recorded_vector(
    state: &mut State,
    services: &Arc<dyn WalletServices>,
    vector_id: &str,
    purpose: &str,
    from_identity: &str,
    sats_out: u64,
) {
    let vector = state
        .create_vectors
        .get(vector_id)
        .unwrap_or_else(|| panic!("{vector_id} not recorded yet"))
        .clone();
    if vector.broadcast.sent {
        return;
    }
    let txid = vector
        .expected
        .get("txid")
        .and_then(|t| t.as_str())
        .expect("vector has txid")
        .to_string();
    let beef_hex = vector
        .expected
        .get("tx")
        .and_then(|t| t.as_str())
        .expect("vector has tx")
        .to_string();

    let results = services
        .post_beef(&from_hex(&beef_hex), std::slice::from_ref(&txid))
        .await;
    let accepted = results.iter().find(|r| r.status == "success");
    let accepted_by = accepted
        .map(|r| r.name.clone())
        .unwrap_or_else(|| panic!("{vector_id}: no service accepted broadcast: {results:?}"));

    let verified = verify_on_network(services, &txid).await;
    assert!(
        verified,
        "{vector_id}: broadcast {txid} not visible on network"
    );

    let entry = state.create_vectors.get_mut(vector_id).expect("vector");
    entry.broadcast = BroadcastRecord {
        sent: true,
        txid: Some(txid.clone()),
        accepted_by: Some(accepted_by.clone()),
    };
    state.broadcasts.push(BroadcastEntry {
        txid,
        purpose: purpose.to_string(),
        from_identity: from_identity.to_string(),
        sats_out,
        accepted_by,
        verified_on_network: true,
    });
    save_state(state);
}

// ---------------------------------------------------------------------------
// Root pinning
// ---------------------------------------------------------------------------

async fn pin_roots_for_set(state: &mut State, services: &Arc<dyn WalletServices>, set: &str) {
    let payments = state.funding_sets[set].payments.clone();
    for p in &payments {
        let roots = funded_common::bump_roots(&from_hex(&p.beef))
            .unwrap_or_else(|e| panic!("bump roots for {}: {e}", p.txid));
        let tracker = services.get_chain_tracker().await.expect("tracker");
        for (h, r) in roots {
            let ok = tracker
                .is_valid_root_for_height(&r, h)
                .await
                .expect("root check");
            assert!(ok, "merkle root at height {h} not valid on mainnet: {r}");
            if let Some(prev) = state.merkle_roots.insert(h, r.clone()) {
                assert_eq!(prev, r, "conflicting pinned roots at height {h}");
            }
        }
    }
    save_state(state);
}

// ---------------------------------------------------------------------------
// signAction recording
// ---------------------------------------------------------------------------

/// Precursor createAction args for a signAction vector: spends the given
/// caller outpoints (unlocking script supplied later via signAction) plus
/// wallet change, pays one small output.
fn sign_precursor_args(
    sig: &SigTxInfo,
    caller_outpoints: &[&str],
    no_send: bool,
    chained_no_send_change: Vec<String>,
) -> CreateActionArgs {
    let caller_key = PrivateKey::from_hex(CALLER_KEY).expect("caller key");
    CreateActionArgs {
        description: "funded signAction precursor".to_string(),
        input_beef: Some(from_hex(&sig.beef)),
        inputs: caller_outpoints
            .iter()
            .map(|op| CreateActionInput {
                outpoint: (*op).to_string(),
                input_description: "conformance caller input".to_string(),
                unlocking_script: None,
                unlocking_script_length: Some(108),
                sequence_number: None,
            })
            .collect(),
        outputs: vec![CreateActionOutput {
            locking_script: Some(p2pkh_lock(&caller_key)),
            satoshis: 100,
            output_description: "signAction vector output".to_string(),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        }],
        lock_time: None,
        version: None,
        labels: vec!["funded-conformance".to_string()],
        options: Some(CreateActionOptions {
            sign_and_process: BooleanDefaultTrue(Some(false)),
            no_send: BooleanDefaultFalse(Some(no_send)),
            no_send_change: chained_no_send_change,
            ..Default::default()
        }),
        reference: None,
    }
}

/// Fully-processed noSend createAction (for sendWith precursor txids).
fn nosend_precursor_args(chained_no_send_change: Vec<String>) -> CreateActionArgs {
    let caller_key = PrivateKey::from_hex(CALLER_KEY).expect("caller key");
    CreateActionArgs {
        description: "funded signAction sendWith precursor".to_string(),
        input_beef: None,
        inputs: vec![],
        outputs: vec![CreateActionOutput {
            locking_script: Some(p2pkh_lock(&caller_key)),
            satoshis: 100,
            output_description: "sendWith batch output".to_string(),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        }],
        lock_time: None,
        version: None,
        labels: vec!["funded-conformance".to_string()],
        options: Some(CreateActionOptions {
            no_send: BooleanDefaultFalse(Some(true)),
            no_send_change: chained_no_send_change,
            ..Default::default()
        }),
        reference: None,
    }
}

/// SignActionOptions with every field in the wire-absent state, so serialized
/// args carry only what the vector explicitly sets (absent means "inherit
/// from createAction" in BRC-100).
fn sign_options_none() -> SignActionOptions {
    SignActionOptions {
        accept_delayed_broadcast: BooleanDefaultTrue::none(),
        return_txid_only: BooleanDefaultFalse::none(),
        no_send: BooleanDefaultFalse::none(),
        send_with: vec![],
    }
}

struct SignVectorPlan {
    upstream_index: usize,
    /// Setup createAction args, in order.
    setup: Vec<CreateActionArgs>,
    /// Index (into `setup`) of the signable precursor whose reference the
    /// signAction consumes, or None when the vector needs no precursor.
    signable_setup: Option<usize>,
    /// (vin, source sats) caller inputs to really sign.
    caller_inputs: Vec<(usize, u64)>,
    sign_options: Option<SignActionOptions>,
    /// For the unknown-reference / empty-spends error vectors: use the
    /// upstream args verbatim.
    verbatim_args: Option<serde_json::Value>,
    notes: Vec<String>,
    /// Whether this vector's recorded run broadcasts inline (v8).
    broadcasts: bool,
}

async fn record_sign_vectors(state: &mut State, real: &Arc<dyn WalletServices>) {
    let upstream = load_upstream("signaction.json");
    let sig = state.sig_tx.clone().expect("sig fixture built");
    let outpoint_a = sig.caller_outpoint_a.clone();
    let outpoint_b = sig.caller_outpoint_b.clone();
    let funding = state.funding_sets["SIG"].payments.clone();

    // Recorded ids follow the upstream numbering.
    for n in 1..=8usize {
        let recorded_id = format!("wallet.brc100.signaction-funded.{n}");
        if state.sign_vectors.contains_key(&recorded_id) {
            continue;
        }
        let uv = &upstream.vectors[n - 1];
        let seed = recorded_id.clone();

        let plan: SignVectorPlan = match n {
            1 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![sign_precursor_args(&sig, &[&outpoint_a], true, vec![])],
                signable_setup: Some(0),
                caller_inputs: vec![(0, sig.caller_sats)],
                sign_options: None,
                verbatim_args: None,
                notes: vec![
                    "upstream reference/unlockingScript were fabrications (no precursor action \
                     exists); recorded against a real signable precursor with a real P2PKH \
                     signature over the actual sighash"
                        .to_string(),
                ],
                broadcasts: false,
            },
            2 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![sign_precursor_args(
                    &sig,
                    &[&outpoint_a, &outpoint_b],
                    true,
                    vec![],
                )],
                signable_setup: Some(0),
                caller_inputs: vec![(0, sig.caller_sats), (1, sig.caller_sats)],
                sign_options: None,
                verbatim_args: None,
                notes: vec!["two real caller inputs, both really signed".to_string()],
                broadcasts: false,
            },
            3 => SignVectorPlan {
                upstream_index: n - 1,
                // Precursor is NOT noSend, so the sign-time noSend option is
                // the thing that keeps this off the network.
                setup: vec![sign_precursor_args(&sig, &[&outpoint_a], false, vec![])],
                signable_setup: Some(0),
                caller_inputs: vec![(0, sig.caller_sats)],
                sign_options: Some(SignActionOptions {
                    no_send: BooleanDefaultFalse(Some(true)),
                    accept_delayed_broadcast: BooleanDefaultTrue(Some(true)),
                    ..sign_options_none()
                }),
                verbatim_args: None,
                notes: vec![
                    "precursor created without noSend; the signAction options.noSend=true is \
                     what makes the result a noSend"
                        .to_string(),
                ],
                broadcasts: false,
            },
            4 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![sign_precursor_args(&sig, &[&outpoint_a], true, vec![])],
                signable_setup: Some(0),
                caller_inputs: vec![(0, sig.caller_sats)],
                sign_options: Some(SignActionOptions {
                    return_txid_only: BooleanDefaultFalse(Some(true)),
                    ..sign_options_none()
                }),
                verbatim_args: None,
                notes: vec![],
                broadcasts: false,
            },
            5 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![
                    nosend_precursor_args(vec![]),
                    // chained below at run time (needs run 0's noSendChange)
                ],
                signable_setup: None, // resolved specially below
                caller_inputs: vec![(0, sig.caller_sats)],
                sign_options: None, // sendWith injected below with real txids
                verbatim_args: None,
                notes: vec![
                    "upstream sendWith txids were placeholders; replaced with the txids of two \
                     real noSend precursor actions in the same wallet (the second funded \
                     entirely by the first's change via options.noSendChange)"
                        .to_string(),
                    "the signable precursor is a delayed send, not noSend: both the TS \
                     reference and this implementation treat isNoSend && isSendWith as an \
                     internal error"
                        .to_string(),
                ],
                broadcasts: false,
            },
            6 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![],
                signable_setup: None,
                caller_inputs: vec![],
                sign_options: None,
                verbatim_args: Some(uv.input.args.clone()),
                notes: vec!["verbatim upstream args; records the real error".to_string()],
                broadcasts: false,
            },
            7 => SignVectorPlan {
                upstream_index: n - 1,
                setup: vec![],
                signable_setup: None,
                caller_inputs: vec![],
                sign_options: None,
                verbatim_args: Some(uv.input.args.clone()),
                notes: vec!["verbatim upstream args; records the real error".to_string()],
                broadcasts: false,
            },
            8 => SignVectorPlan {
                upstream_index: n - 1,
                // Not noSend and not delayed at sign time: the recorded run
                // really broadcasts, inline, and the vector carries the
                // network's response for offline replay.
                setup: vec![sign_precursor_args(&sig, &[&outpoint_b], false, vec![])],
                signable_setup: Some(0),
                caller_inputs: vec![(0, sig.caller_sats)],
                sign_options: Some(SignActionOptions {
                    accept_delayed_broadcast: BooleanDefaultTrue(Some(false)),
                    ..sign_options_none()
                }),
                verbatim_args: None,
                notes: vec![
                    "recorded with a real inline broadcast (acceptDelayedBroadcast=false); \
                     expected.postBeefResponse carries the network's recorded answer, which \
                     offline replay serves back instead of the tripwire"
                        .to_string(),
                ],
                broadcasts: true,
            },
            _ => unreachable!(),
        };

        record_one_sign_vector(
            state,
            real,
            &upstream.vectors[plan.upstream_index],
            plan,
            recorded_id,
            seed,
            &funding,
        )
        .await;
    }
}

async fn record_one_sign_vector(
    state: &mut State,
    real: &Arc<dyn WalletServices>,
    uv: &UpstreamVector,
    plan: SignVectorPlan,
    recorded_id: String,
    seed: String,
    funding: &[FundingPayment],
) {
    // --- Verbatim error vectors need no discovery pass ---
    if let Some(args) = plan.verbatim_args.clone() {
        let run = run_sign_vector(fixture_services(state), ROOT_1, &seed, funding, &[], &args)
            .await
            .unwrap_or_else(|e| panic!("{recorded_id}: run failed: {e}"));
        assert_eq!(
            run.outcome.status, "error",
            "{recorded_id}: expected an error"
        );
        eprintln!("recorded {recorded_id}: error {:?}", run.outcome.message);
        state.sign_vectors.insert(
            recorded_id.clone(),
            FundedVector {
                id: recorded_id,
                description: uv.description.clone(),
                input: FundedInput {
                    root_key: ROOT_1.to_string(),
                    funding_set: "SIG".to_string(),
                    entropy_seed: seed,
                    setup: vec![],
                    args,
                },
                expected: serde_json::to_value(&run.outcome).expect("serialize"),
                broadcast: BroadcastRecord {
                    sent: false,
                    txid: None,
                    accepted_by: None,
                },
                notes: plan.notes,
                upstream: Some(serde_json::json!({"id": uv.id, "expected": uv.expected})),
                tags: uv.tags.clone(),
            },
        );
        save_state(state);
        return;
    }

    // --- Pass 1 (discovery): run the setup with the vector's seed to learn
    // the reference, the signable tx bytes, and (for sendWith) precursor
    // txids; then compute the real caller unlocking scripts.
    let mut discovery_setup = plan.setup.clone();

    // Vector 5's second noSend precursor and its signable precursor are
    // constructed during discovery because they chain on run-time outputs.
    let is_send_with_vector = plan.signable_setup.is_none() && plan.verbatim_args.is_none();

    let (reference, signable_beef, send_with_txids, final_setup_json): (
        String,
        Vec<u8>,
        Vec<String>,
        Vec<serde_json::Value>,
    );

    if is_send_with_vector {
        // Discovery for the sendWith vector: run precursors one at a time in
        // a single wallet, chaining noSendChange.
        let setup_wallet = funded_common::build_vector_wallet(ROOT_1, fixture_services(state))
            .await
            .expect("discovery wallet");
        for p in funding {
            funded_common::internalize_funding(&setup_wallet, p)
                .await
                .expect("discovery funding");
        }
        bsv_wallet_toolbox::utility::conformance_entropy::set_conformance_entropy(&seed);

        let ns1_args = nosend_precursor_args(vec![]);
        let ns1 = setup_wallet
            .wallet
            .create_action(ns1_args.clone(), None)
            .await
            .expect("noSend precursor 1");
        let ns1_txid = ns1.txid.clone().expect("ns1 txid");

        let ns2_args = nosend_precursor_args(ns1.no_send_change.clone());
        let ns2 = setup_wallet
            .wallet
            .create_action(ns2_args.clone(), None)
            .await
            .expect("noSend precursor 2");
        let ns2_txid = ns2.txid.clone().expect("ns2 txid");

        // The signable action is NOT noSend: it is the delayed send that
        // carries the batch (TS: isNoSend && isSendWith is an internal error
        // in both implementations — sendWith members are the noSend actions,
        // the signing action rides normally). Its caller input covers the
        // output + fee, so it needs no wallet change at all.
        let sig = state.sig_tx.clone().expect("sig fixture");
        let signable_args = sign_precursor_args(&sig, &[&sig.caller_outpoint_a], false, vec![]);
        let signable = setup_wallet
            .wallet
            .create_action(signable_args.clone(), None)
            .await
            .expect("signable precursor");
        bsv_wallet_toolbox::utility::conformance_entropy::clear_conformance_entropy();

        let st = signable.signable_transaction.expect("signable");
        // Raw reference bytes → the base64 text storage keys the row by.
        reference = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&st.reference)
        };
        signable_beef = st.tx;
        send_with_txids = vec![ns1_txid, ns2_txid];
        final_setup_json = vec![
            serde_json::to_value(&ns1_args).expect("args"),
            serde_json::to_value(&ns2_args).expect("args"),
            serde_json::to_value(&signable_args).expect("args"),
        ];
    } else {
        let signable_idx = plan.signable_setup.expect("signable setup index");
        let discovery_args: Vec<serde_json::Value> = discovery_setup
            .iter()
            .map(|a| serde_json::to_value(a).expect("args"))
            .collect();
        let setup_wallet = funded_common::build_vector_wallet(ROOT_1, fixture_services(state))
            .await
            .expect("discovery wallet");
        for p in funding {
            funded_common::internalize_funding(&setup_wallet, p)
                .await
                .expect("discovery funding");
        }
        bsv_wallet_toolbox::utility::conformance_entropy::set_conformance_entropy(&seed);
        let mut sig_ref: Option<(String, Vec<u8>)> = None;
        for (i, a) in discovery_setup.drain(..).enumerate() {
            let r = setup_wallet
                .wallet
                .create_action(a, None)
                .await
                .unwrap_or_else(|e| panic!("{recorded_id}: discovery setup[{i}] failed: {e}"));
            if i == signable_idx {
                let st = r.signable_transaction.expect("signable");
                // Raw reference bytes → base64 text (the storage key).
                let text = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&st.reference)
                };
                sig_ref = Some((text, st.tx));
            }
        }
        bsv_wallet_toolbox::utility::conformance_entropy::clear_conformance_entropy();
        let (r, b) = sig_ref.expect("signable discovered");
        reference = r;
        signable_beef = b;
        send_with_txids = vec![];
        final_setup_json = discovery_args;
    }
    let setup_json = final_setup_json;

    // --- Compute the real unlocking scripts over the discovered signable tx.
    let mut spends: std::collections::HashMap<u32, SignActionSpend> = Default::default();
    for (vin, sats) in &plan.caller_inputs {
        let caller_key = PrivateKey::from_hex(CALLER_KEY).expect("caller key");
        let unlock = caller_unlock_script(&signable_beef, *vin, *sats, &caller_key).await;
        spends.insert(
            *vin as u32,
            SignActionSpend {
                unlocking_script: unlock,
                sequence_number: None,
            },
        );
    }

    let mut sign_options = plan.sign_options.clone();
    if !send_with_txids.is_empty() {
        sign_options = Some(SignActionOptions {
            send_with: send_with_txids.clone(),
            ..sign_options_none()
        });
    }

    let sign_args = SignActionArgs {
        reference: reference.clone().into_bytes(),
        spends,
        options: sign_options,
    };
    let sign_args_json = serde_json::to_value(&sign_args).expect("sign args serialize");

    // --- Pass 2 (canonical): run through the exact function the offline
    // replayer uses, with the same seed, so recorded == replayable by
    // construction. v8 runs against real services because its recorded
    // behavior includes the inline broadcast.
    let (services, recording): (Arc<dyn WalletServices>, Option<Arc<RecordingServices>>) =
        if plan.broadcasts {
            let rec = Arc::new(RecordingServices {
                inner: real.clone(),
                captured: std::sync::Mutex::new(vec![]),
            });
            (rec.clone(), Some(rec))
        } else {
            (fixture_services(state), None)
        };

    let run = run_sign_vector(
        services,
        ROOT_1,
        &seed,
        funding,
        &setup_json,
        &sign_args_json,
    )
    .await
    .unwrap_or_else(|e| panic!("{recorded_id}: canonical run failed: {e}"));
    assert_eq!(
        run.outcome.status, "success",
        "{recorded_id}: expected success, got {:?}",
        run.outcome.message
    );
    eprintln!("recorded {recorded_id}: txid={:?}", run.outcome.txid);

    let mut expected = serde_json::to_value(&run.outcome).expect("serialize");

    let mut broadcast = BroadcastRecord {
        sent: false,
        txid: None,
        accepted_by: None,
    };
    if let Some(rec) = recording {
        let captured = rec.captured.lock().expect("captured").clone();
        let accepted = captured
            .iter()
            .find(|r| r.status == "success")
            .unwrap_or_else(|| panic!("{recorded_id}: inline broadcast rejected: {captured:?}"));
        let txid = run.outcome.txid.clone().expect("txid");
        assert!(
            verify_on_network(real, &txid).await,
            "{recorded_id}: {txid} not visible on network"
        );
        broadcast = BroadcastRecord {
            sent: true,
            txid: Some(txid.clone()),
            accepted_by: Some(accepted.name.clone()),
        };
        expected["postBeefResponse"] =
            serde_json::to_value(&captured).expect("postBeefResponse serialize");
        state.broadcasts.push(BroadcastEntry {
            txid,
            purpose: format!("{recorded_id} inline broadcast"),
            from_identity: "root-1 signAction wallet".to_string(),
            sats_out: 100,
            accepted_by: accepted.name.clone(),
            verified_on_network: true,
        });
    }

    // Pin the setup expectations for the replayer.
    let setup_steps: Vec<SetupStep> = setup_json
        .iter()
        .zip(run.setup_outcomes.iter())
        .map(|(args, outcome)| SetupStep {
            create_args: args.clone(),
            reference: outcome.signable_reference.clone(),
            txid: outcome.txid.clone(),
        })
        .collect();

    state.sign_vectors.insert(
        recorded_id.clone(),
        FundedVector {
            id: recorded_id,
            description: uv.description.clone(),
            input: FundedInput {
                root_key: ROOT_1.to_string(),
                funding_set: "SIG".to_string(),
                entropy_seed: seed,
                setup: setup_steps,
                args: sign_args_json,
            },
            expected,
            broadcast,
            notes: plan.notes,
            upstream: Some(serde_json::json!({"id": uv.id, "expected": uv.expected})),
            tags: uv.tags.clone(),
        },
    );
    save_state(state);
}

// ---------------------------------------------------------------------------
// File emission
// ---------------------------------------------------------------------------

fn recording_metadata() -> serde_json::Value {
    serde_json::json!({
        "recorded_at": chrono::Utc::now().to_rfc3339(),
        "chain": "main",
        "recorder": "tests/record_funded_vectors.rs",
        "determinism": "entropy seeded per vector via conformance_entropy (SHA-256 counter \
                        stream over input.entropy_seed); ECDSA nonces are RFC 6979; replay \
                        rebuilds the wallet from the funding fixtures and must reproduce every \
                        recorded byte",
        "caller_key": CALLER_KEY,
        "warning": "root keys and funding fixtures are public; any change remaining on these \
                    keys is forfeit by design",
    })
}

fn emit_files(state: &State) {
    let create_file = FundedFile {
        schema: "../../../schema/vector.schema.json".to_string(),
        id: "wallet.brc100.createaction-funded".to_string(),
        name: "BRC-100 createaction (funded, recorded)".to_string(),
        brc: vec!["BRC-100".to_string()],
        version: "1.0.0".to_string(),
        reference_impl: "bsv-wallet-toolbox (Rust), recorded against BSV mainnet".to_string(),
        parity_class: "required".to_string(),
        recording: recording_metadata(),
        funding_sets: state
            .funding_sets
            .iter()
            .filter(|(k, _)| k.as_str() != "SIG")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        merkle_roots: state.merkle_roots.clone(),
        vectors: state.create_vectors.values().cloned().collect(),
    };
    let sign_file = FundedFile {
        schema: "../../../schema/vector.schema.json".to_string(),
        id: "wallet.brc100.signaction-funded".to_string(),
        name: "BRC-100 signaction (funded, recorded)".to_string(),
        brc: vec!["BRC-100".to_string()],
        version: "1.0.0".to_string(),
        reference_impl: "bsv-wallet-toolbox (Rust), recorded against BSV mainnet".to_string(),
        parity_class: "required".to_string(),
        recording: recording_metadata(),
        funding_sets: state
            .funding_sets
            .iter()
            .filter(|(k, _)| k.as_str() == "SIG")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        merkle_roots: state.merkle_roots.clone(),
        vectors: state.sign_vectors.values().cloned().collect(),
    };
    std::fs::write(
        corpus_dir().join("createaction-funded.json"),
        serde_json::to_string_pretty(&create_file).expect("serialize"),
    )
    .expect("write createaction-funded.json");
    std::fs::write(
        corpus_dir().join("signaction-funded.json"),
        serde_json::to_string_pretty(&sign_file).expect("serialize"),
    )
    .expect("write signaction-funded.json");

    // The ledger: every identity, every funding request, every broadcast.
    let ledger = serde_json::json!({
        "description": "Funding and broadcast ledger for the funded BRC-100 conformance vectors. \
                        Every mainnet action of the recording run, machine-readable.",
        "faucet": {"host": MESSAGEBOX_HOST, "identity": FAUCET_PEER, "sats_per_identity": FAUCET_SATS},
        "identities": state.identities.iter().map(|i| serde_json::json!({
            "name": i.name,
            "identity_key": i.identity_key,
            "private_key": i.private_key,
            "note": "burner identity, fully swept; key published deliberately for reproducibility",
            "faucet_requested_at_ms": i.faucet_requested_at_ms,
            "faucet_txid": i.faucet_txid,
            "faucet_sats": i.faucet_sats,
        })).collect::<Vec<_>>(),
        "broadcasts": state.broadcasts,
    });
    std::fs::write(
        corpus_dir().join("funded-ledger.json"),
        serde_json::to_string_pretty(&ledger).expect("ledger serialize"),
    )
    .expect("write funded-ledger.json");
    eprintln!("emitted corpus files + ledger");
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "spends real satoshis on mainnet; run with BSV_FUNDED_RECORD=1"]
async fn record_funded_vectors() {
    if std::env::var("BSV_FUNDED_RECORD").as_deref() != Ok("1") {
        eprintln!("BSV_FUNDED_RECORD != 1 — refusing to spend real money");
        return;
    }
    let mut state = load_state();
    let real = real_services();

    // Phase 1: identities + faucet
    ensure_identities(&mut state).await;
    ensure_faucet_funding(&mut state).await;

    let id1 = PrivateKey::from_hex(ROOT_1).expect("root1");
    let id2 = PrivateKey::from_hex(ROOT_2).expect("root2");
    let id3 = PrivateKey::from_hex(ROOT_3).expect("root3");
    let id1_pub = id1.to_public_key().to_der_hex();
    let id2_pub = id2.to_public_key().to_der_hex();
    let id3_pub = id3.to_public_key().to_der_hex();

    // Phase 2: sweep faucet grants into wallet(root1) = S1.
    // Incremental: each completed sweep persists before the next starts, so a
    // resume skips identities already swept instead of finding them empty.
    let s1_complete = state
        .funding_sets
        .get("S1")
        .map(|s| s.payments.len() == state.identities.len())
        .unwrap_or(false);
    if !s1_complete {
        state
            .funding_sets
            .entry("S1".to_string())
            .or_insert_with(|| FundingSet { payments: vec![] });
        for ident in state.identities.clone() {
            let already = state.funding_sets["S1"]
                .payments
                .iter()
                .any(|p| p.sender_identity_key == ident.identity_key);
            if already {
                continue;
            }
            let key = PrivateKey::from_hex(&ident.private_key).expect("key");
            let db = db_dir().join(format!("faucet-{}.db", ident.name));
            let setup = WalletBuilder::new()
                .chain(Chain::Main)
                .root_key(key.clone())
                .with_sqlite(db.to_str().expect("path"))
                .with_default_services()
                .without_monitor()
                .build()
                .await
                .expect("wallet");
            let balance = setup.wallet.balance(None).await.expect("balance");
            eprintln!("identity {} balance: {balance}", ident.name);
            if balance == 0 {
                panic!("identity {} has no balance to sweep", ident.name);
            }
            let p = send_max_brc29(
                &setup.wallet,
                &real,
                &key,
                &ident.identity_key,
                &id1_pub,
                balance,
                &format!("funded-conformance sweep {} -> root1", ident.name),
            )
            .await
            .expect("sweep");
            state.broadcasts.push(BroadcastEntry {
                txid: p.txid.clone(),
                purpose: format!("sweep {} -> root1", ident.name),
                from_identity: ident.identity_key.clone(),
                sats_out: p.satoshis,
                accepted_by: "inline create_action broadcast".to_string(),
                verified_on_network: true,
            });
            state
                .funding_sets
                .get_mut("S1")
                .expect("S1 present")
                .payments
                .push(p);
            save_state(&state);
        }
    }
    pin_roots_for_set(&mut state, &real, "S1").await;

    // Phase 3: record root-1 createAction vectors (upstream 1..30)
    let upstream_create = load_upstream("createaction.json");
    for uv in upstream_create.vectors.iter().take(30) {
        let n: usize = uv.id.rsplit('.').next().unwrap().parse().unwrap();
        let (mut args, mut notes) = transform_create_args(&uv.input.args);
        force_delayed_if_immediate(&mut args, &mut notes);
        record_create_vector(
            &mut state,
            uv,
            "S1",
            args,
            notes,
            format!("wallet.brc100.createaction-funded.{n}"),
        )
        .await;
    }
    record_defect_vectors(&mut state, &upstream_create.vectors[0]).await;

    // Phase 4: sweep root1 -> root2 = S2, record 31..60; then root2 -> root3 = S3
    if !state.funding_sets.contains_key("S2") {
        let setup = funded_common::build_vector_wallet(ROOT_1, real.clone())
            .await
            .expect("root1 wallet");
        for p in &state.funding_sets["S1"].payments.clone() {
            funded_common::internalize_funding(&setup, p)
                .await
                .expect("fund");
        }
        let balance = setup.wallet.balance(None).await.expect("balance");
        eprintln!("root1 balance for sweep: {balance}");
        let p = send_max_brc29(
            &setup.wallet,
            &real,
            &id1,
            &id1_pub,
            &id2_pub,
            balance,
            "funded-conformance sweep root1 -> root2",
        )
        .await
        .expect("sweep root1->root2");
        state.broadcasts.push(BroadcastEntry {
            txid: p.txid.clone(),
            purpose: "sweep root1 -> root2".to_string(),
            from_identity: id1_pub.clone(),
            sats_out: p.satoshis,
            accepted_by: "inline create_action broadcast".to_string(),
            verified_on_network: true,
        });
        state
            .funding_sets
            .insert("S2".to_string(), FundingSet { payments: vec![p] });
        save_state(&state);
    }
    pin_roots_for_set(&mut state, &real, "S2").await;

    for uv in upstream_create.vectors.iter().skip(30).take(30) {
        let n: usize = uv.id.rsplit('.').next().unwrap().parse().unwrap();
        let (mut args, mut notes) = transform_create_args(&uv.input.args);
        force_delayed_if_immediate(&mut args, &mut notes);
        record_create_vector(
            &mut state,
            uv,
            "S2",
            args,
            notes,
            format!("wallet.brc100.createaction-funded.{n}"),
        )
        .await;
    }

    if !state.funding_sets.contains_key("S3") {
        let setup = funded_common::build_vector_wallet(ROOT_2, real.clone())
            .await
            .expect("root2 wallet");
        for p in &state.funding_sets["S2"].payments.clone() {
            funded_common::internalize_funding(&setup, p)
                .await
                .expect("fund");
        }
        let balance = setup.wallet.balance(None).await.expect("balance");
        eprintln!("root2 balance for sweep: {balance}");
        let p = send_max_brc29(
            &setup.wallet,
            &real,
            &id2,
            &id2_pub,
            &id3_pub,
            balance,
            "funded-conformance sweep root2 -> root3",
        )
        .await
        .expect("sweep root2->root3");
        state.broadcasts.push(BroadcastEntry {
            txid: p.txid.clone(),
            purpose: "sweep root2 -> root3".to_string(),
            from_identity: id2_pub.clone(),
            sats_out: p.satoshis,
            accepted_by: "inline create_action broadcast".to_string(),
            verified_on_network: true,
        });
        state
            .funding_sets
            .insert("S3".to_string(), FundingSet { payments: vec![p] });
        save_state(&state);
    }
    pin_roots_for_set(&mut state, &real, "S3").await;

    // Phase 5: root-3 vectors. 61 records from S3 and broadcasts; 62 records
    // from S3B (61's change) and broadcasts; the rest record from S3.
    let uv61 = upstream_create
        .vectors
        .iter()
        .find(|v| v.id.ends_with(".61"))
        .expect("61");
    {
        let (mut args, mut notes) = transform_create_args(&uv61.input.args);
        force_delayed_if_immediate(&mut args, &mut notes);
        record_create_vector(
            &mut state,
            uv61,
            "S3",
            args,
            notes,
            "wallet.brc100.createaction-funded.61".to_string(),
        )
        .await;
    }
    broadcast_recorded_vector(
        &mut state,
        &real,
        "wallet.brc100.createaction-funded.61",
        "broadcast chain link 1 (vector 61)",
        &id3_pub,
        1000,
    )
    .await;

    // S3B: vector 61's change, internalized as a self-payment.
    if !state.funding_sets.contains_key("S3B") {
        let v61 = state.create_vectors["wallet.brc100.createaction-funded.61"].clone();
        let txid = v61.expected["txid"].as_str().expect("txid").to_string();
        let beef = v61.expected["tx"].as_str().expect("tx").to_string();
        let change: Vec<serde_json::Value> =
            serde_json::from_value(v61.expected["change"].clone()).expect("change");
        let c = change.first().expect("61 has change");
        state.funding_sets.insert(
            "S3B".to_string(),
            FundingSet {
                payments: vec![FundingPayment {
                    beef,
                    output_index: c["vout"].as_u64().expect("vout") as u32,
                    derivation_prefix: c["derivationPrefix"].as_str().expect("prefix").to_string(),
                    derivation_suffix: c["derivationSuffix"].as_str().expect("suffix").to_string(),
                    sender_identity_key: id3_pub.clone(),
                    satoshis: c["satoshis"].as_u64().expect("sats"),
                    txid,
                    description: "vector 61 change (broadcast on mainnet)".to_string(),
                }],
            },
        );
        save_state(&state);
    }
    pin_roots_for_set(&mut state, &real, "S3B").await;

    let uv62 = upstream_create
        .vectors
        .iter()
        .find(|v| v.id.ends_with(".62"))
        .expect("62");
    {
        // Vector 62 keeps its upstream acceptDelayedBroadcast=false and runs
        // against real services: the recorded run IS the inline broadcast,
        // spending 61's on-chain change (funding set S3B).
        let (args, mut notes) = transform_create_args(&uv62.input.args);
        notes.push(
            "the immediate-broadcast representative: recorded with its upstream \
             acceptDelayedBroadcast=false intact, really broadcast inline during the recorded \
             run (expected.postBeefResponse carries the network's answer for offline replay), \
             funded by vector 61's broadcast change (set S3B)"
                .to_string(),
        );
        record_create_vector_inner(
            &mut state,
            uv62,
            "S3B",
            args,
            notes,
            "wallet.brc100.createaction-funded.62".to_string(),
            Some(real.clone()),
        )
        .await;
    }

    for uv in upstream_create.vectors.iter().skip(60) {
        let n: usize = uv.id.rsplit('.').next().unwrap().parse().unwrap();
        if n == 61 || n == 62 {
            continue;
        }
        let (mut args, mut notes) = transform_create_args(&uv.input.args);
        force_delayed_if_immediate(&mut args, &mut notes);
        record_create_vector(
            &mut state,
            uv,
            "S3",
            args,
            notes,
            format!("wallet.brc100.createaction-funded.{n}"),
        )
        .await;
    }

    // Phase 6: the signAction fixture tx from 62's change, then the 8
    // signAction vectors.
    if state.sig_tx.is_none() {
        let v62 = state.create_vectors["wallet.brc100.createaction-funded.62"].clone();
        let txid62 = v62.expected["txid"].as_str().expect("txid").to_string();
        let beef62 = v62.expected["tx"].as_str().expect("tx").to_string();
        let change: Vec<serde_json::Value> =
            serde_json::from_value(v62.expected["change"].clone()).expect("change");
        let c = change.first().expect("62 has change");

        // Wallet holding 62's change.
        let setup = funded_common::build_vector_wallet(ROOT_3, real.clone())
            .await
            .expect("root3 wallet");
        funded_common::internalize_funding(
            &setup,
            &FundingPayment {
                beef: beef62,
                output_index: c["vout"].as_u64().expect("vout") as u32,
                derivation_prefix: c["derivationPrefix"].as_str().expect("p").to_string(),
                derivation_suffix: c["derivationSuffix"].as_str().expect("s").to_string(),
                sender_identity_key: id3_pub.clone(),
                satoshis: c["satoshis"].as_u64().expect("sats"),
                txid: txid62,
                description: "vector 62 change".to_string(),
            },
        )
        .await
        .expect("internalize 62 change");

        let balance = setup.wallet.balance(None).await.expect("balance");
        eprintln!("root3 balance for signAction fixture: {balance}");
        let caller_key = PrivateKey::from_hex(CALLER_KEY).expect("caller");
        let caller_sats = 300u64;
        let extra = vec![
            CreateActionOutput {
                locking_script: Some(p2pkh_lock(&caller_key)),
                satoshis: caller_sats,
                output_description: "signAction caller input A".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            },
            CreateActionOutput {
                locking_script: Some(p2pkh_lock(&caller_key)),
                satoshis: caller_sats,
                output_description: "signAction caller input B".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            },
        ];
        // Size the BRC-29 wallet output downward until it funds.
        let mut amount = balance.saturating_sub(2 * caller_sats + 2);
        let mut result = None;
        for _ in 0..60 {
            match send_brc29(
                &setup.wallet,
                &real,
                &id3,
                &id3_pub,
                &id1_pub,
                amount,
                "funded-conformance signAction fixture",
                extra.clone(),
            )
            .await
            {
                Ok((p, _)) => {
                    result = Some(p);
                    break;
                }
                Err(e) if e.to_uppercase().contains("INSUFFICIENT") => {
                    amount = amount.saturating_sub(5)
                }
                Err(e) => panic!("signAction fixture tx failed: {e}"),
            }
        }
        let p = result.expect("signAction fixture sized");
        state.broadcasts.push(BroadcastEntry {
            txid: p.txid.clone(),
            purpose: "signAction fixture tx (2 caller P2PKH outputs + wallet funding)".to_string(),
            from_identity: id3_pub.clone(),
            sats_out: p.satoshis + 2 * caller_sats,
            accepted_by: "inline create_action broadcast".to_string(),
            verified_on_network: true,
        });
        state.sig_tx = Some(SigTxInfo {
            txid: p.txid.clone(),
            beef: p.beef.clone(),
            caller_outpoint_a: format!("{}.0", p.txid),
            caller_outpoint_b: format!("{}.1", p.txid),
            caller_sats,
        });
        state
            .funding_sets
            .insert("SIG".to_string(), FundingSet { payments: vec![p] });
        save_state(&state);
    }
    pin_roots_for_set(&mut state, &real, "SIG").await;

    record_sign_vectors(&mut state, &real).await;

    // Phase 7: emit.
    emit_files(&state);

    let recorded = state.create_vectors.len();
    let signs = state.sign_vectors.len();
    eprintln!("DONE: {recorded} createAction vectors, {signs} signAction vectors recorded");
}
