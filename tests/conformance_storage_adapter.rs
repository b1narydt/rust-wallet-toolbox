//! Storage adapter conformance runner.
//!
//! Consumes the vendored corpus embedded at compile time from
//! `conformance/vectors/wallet/storage/adapter-conformance.json` (pinned via
//! `conformance/SOURCE` to bsv-blockchain/ts-stack @ 1920a9c1).
//!
//! The corpus describes an HTTP contract (`/storage/v1/*`) whose reference
//! implementation is the TS `storage/remoting` server. This crate ships a
//! remoting *client* but no HTTP server, so the runner plays the adapter role
//! itself, mirroring the upstream Go dispatcher
//! (go-wallet-toolbox/pkg/storage/v1adapter/handler.go and its
//! adapter_conformance_test.go):
//!
//! - each vector's path + method maps to one `WalletStorageProvider` trait
//!   method, called on a real storage backend (the Go runner used a mocked
//!   provider; here every 200-path executes genuine storage logic against a
//!   seeded SQLite fixture);
//! - request bodies decode leniently, Go-style: route-specific defaults stand
//!   in for Go's zero values and, for createAction, for the TS
//!   `validateCreateActionArgs` defaulting that the reference applies before
//!   storage sees the args (@bsv/sdk validationHelpers.ts:529);
//! - responses reuse the Go envelope: raw JSON for struct results, wrapper
//!   objects (`{"storageName"}`, `{"updated"}`, `{"certificateId"}`,
//!   `{"syncState","isNew"}`) for scalar/tuple results;
//! - assertion semantics are the Go runner's: status must match; bodies whose
//!   expectation carries an `error` key match exactly; success bodies assert
//!   top-level key presence only (the corpus values are illustrative).
//!
//! The 401/400 transport shell (`Authentication required`, `args is
//! required`, `invalid outpoint format`) lives in the adapter in both Go and
//! TS, never in the provider. The runner's dispatcher supplies those exact
//! strings when the corresponding rejection fires; the rejection itself comes
//! from crate code wherever the crate has it (`relinquish_output`'s outpoint
//! parse), and from the dispatcher shim where only a transport layer could
//! check (missing Authorization header, missing `args` key).
//!
//! Divergences are pinned in `KNOWN_DIVERGENCES`: every entry still executes
//! and the suite fails if a pinned divergence disappears, so the ledger can
//! neither grow nor rot silently.

#![cfg(feature = "sqlite")]

mod common;

use async_trait::async_trait;
use chrono::NaiveDateTime;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use bsv::wallet::interfaces::{
    AbortActionArgs, ListActionsArgs, ListOutputsArgs, RelinquishCertificateArgs,
    RelinquishOutputArgs,
};
use bsv_wallet_toolbox::error::WalletError;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::action_types::{
    StorageCreateActionArgs, StorageInternalizeActionArgs, StorageProcessActionArgs,
};
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::sync::request_args::RequestSyncChunkArgs;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::traits::WalletStorageProvider;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::tables::{Certificate, Output, OutputBasket, Transaction};
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
use bsv_wallet_toolbox::wallet::types::AuthId;

use common::MockWalletServices;

const CORPUS: &str = include_str!("../conformance/vectors/wallet/storage/adapter-conformance.json");

/// The identity the Go adapter maps its conformance Bearer token onto
/// (v1adapter/handler.go getIdentityKey). All user-scoped seeding and auth
/// resolution use this identity.
const TEST_IDENTITY: &str = "test-identity-from-vector";

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
    expected: Expected,
    #[serde(default)]
    skip: bool,
}

#[derive(Deserialize)]
struct Input {
    method: String,
    path: String,
    #[serde(default)]
    headers: Map<String, Value>,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Deserialize)]
struct Expected {
    status: u16,
    #[serde(default)]
    body: Option<Value>,
}

fn load_corpus() -> VectorFile {
    let f: VectorFile = serde_json::from_str(CORPUS).expect("corpus JSON must parse");
    assert_eq!(f.id, "wallet.storage.adapterconformance");
    f
}

/// 18 vectors, none corpus-skipped. A refresh that changes this fails here.
#[test]
fn corpus_shape() {
    let f = load_corpus();
    assert_eq!(f.vectors.len(), 18, "vector count changed on refresh");
    assert!(
        f.vectors.iter().all(|v| !v.skip),
        "corpus gained a skip flag; re-verify the runner"
    );
}

// ---------------------------------------------------------------------------
// Divergence ledger
// ---------------------------------------------------------------------------

/// Pinned divergences. Every entry still executes; `assert_known_divergences`
/// fails on any unpinned failure AND on any pinned entry that stops failing.
///
/// - `.6` (processAction): status 200 matches but the response omits
///   `notDelayedResults`. The TS reference broadcasts inside
///   storage processAction (attemptToPostReqsToNetwork) and reports
///   per-txid results; this crate's storage-level `process_action` never
///   broadcasts (no services at that layer) and always returns
///   `not_delayed_results: None`. Real architectural divergence in Rust.
///
/// - `.7` (abortAction): the SDK `AbortActionArgs` deserializes the wire
///   reference (Base64String per BRC-100) into raw BYTES, and
///   `abort_action` looks up `String::from_utf8_lossy(bytes)` — i.e. the
///   base64-DECODED text. Storage stores references as the base64 STRING
///   itself (create_action.rs base64-encodes; the TS reference compares the
///   base64 string as-is). Over the wire, a Rust abort can therefore never
///   find a storage-created action. Rust bug candidate (wire path only;
///   in-process callers that put the base64 string's bytes into
///   `reference` are unaffected).
///
/// - `.8` / `.9` (internalizeAction): the corpus `tx` is 12 bytes and not a
///   valid AtomicBEEF; `Beef::from_binary` rejects it, as would the TS
///   reference (`Beef.fromBinary` throws). These vectors are satisfiable
///   only by Go's fully mocked provider. Corpus authoring gap.
///
/// - `.12` (getSyncChunk): the request omits `toStorageIdentityKey`,
///   `maxRoughSize`, `maxItems` and `offsets` (all required by
///   RequestSyncChunkArgs in TS, Go and Rust; the extra `paged` object
///   exists in no implementation) so `RequestSyncChunkArgs::validate`
///   rejects maxItems=0. The expected body also declares `users` and
///   `syncStates` arrays that exist in no SyncChunk type — the Go runner
///   skips vector 12's body assertion for the same reason. Corpus
///   authoring gap.
///
/// - `.16` (relinquishCertificate): serialNumber "SN-00001-2026" is not
///   base64 and certifier "02certifierkey…" is not hex. The TS reference's
///   own validateRelinquishCertificateArgs (validateBase64String /
///   validateHexString) would throw on both; Rust's typed args
///   (SerialNumber, PublicKey) reject them at deserialization. Only Go's
///   plain-string decode + mock accepts. Corpus authoring gap.
const KNOWN_DIVERGENCES: &[&str] = &[
    "wallet.storage.adapterconformance.6",
    "wallet.storage.adapterconformance.7",
    "wallet.storage.adapterconformance.8",
    "wallet.storage.adapterconformance.9",
    "wallet.storage.adapterconformance.12",
    "wallet.storage.adapterconformance.16",
];

/// Vectors whose 200-path depends on backend-specific seeding (direct table
/// writes into the SQLite fixture). A future MPC-backed run cannot seed this
/// way and must provision equivalent state through its own means before these
/// vectors can be expected to pass.
#[allow(dead_code)]
const BACKEND_SEEDED_VECTORS: &[&str] = &[
    "wallet.storage.adapterconformance.4",  // funded change UTXO
    "wallet.storage.adapterconformance.6",  // existing unsigned action
    "wallet.storage.adapterconformance.7",  // existing unsigned action
    "wallet.storage.adapterconformance.17", // existing output at the outpoint
];

// ---------------------------------------------------------------------------
// The seam: backend under test
// ---------------------------------------------------------------------------

/// Yields a fresh, isolated storage-under-test per vector, with that vector's
/// precondition state applied. The runner and dispatcher only ever touch the
/// public `WalletStorageProvider` interface; everything backend-specific
/// (construction, direct-table seeding) lives behind this trait so an
/// MPC-backed wallet storage can be substituted later without touching the
/// runner or the assertions.
#[async_trait]
trait AdapterBackend {
    type Storage: WalletStorageProvider + Send + Sync;
    async fn fresh(&self, vector_id: &str) -> Self::Storage;
}

struct SqliteBackend;

#[async_trait]
impl AdapterBackend for SqliteBackend {
    type Storage = SqliteStorage;

    async fn fresh(&self, vector_id: &str) -> SqliteStorage {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test)
            .await
            .expect("storage");
        storage.migrate_database().await.expect("migrate");
        StorageProvider::make_available(&storage)
            .await
            .expect("available");
        seed(&storage, vector_id).await;
        storage
    }
}

// ---------------------------------------------------------------------------
// SQLite seeding (backend-specific; see BACKEND_SEEDED_VECTORS)
// ---------------------------------------------------------------------------

fn now() -> NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// The corpus's illustrative txids, reused as seeded state so vectors that
/// reference them find real rows.
const TXID_A: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const P2PKH_SCRIPT: &str = "76a914f54a5851e9372b87810a8e60cdd2e7cfd80b6e5388ac";
/// base64("testreference") — the reference string vectors 6 and 7 carry.
/// Stored as-is: both this crate's create_action and the TS reference store
/// references as base64 STRINGS.
const REFERENCE_B64: &str = "dGVzdHJlZmVyZW5jZQ==";

async fn seed(storage: &SqliteStorage, vector_id: &str) {
    // Every vector resolves auth against this user (Go resolveAuthID).
    let (user, _) = StorageReaderWriter::find_or_insert_user(storage, TEST_IDENTITY, None)
        .await
        .expect("seed user");
    let user_id = user.user_id;

    let basket_id = StorageReaderWriter::insert_output_basket(
        storage,
        &OutputBasket {
            created_at: now(),
            updated_at: now(),
            basket_id: 0,
            user_id,
            name: "default".to_string(),
            number_of_desired_utxos: 10,
            minimum_desired_utxo_value: 1000,
            is_deleted: false,
        },
        None,
    )
    .await
    .expect("seed basket");

    let seed_completed_tx = |reference: String, satoshis: i64| Transaction {
        created_at: now(),
        updated_at: now(),
        transaction_id: 0,
        user_id,
        proven_tx_id: None,
        status: TransactionStatus::Completed,
        reference,
        is_outgoing: false,
        satoshis,
        description: "conformance seed".to_string(),
        version: Some(1),
        lock_time: Some(0),
        txid: Some(TXID_A.to_string()),
        input_beef: None,
        raw_tx: None,
    };

    match vector_id {
        // Funded change UTXO so the real create_action can allocate an input
        // and produce change (vector's illustrative source: 200_000 sats at
        // TXID_A:0 with the P2PKH script).
        "wallet.storage.adapterconformance.4" => {
            let tx_id = StorageReaderWriter::insert_transaction(
                storage,
                &seed_completed_tx("seed-funding".to_string(), 200_000),
                None,
            )
            .await
            .expect("seed funding tx");
            let script = hex_bytes(P2PKH_SCRIPT);
            StorageReaderWriter::insert_output(
                storage,
                &Output {
                    created_at: now(),
                    updated_at: now(),
                    output_id: 0,
                    user_id,
                    transaction_id: tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: true,
                    output_description: Some("seed change".to_string()),
                    vout: 0,
                    satoshis: 200_000,
                    provided_by: StorageProvidedBy::Storage,
                    purpose: "change".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(TXID_A.to_string()),
                    sender_identity_key: None,
                    derivation_prefix: Some("dGVzdHByZWZpeA==".to_string()),
                    derivation_suffix: Some("dGVzdHN1ZmZpeA==".to_string()),
                    custom_instructions: None,
                    spent_by: None,
                    sequence_number: None,
                    spending_description: None,
                    script_length: Some(script.len() as i64),
                    script_offset: None,
                    locking_script: Some(script),
                },
                None,
            )
            .await
            .expect("seed change output");
        }
        // An unsigned outgoing action carrying the vectors' reference, as
        // create_action would have left it (reference stored as the base64
        // string), ready to be processed (6) or aborted (7).
        "wallet.storage.adapterconformance.6" | "wallet.storage.adapterconformance.7" => {
            let mut tx = seed_completed_tx(REFERENCE_B64.to_string(), -100_250);
            tx.status = TransactionStatus::Unsigned;
            tx.is_outgoing = true;
            tx.txid = None;
            StorageReaderWriter::insert_transaction(storage, &tx, None)
                .await
                .expect("seed unsigned action");
        }
        // An output at the exact outpoint vector 17 relinquishes.
        "wallet.storage.adapterconformance.17" => {
            let tx_id = StorageReaderWriter::insert_transaction(
                storage,
                &seed_completed_tx("seed-relinquish".to_string(), 99_750),
                None,
            )
            .await
            .expect("seed tx");
            StorageReaderWriter::insert_output(
                storage,
                &Output {
                    created_at: now(),
                    updated_at: now(),
                    output_id: 0,
                    user_id,
                    transaction_id: tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: false,
                    output_description: None,
                    vout: 0,
                    satoshis: 99_750,
                    provided_by: StorageProvidedBy::You,
                    purpose: String::new(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(TXID_A.to_string()),
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
                },
                None,
            )
            .await
            .expect("seed output");
        }
        // getSyncChunk selects rows by the identityKey claimed in the request
        // body, not the transport identity.
        "wallet.storage.adapterconformance.12" => {
            StorageReaderWriter::find_or_insert_user(
                storage,
                "02a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                None,
            )
            .await
            .expect("seed sync user");
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Dispatcher (the adapter role; mirrors go v1adapter/handler.go)
// ---------------------------------------------------------------------------

/// Merge `provided` over `defaults`: objects merge recursively, everything
/// else `provided` wins. Stands in for Go's zero-value JSON decode (missing
/// fields become the template's values) without touching the crate's strict
/// serde types.
fn overlay(defaults: Value, provided: &Value) -> Value {
    match (defaults, provided) {
        (Value::Object(mut d), Value::Object(p)) => {
            for (k, v) in p {
                let merged = match d.remove(k) {
                    Some(dv) => overlay(dv, v),
                    None => v.clone(),
                };
                d.insert(k.clone(), merged);
            }
            Value::Object(d)
        }
        (_, p) => p.clone(),
    }
}

fn err_body(msg: &str) -> Value {
    json!({ "error": msg })
}

fn ok<T: serde::Serialize>(v: &T) -> (u16, Value) {
    (200, serde_json::to_value(v).expect("result serializes"))
}

/// Map a provider error the way the Go adapter maps its own pre-checks and
/// provider failures: the outpoint-format rejection (which lives in crate
/// code here, in the adapter in Go) becomes the canonical 400; everything
/// else is a 500 with the error text.
fn provider_err(e: WalletError) -> (u16, Value) {
    if let WalletError::InvalidParameter { ref parameter, .. } = e {
        if parameter == "output" {
            return (400, err_body("invalid outpoint format"));
        }
    }
    (500, err_body(&e.to_string()))
}

async fn dispatch<S>(storage: &S, v: &Vector) -> (u16, Value)
where
    S: WalletStorageProvider + Send + Sync,
{
    // Transport shell: the Go adapter's auth middleware rejects requests
    // without credentials before any provider code runs. This crate has no
    // HTTP surface, so this check exercises the runner's shim only; it
    // documents the contract for a future Rust storage server.
    if !v.input.headers.contains_key("Authorization") {
        return (401, err_body("Authentication required"));
    }

    let (user, _) = match storage.find_or_insert_user(TEST_IDENTITY).await {
        Ok(u) => u,
        Err(e) => return provider_err(e),
    };
    let auth = AuthId {
        identity_key: TEST_IDENTITY.to_string(),
        user_id: Some(user.user_id),
        is_active: Some(true),
    };
    let body = v.input.body.clone().unwrap_or(Value::Null);
    let route = (v.input.method.as_str(), v.input.path.as_str());

    match route {
        ("GET", "/storage/v1/settings") => match storage.make_available().await {
            Ok(s) => ok(&s),
            Err(e) => provider_err(e),
        },
        ("POST", "/storage/v1/migrate") => {
            let name = body["storageName"].as_str().unwrap_or_default();
            let key = body["storageIdentityKey"].as_str().unwrap_or_default();
            match storage.migrate(name, key).await {
                Ok(n) => (200, json!({ "storageName": n })),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/actions") => {
            // Go: missing "args" key is a 400 before any decode.
            let Some(args_raw) = body.get("args") else {
                return (400, err_body("args is required"));
            };
            // The reference applies validateCreateActionArgs before storage
            // sees the args; replicate its defaulting and derived flags
            // (@bsv/sdk validationHelpers.ts:529-557) over the wire shape.
            let merged = overlay(create_action_defaults(), args_raw);
            let merged = with_derived_create_flags(merged);
            let args: StorageCreateActionArgs = match serde_json::from_value(merged) {
                Ok(a) => a,
                Err(e) => return (400, err_body(&format!("invalid args for createAction: {e}"))),
            };
            match storage.create_action(&auth, &args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/actions/process") => {
            let args: StorageProcessActionArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for processAction: {e}")),
                    )
                }
            };
            match storage.process_action(&auth, &args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/actions/abort") => {
            let args: AbortActionArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for abortAction: {e}")),
                    )
                }
            };
            match storage.abort_action(&auth, &args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/actions/internalize") => {
            let args: StorageInternalizeActionArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for internalizeAction: {e}")),
                    )
                }
            };
            match storage
                .internalize_action(&auth, &args, &MockWalletServices)
                .await
            {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/list/actions") => {
            let args: ListActionsArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for listActions: {e}")),
                    )
                }
            };
            match storage.list_actions(&auth, &args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/list/outputs") => {
            let args: ListOutputsArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for listOutputs: {e}")),
                    )
                }
            };
            match storage.list_outputs(&auth, &args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/certificates") => {
            // Go decodes into TableCertificateX where every absent column is
            // its zero value; mirror that for the crate's strict table type.
            let merged = overlay(certificate_defaults(), &body);
            let cert: Certificate = match serde_json::from_value(merged) {
                Ok(c) => c,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for insertCertificate: {e}")),
                    )
                }
            };
            match storage.insert_certificate_auth(&auth, &cert).await {
                Ok(id) => (200, json!({ "certificateId": id })),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/certificates/relinquish") => {
            let args: RelinquishCertificateArgs = match serde_json::from_value(body) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for relinquishCertificate: {e}")),
                    )
                }
            };
            match storage.relinquish_certificate(&auth, &args).await {
                Ok(n) => (200, json!({ "updated": n })),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/outputs/relinquish") => {
            // Both the TS and Rust providers ignore `basket`; the corpus
            // omits it. Default it Go-style so the crate's own outpoint
            // validation (not serde) decides vectors 17/18.
            let merged = overlay(json!({ "basket": "" }), &body);
            let args: RelinquishOutputArgs = match serde_json::from_value(merged) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for relinquishOutput: {e}")),
                    )
                }
            };
            match storage.relinquish_output(&auth, &args).await {
                Ok(n) => (200, json!({ "updated": n })),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/sync/active") => {
            let key = body["newActiveStorageIdentityKey"]
                .as_str()
                .unwrap_or_default();
            match storage.set_active(&auth, key).await {
                Ok(n) => (200, json!({ "updated": n })),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/sync/chunk") => {
            // Go zero-decodes the missing budget fields; RequestSyncChunkArgs
            // requires them, so default them to Go's zeros and let the
            // crate's validate() judge the result.
            let merged = overlay(
                json!({
                    "toStorageIdentityKey": "",
                    "maxRoughSize": 0,
                    "maxItems": 0,
                    "offsets": [],
                }),
                &body,
            );
            let args: RequestSyncChunkArgs = match serde_json::from_value(merged) {
                Ok(a) => a,
                Err(e) => {
                    return (
                        400,
                        err_body(&format!("invalid JSON body for syncChunk: {e}")),
                    )
                }
            };
            match storage.get_sync_chunk(&args).await {
                Ok(r) => ok(&r),
                Err(e) => provider_err(e),
            }
        }
        ("POST", "/storage/v1/sync/state") => {
            let key = body["storageIdentityKey"].as_str().unwrap_or_default();
            let name = body["storageName"].as_str().unwrap_or_default();
            match storage.find_or_insert_sync_state_auth(&auth, key, name).await {
                Ok((state, is_new)) => (
                    200,
                    json!({
                        "syncState": serde_json::to_value(&state).expect("sync state serializes"),
                        "isNew": is_new,
                    }),
                ),
                Err(e) => provider_err(e),
            }
        }
        (m, p) => panic!("unmapped route {m} {p} — extend the dispatcher, don't skip"),
    }
}

/// TS validateCreateActionArgs defaults for the wire fields the corpus omits
/// (defaultOne(version), defaultZero(lockTime), defaultEmpty(inputs/labels))
/// plus the fully-defaulted ValidCreateActionOptions.
fn create_action_defaults() -> Value {
    json!({
        "inputs": [],
        "lockTime": 0,
        "version": 1,
        "labels": [],
        "isNewTx": false,
        "isSignAction": false,
        "isNoSend": false,
        "isDelayed": false,
        "isSendWith": false,
        "isRemixChange": false,
        "includeAllSourceTransactions": false,
        "options": {
            "signAndProcess": true,
            "acceptDelayedBroadcast": true,
            "trustSelf": null,
            "returnTxidOnly": false,
            "noSend": false,
            "randomizeOutputs": true,
        },
    })
}

/// The derived flags validateCreateActionArgs computes after defaulting
/// (validationHelpers.ts:550-557).
fn with_derived_create_flags(mut v: Value) -> Value {
    let inputs_n = v["inputs"].as_array().map_or(0, Vec::len);
    let outputs_n = v["outputs"].as_array().map_or(0, Vec::len);
    let is_send_with = v["options"]["sendWith"].as_array().is_some_and(|a| !a.is_empty());
    let is_remix = !is_send_with && inputs_n == 0 && outputs_n == 0;
    let is_new_tx = is_remix || inputs_n > 0 || outputs_n > 0;
    let sign_and_process = v["options"]["signAndProcess"].as_bool().unwrap_or(true);
    v["isSendWith"] = json!(is_send_with);
    v["isRemixChange"] = json!(is_remix);
    v["isNewTx"] = json!(is_new_tx);
    v["isSignAction"] = json!(is_new_tx && !sign_and_process);
    v["isDelayed"] = v["options"]["acceptDelayedBroadcast"].clone();
    v["isNoSend"] = v["options"]["noSend"].clone();
    v
}

/// Zero-value template for the crate's strict Certificate table type.
fn certificate_defaults() -> Value {
    json!({
        "created_at": "2026-01-01T00:00:00.000Z",
        "updated_at": "2026-01-01T00:00:00.000Z",
        "certificateId": 0,
        "userId": 0,
        "type": "",
        "serialNumber": "",
        "certifier": "",
        "subject": "",
        "revocationOutpoint": "",
        "signature": "",
        "isDeleted": false,
    })
}

// ---------------------------------------------------------------------------
// Assertions (mirror the Go runner's semantics)
// ---------------------------------------------------------------------------

/// Status must match. Error expectations (body carries an `error` key) match
/// exactly; success expectations assert top-level key presence only, because
/// the corpus values are illustrative (the Go runner's assertResponseBody).
fn check_vector(v: &Vector, status: u16, body: &Value, failures: &mut Vec<String>) {
    if status != v.expected.status {
        failures.push(format!(
            "{}: status: expected {}, observed {} (body: {})",
            v.id, v.expected.status, status, body
        ));
        return;
    }
    let Some(expected_body) = &v.expected.body else {
        return;
    };
    let expected_obj = expected_body.as_object().expect("expected body object");

    if expected_obj.contains_key("error") {
        if body != expected_body {
            failures.push(format!(
                "{}: error body: expected {}, observed {}",
                v.id, expected_body, body
            ));
        }
        return;
    }

    let got_obj = match body.as_object() {
        Some(o) => o,
        None => {
            failures.push(format!("{}: response body is not an object: {}", v.id, body));
            return;
        }
    };
    let missing: Vec<&str> = expected_obj
        .keys()
        .filter(|k| !got_obj.contains_key(*k))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        failures.push(format!(
            "{}: response missing expected keys {:?} (observed: {})",
            v.id, missing, body
        ));
    }
}

/// Every failure must correspond to exactly the pinned divergence set: a new
/// failure breaks the build, and a divergence that stops failing (someone
/// fixed it, or the corpus changed) also breaks the build until its ledger
/// entry is removed.
fn assert_known_divergences(failures: &[String], known: &[&str]) {
    let hit = |k: &str, f: &str| {
        // Exact-id prefix match: ".1" must not swallow ".12"'s failures.
        f.starts_with(k) && f.as_bytes().get(k.len()) == Some(&b':')
    };
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| !known.iter().any(|k| hit(k, f)))
        .collect();
    let resolved: Vec<&&str> = known
        .iter()
        .filter(|k| !failures.iter().any(|f| hit(k, f)))
        .collect();
    assert!(
        unexpected.is_empty() && resolved.is_empty(),
        "divergence ledger out of date.\nUnexpected failures:\n{}\nResolved (remove from ledger):\n{}\nAll failures:\n{}",
        unexpected.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"),
        resolved.iter().map(|k| format!("  {k}")).collect::<Vec<_>>().join("\n"),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

async fn run_channel<B: AdapterBackend>(backend: &B) {
    let corpus = load_corpus();
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &corpus.vectors {
        assert!(!v.skip, "{}: unexpected corpus skip", v.id);
        executed += 1;
        let storage = backend.fresh(&v.id).await;
        let (status, body) = dispatch(&storage, v).await;
        check_vector(v, status, &body, &mut failures);
    }

    assert_eq!(executed, 18, "all 18 adapter vectors must execute");
    // Always visible with --nocapture: the observed text of every pinned
    // divergence, so the ledger's explanations can be audited against reality.
    for f in &failures {
        eprintln!("DIVERGENCE {f}");
    }
    assert_known_divergences(&failures, KNOWN_DIVERGENCES);
}

#[tokio::test]
async fn adapter_conformance_channel() {
    run_channel(&SqliteBackend).await;
}
