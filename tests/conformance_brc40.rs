//! BRC-40 sync conformance runner.
//!
//! Consumes the vendored corpus embedded at compile time from
//! `conformance/vectors/sync/brc40-user-state.json` (pinned via
//! `conformance/SOURCE` to bsv-blockchain/ts-stack @ 8b074a06).
//!
//! Dispatcher contract (mirrors the Go runner in
//! go-wallet-toolbox/pkg/internal/storage/repo/syncrepo/brc40_conformance_test.go
//! and ts-stack/conformance/runner/ts/dispatchers/sync.ts):
//!
//! - `brc40/requestSyncChunk`: the `message` is a `RequestSyncChunkArgs` wire
//!   object. `valid: true` means the producer must accept it (deserialize +
//!   validate); `valid: false` means it must reject.
//! - `brc40/syncChunk`: the `message` is a `SyncChunk` wire object. Valid
//!   chunks must parse; the all-arrays-present-and-empty sentinel must produce
//!   `done: true`; malformed chunks must be rejected.
//! - `brc40/mergeExisting`: seed `existing` via a first chunk, apply
//!   `incoming` via a second chunk through `process_sync_chunk`, and compare
//!   the surviving row to `expected.action` ("update" or "skip").
//! - `brc40/flow`: replay `messages[].syncChunk` in order against one storage
//!   and one `SyncMap`, then assert the post-merge state.
//!
//! Divergences from the corpus that we deliberately do not paper over are
//! pinned in `KNOWN_DIVERGENCES`: each entry still executes, its observed
//! behavior is asserted, and the test fails if the divergence disappears — so
//! a fix upstream forces the entry's removal and the list cannot grow or rot
//! silently.

#![cfg(feature = "sqlite")]

use chrono::NaiveDateTime;
use serde::Deserialize;
use serde_json::Value;

use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::find_args::*;
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::sync::process_sync_chunk::{
    process_sync_chunk, AuthenticatedIdentityKey,
};
use bsv_wallet_toolbox::storage::sync::request_args::RequestSyncChunkArgs;
use bsv_wallet_toolbox::storage::sync::sync_map::{SyncChunk, SyncMap};
// CRUD comes from StorageReader/StorageReaderWriter; WalletStorageProvider
// (the BRC-40 wire surface) is called via full-path UFCS because importing it
// would make the shared method names ambiguous.
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::tables::*;
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

const CORPUS: &str = include_str!("../conformance/vectors/sync/brc40-user-state.json");

/// The user identity key used across the corpus's flow/response vectors.
const USER_KEY: &str = "02cccc00000000000000000000000000000000000000000000000000000000cccc";

// ---------------------------------------------------------------------------
// Corpus model
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct VectorFile {
    id: String,
    parity_class: String,
    skip_reason: Option<String>,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: Input,
    expected: Expected,
    #[serde(default)]
    skip: bool,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct Input {
    channel: String,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    existing: Option<Value>,
    #[serde(default)]
    incoming: Option<Value>,
    #[serde(default)]
    messages: Option<Vec<Value>>,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    request: Option<Value>,
    #[serde(default)]
    response: Option<Value>,
}

#[derive(Deserialize)]
struct Expected {
    #[serde(default)]
    valid: Option<bool>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default, rename = "finalState")]
    final_state: Option<Value>,
    #[serde(default)]
    done: Option<bool>,
}

fn load_corpus() -> VectorFile {
    let f: VectorFile = serde_json::from_str(CORPUS).expect("corpus JSON must parse");
    assert_eq!(f.id, "sync.brc40");
    f
}

fn vectors_for(channel: &str) -> Vec<Vector> {
    load_corpus()
        .vectors
        .into_iter()
        .filter(|v| v.input.channel == channel)
        .collect()
}

/// The upstream corpus can govern a runtime-only assertion at file level.
/// The vector itself identifies when that inherited classification applies;
/// avoid pinning a vector ID or a per-vector skip count in this runner.
fn governed_by_file_classification(file: &VectorFile, vector: &Vector) -> bool {
    file.parity_class == "intended"
        && file
            .skip_reason
            .as_deref()
            .is_some_and(|reason| !reason.is_empty())
        && vector.notes.as_deref().is_some_and(|notes| {
            notes.contains("governed by the file-level intended classification")
        })
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

async fn setup_storage() -> SqliteStorage {
    let config = StorageConfig {
        url: "sqlite::memory:".to_string(),
        ..Default::default()
    };
    let storage = SqliteStorage::new_sqlite(config, Chain::Test)
        .await
        .expect("storage");
    storage.migrate_database().await.expect("migrate");
    storage.make_available().await.expect("available");
    storage
}

fn parse_iso(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f")
        .unwrap_or_else(|e| panic!("bad ISO timestamp {s:?}: {e}"))
}

fn s(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("field {key} missing/not a string in {v}"))
        .to_string()
}

fn i(v: &Value, key: &str) -> i64 {
    v[key]
        .as_i64()
        .unwrap_or_else(|| panic!("field {key} missing/not an int in {v}"))
}

fn status(v: &Value) -> TransactionStatus {
    match v["status"].as_str() {
        Some("completed") => TransactionStatus::Completed,
        Some("unsigned") => TransactionStatus::Unsigned,
        other => panic!("unmapped vector status {other:?} — extend the runner, don't skip"),
    }
}

fn empty_chunk() -> SyncChunk {
    SyncChunk {
        from_storage_identity_key:
            "02aaaa00000000000000000000000000000000000000000000000000000000aaaa".to_string(),
        to_storage_identity_key:
            "02bbbb00000000000000000000000000000000000000000000000000000000bbbb".to_string(),
        user_identity_key: USER_KEY.to_string(),
        user: None,
        proven_txs: None,
        output_baskets: None,
        transactions: None,
        outputs: None,
        tx_labels: None,
        tx_label_maps: None,
        output_tags: None,
        output_tag_maps: None,
        certificates: None,
        certificate_fields: None,
        commissions: None,
        proven_tx_reqs: None,
    }
}

/// Build a `tables::Transaction` from a vector row. Fields the corpus row
/// doesn't carry get fixed defaults; the natural key (reference) is
/// synthesized from the producer-side transactionId, mirroring the Go runner.
fn tx_row(row: &Value, user_id: i64) -> Transaction {
    Transaction {
        created_at: parse_iso(&s(row, "created_at")),
        updated_at: parse_iso(&s(row, "updated_at")),
        transaction_id: i(row, "transactionId"),
        user_id,
        proven_tx_id: row["provenTxId"].as_i64(),
        status: status(row),
        reference: format!("vector-tx-{}", i(row, "transactionId")),
        is_outgoing: false,
        satoshis: 1,
        description: "conformance".to_string(),
        version: Some(1),
        lock_time: Some(0),
        txid: None,
        input_beef: None,
        raw_tx: None,
    }
}

/// Build a `tables::ProvenTx` from a vector row. `block_hash` is a
/// runner-synthesized marker used to detect which side survived the merge
/// (the corpus rows carry identical merklePath/height on both sides).
fn proven_row(row: &Value, block_hash_marker: &str) -> ProvenTx {
    ProvenTx {
        created_at: parse_iso(&s(row, "created_at")),
        updated_at: parse_iso(&s(row, "updated_at")),
        proven_tx_id: i(row, "provenTxId"),
        txid: s(row, "txid"),
        height: row["height"].as_i64().unwrap_or(1) as i32,
        index: 0,
        merkle_path: row["merklePath"].as_str().unwrap_or("").as_bytes().to_vec(),
        raw_tx: vec![0u8; 4],
        block_hash: block_hash_marker.to_string(),
        merkle_root: "root".to_string(),
    }
}

/// Build a `tables::Output` from a vector row.
fn output_row(row: &Value, user_id: i64) -> Output {
    Output {
        created_at: parse_iso(&s(row, "created_at")),
        updated_at: parse_iso(&s(row, "updated_at")),
        output_id: i(row, "outputId"),
        user_id,
        transaction_id: i(row, "transactionId"),
        basket_id: None,
        spendable: row["spendable"].as_bool().expect("spendable"),
        change: false,
        output_description: None,
        vout: i(row, "vout") as i32,
        satoshis: i(row, "satoshis"),
        provided_by: StorageProvidedBy::You,
        purpose: String::new(),
        output_type: "P2PKH".to_string(),
        txid: None,
        sender_identity_key: None,
        derivation_prefix: None,
        derivation_suffix: None,
        custom_instructions: None,
        spent_by: row["spentBy"].as_i64(),
        sequence_number: None,
        spending_description: None,
        script_length: None,
        script_offset: None,
        locking_script: None,
    }
}

async fn find_tx_by_reference(
    storage: &SqliteStorage,
    user_id: i64,
    reference: &str,
) -> Transaction {
    let rows = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user_id),
                    reference: Some(reference.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("find_transactions");
    assert_eq!(rows.len(), 1, "expected exactly one row for {reference}");
    rows.into_iter().next().unwrap()
}

// ---------------------------------------------------------------------------
// Corpus shape
// ---------------------------------------------------------------------------

/// 24 vectors across 4 channels. Upstream classifies the file as intended
/// because runtime producer assertions require seeded state; individual
/// vectors no longer carry `skip: true`. A refresh that changes this shape
/// fails here and the per-channel counts below must be re-verified.
#[test]
fn corpus_shape() {
    let f = load_corpus();
    assert_eq!(f.vectors.len(), 24, "vector count changed on refresh");
    assert_eq!(f.parity_class, "intended");
    assert!(
        f.skip_reason
            .as_deref()
            .is_some_and(|reason| !reason.is_empty()),
        "intended file classification must explain the governed skip"
    );
    let count = |c: &str| f.vectors.iter().filter(|v| v.input.channel == c).count();
    assert_eq!(count("brc40/requestSyncChunk"), 9);
    assert_eq!(count("brc40/syncChunk"), 5);
    assert_eq!(count("brc40/mergeExisting"), 6);
    assert_eq!(count("brc40/flow"), 4);
    assert!(
        f.vectors.iter().all(|v| !v.skip),
        "per-vector skips returned; re-check the upstream classification"
    );
}

// ---------------------------------------------------------------------------
// brc40/requestSyncChunk — producer must accept valid args, reject malformed
// ---------------------------------------------------------------------------

#[test]
fn request_sync_chunk_channel() {
    let file = load_corpus();
    let vectors: Vec<&Vector> = file
        .vectors
        .iter()
        .filter(|v| v.input.channel == "brc40/requestSyncChunk")
        .collect();
    assert_eq!(vectors.len(), 9);
    let mut executed = 0usize;
    let mut governed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &vectors {
        if governed_by_file_classification(&file, v) {
            governed += 1;
            continue;
        }
        executed += 1;
        let message = v.input.message.as_ref().expect("request vector message");
        let accepted = serde_json::from_value::<RequestSyncChunkArgs>(message.clone())
            .map_err(|e| e.to_string())
            .and_then(|args| args.validate().map_err(|e| e.to_string()));
        let want_valid = v.expected.valid.expect("request vector valid flag");

        match (want_valid, &accepted) {
            (true, Err(e)) => failures.push(format!("{}: expected accept, got reject: {e}", v.id)),
            (false, Ok(())) => failures.push(format!("{}: expected reject, got accept", v.id)),
            _ => {}
        }
    }

    assert!(governed > 0, "file-level classification governs no vector");
    assert_eq!(
        executed + governed,
        vectors.len(),
        "every request vector must execute or be governed by upstream metadata"
    );
    assert!(
        failures.is_empty(),
        "requestSyncChunk divergences:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// brc40/syncChunk — consumer-side wire acceptance and the done sentinel
// ---------------------------------------------------------------------------

/// sync.brc40.response.1 (valid: true) currently FAILS in Rust: the corpus's
/// schema-minimal rows (a provenTx with only created_at/updated_at/
/// provenTxId/txid; a transaction without reference/satoshis/description)
/// are rejected by this crate's strict table deserialization, which requires
/// every column. BRC-40 only mandates created_at/updated_at per record. The
/// live TS producer always serializes full table rows, so the practical
/// exposure is limited to schema-minimal third-party producers — but it is a
/// real divergence from the corpus, recorded here rather than papered over.
const KNOWN_SYNC_CHUNK_DIVERGENCES: &[&str] = &["sync.brc40.response.1"];

#[tokio::test]
async fn sync_chunk_channel() {
    let vectors = vectors_for("brc40/syncChunk");
    assert_eq!(vectors.len(), 5);
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &vectors {
        assert!(!v.skip, "{}: unexpected corpus skip", v.id);
        executed += 1;
        let message = v.input.message.as_ref().expect("syncChunk vector message");
        let want_valid = v.expected.valid.expect("syncChunk vector valid flag");

        let parsed = serde_json::from_value::<SyncChunk>(message.clone());

        match (want_valid, parsed) {
            (true, Err(e)) => failures.push(format!("{}: expected parse, got: {e}", v.id)),
            (false, Err(_)) => {} // rejected at the wire, as required
            (false, Ok(chunk)) => {
                // Parsed structurally — the rejection must then come from the
                // consumer. Drive the full consumer path with the vector's
                // request context (response.error.3: user mismatch).
                let request_identity = v
                    .input
                    .request
                    .as_ref()
                    .and_then(|r| r["identityKey"].as_str())
                    .unwrap_or(USER_KEY)
                    .to_string();
                let storage = setup_storage().await;
                let args = request_args_for(&request_identity);
                if bsv_wallet_toolbox::storage::traits::WalletStorageProvider::process_sync_chunk(
                    &storage, &args, &chunk,
                )
                .await
                .is_ok()
                {
                    failures.push(format!(
                        "{}: consumer accepted a chunk it must reject",
                        v.id
                    ));
                }
            }
            (true, Ok(chunk)) => {
                if let Some(want_done) = v.expected.done {
                    let storage = setup_storage().await;
                    let args = request_args_for(USER_KEY);
                    let r = bsv_wallet_toolbox::storage::traits::WalletStorageProvider::process_sync_chunk(
                        &storage, &args, &chunk,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
                    if r.done != want_done {
                        failures.push(format!(
                            "{}: done sentinel: expected {want_done}, got {}",
                            v.id, r.done
                        ));
                    }
                }
            }
        }
    }

    assert_eq!(executed, 5, "all 5 syncChunk vectors must execute");
    assert_known_divergences("syncChunk", &failures, KNOWN_SYNC_CHUNK_DIVERGENCES);
}

fn request_args_for(identity_key: &str) -> RequestSyncChunkArgs {
    RequestSyncChunkArgs {
        from_storage_identity_key:
            "02aaaa00000000000000000000000000000000000000000000000000000000aaaa".to_string(),
        to_storage_identity_key:
            "02bbbb00000000000000000000000000000000000000000000000000000000bbbb".to_string(),
        identity_key: identity_key.to_string(),
        since: None,
        max_rough_size: 10_000_000,
        max_items: 1000,
        offsets: [
            "provenTx",
            "outputBasket",
            "outputTag",
            "txLabel",
            "transaction",
            "output",
            "txLabelMap",
            "outputTagMap",
            "certificate",
            "certificateField",
            "commission",
            "provenTxReq",
        ]
        .into_iter()
        .map(
            |name| bsv_wallet_toolbox::storage::sync::request_args::SyncChunkOffset {
                name: name.to_string(),
                offset: 0,
            },
        )
        .collect(),
    }
}

/// Every failure must correspond to exactly the pinned divergence set: a new
/// failure breaks the build, and a divergence that stops failing (someone
/// fixed it) also breaks the build until its ledger entry is removed.
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

// ---------------------------------------------------------------------------
// brc40/mergeExisting — seed existing, apply incoming, compare to action
// ---------------------------------------------------------------------------

#[tokio::test]
async fn merge_existing_channel() {
    let vectors = vectors_for("brc40/mergeExisting");
    assert_eq!(vectors.len(), 6);
    let mut executed = 0usize;

    for v in &vectors {
        assert!(!v.skip, "{}: unexpected corpus skip", v.id);
        executed += 1;
        match v.input.entity.as_deref() {
            Some("transactions") => run_merge_transaction(v).await,
            Some("outputs") => run_merge_output(v).await,
            Some("provenTxs") => run_merge_proven_tx(v).await,
            other => panic!(
                "{}: unmapped mergeExisting entity {other:?} — extend the runner, don't skip",
                v.id
            ),
        }
    }
    assert_eq!(executed, 6, "all 6 mergeExisting vectors must execute");
}

/// Replay `existing` then `incoming` as two single-transaction chunks.
///
/// The rows reference producer-side provenTxId 1001; a real BRC-40 round
/// delivers provenTxs before transactions, so the proven-tx id-map entry is
/// seeded up front (both TS and Rust drop an UNMAPPED optional FK to null by
/// design — without the mapping the vectors' provenTxId expectations would be
/// unobservable through the consumer).
async fn run_merge_transaction(v: &Vector) {
    let storage = setup_storage().await;
    let user_id = storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user")
        .0
        .user_id;

    let existing = v.input.existing.as_ref().expect("existing");
    let incoming = v.input.incoming.as_ref().expect("incoming");

    let mut sync_map = SyncMap::new();
    let local_proven = seed_proven_tx_mapping(&storage, &mut sync_map, 1001).await;

    for row in [existing, incoming] {
        let mut chunk = empty_chunk();
        chunk.transactions = Some(vec![tx_row(row, user_id)]);
        process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
    }

    let reference = format!("vector-tx-{}", i(existing, "transactionId"));
    let got = find_tx_by_reference(&storage, user_id, &reference).await;

    let (want_row, side) = match v.expected.action.as_deref() {
        Some("update") => (incoming, "incoming"),
        Some("skip") => (existing, "existing"),
        other => panic!("{}: unmapped action {other:?}", v.id),
    };
    assert_eq!(
        got.status,
        status(want_row),
        "{}: {side} status must survive the merge",
        v.id
    );
    let want_proven = want_row["provenTxId"].as_i64().map(|_| local_proven);
    assert_eq!(
        got.proven_tx_id, want_proven,
        "{}: {side} provenTxId must survive the merge (stale chunk must not clear it)",
        v.id
    );
}

/// Insert a local proven-tx row and map the producer-side id onto it,
/// standing in for the earlier chunk of the round that carried it.
async fn seed_proven_tx_mapping(
    storage: &SqliteStorage,
    sync_map: &mut SyncMap,
    foreign_id: i64,
) -> i64 {
    let now = parse_iso("2026-04-23T11:00:00.000Z");
    let local = storage
        .insert_proven_tx(
            &ProvenTx {
                created_at: now,
                updated_at: now,
                proven_tx_id: 0,
                txid: format!("seeded-proven-{foreign_id}"),
                height: 1,
                index: 0,
                merkle_path: vec![1],
                raw_tx: vec![0u8; 4],
                block_hash: "seed".to_string(),
                merkle_root: "seed".to_string(),
            },
            None,
        )
        .await
        .expect("seed proven tx");
    sync_map
        .proven_tx
        .map_id(foreign_id, local)
        .expect("map seeded proven tx");
    local
}

/// Replay `existing` then `incoming` as two single-output chunks. The parent
/// transaction (producer id 42) and the spender (producer id 43, referenced
/// by spentBy) ride in the first chunk so both FKs are mappable.
async fn run_merge_output(v: &Vector) {
    let storage = setup_storage().await;
    let user_id = storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user")
        .0
        .user_id;

    let existing = v.input.existing.as_ref().expect("existing");
    let incoming = v.input.incoming.as_ref().expect("incoming");

    let mut sync_map = SyncMap::new();

    // Parent + spender transactions, completed, older than both output rows.
    let seed_time = "2026-04-23T11:00:00.000Z";
    let mk_seed_tx = |id: i64| Transaction {
        created_at: parse_iso(seed_time),
        updated_at: parse_iso(seed_time),
        transaction_id: id,
        user_id,
        proven_tx_id: None,
        status: TransactionStatus::Completed,
        reference: format!("vector-tx-{id}"),
        is_outgoing: false,
        satoshis: 1,
        description: "conformance".to_string(),
        version: Some(1),
        lock_time: Some(0),
        txid: None,
        input_beef: None,
        raw_tx: None,
    };
    let mut seed = empty_chunk();
    seed.transactions = Some(vec![
        mk_seed_tx(i(existing, "transactionId")),
        mk_seed_tx(i(existing, "spentBy")),
    ]);
    process_sync_chunk(
        &storage,
        AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
        seed,
        &mut sync_map,
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("{}: seed failed: {e}", v.id));

    for row in [existing, incoming] {
        let mut chunk = empty_chunk();
        chunk.outputs = Some(vec![output_row(row, user_id)]);
        process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
    }

    let local_tx = sync_map
        .transaction
        .get_local_id(i(existing, "transactionId"))
        .expect("parent tx mapped");
    let local_spender = sync_map
        .transaction
        .get_local_id(i(existing, "spentBy"))
        .expect("spender tx mapped");

    let outputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    transaction_id: Some(local_tx),
                    vout: Some(i(existing, "vout") as i32),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("find_outputs");
    assert_eq!(outputs.len(), 1, "{}: exactly one output row", v.id);
    let got = &outputs[0];

    let (want_row, side) = match v.expected.action.as_deref() {
        Some("update") => (incoming, "incoming"),
        Some("skip") => (existing, "existing"),
        other => panic!("{}: unmapped action {other:?}", v.id),
    };
    assert_eq!(
        got.spendable,
        want_row["spendable"].as_bool().unwrap(),
        "{}: {side} spendable must survive (stale chunk must not flip spendable)",
        v.id
    );
    let want_spent_by = want_row["spentBy"].as_i64().map(|_| local_spender);
    assert_eq!(
        got.spent_by, want_spent_by,
        "{}: {side} spentBy must survive (stale chunk must not clear attribution)",
        v.id
    );
}

/// Replay `existing` then `incoming` as two single-provenTx chunks. The
/// corpus rows are identical apart from updated_at, so the runner marks each
/// side with a distinct block_hash (the field the merge writes) to observe
/// which side survived — same technique as the Go runner's merkle-root
/// markers.
async fn run_merge_proven_tx(v: &Vector) {
    let storage = setup_storage().await;
    storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user");

    let existing = v.input.existing.as_ref().expect("existing");
    let incoming = v.input.incoming.as_ref().expect("incoming");

    let mut sync_map = SyncMap::new();
    for (row, marker) in [(existing, "hash-existing"), (incoming, "hash-incoming")] {
        let mut chunk = empty_chunk();
        chunk.proven_txs = Some(vec![proven_row(row, marker)]);
        process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
    }

    let rows = storage
        .find_proven_txs(
            &FindProvenTxsArgs {
                partial: ProvenTxPartial {
                    txid: Some(s(existing, "txid")),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("find_proven_txs");
    assert_eq!(rows.len(), 1, "{}: exactly one proven tx row", v.id);

    let want_marker = match v.expected.action.as_deref() {
        Some("update") => "hash-incoming",
        Some("skip") => "hash-existing",
        other => panic!("{}: unmapped action {other:?}", v.id),
    };
    assert_eq!(
        rows[0].block_hash, want_marker,
        "{}: stale chunk must not overwrite a proven tx",
        v.id
    );
}

// ---------------------------------------------------------------------------
// brc40/flow — multi-chunk replays
// ---------------------------------------------------------------------------

/// Pinned flow divergences:
///
/// - sync.brc40.flow.idmap.1 — convergence itself passes (one row, both
///   producer ids mapped to the same local id), but the corpus pins the
///   surviving row's updated_at at the newer chunk's 13:00:00Z. Both this
///   crate and the TS reference treat proven txs as immutable on merge
///   (TS EntityProvenTx.mergeExisting returns false unconditionally), so the
///   row keeps the FIRST chunk's timestamp. Observed: 12:34:56Z. The corpus
///   expectation exceeds both implementations; flag upstream.
///
/// - sync.brc40.flow.idmap.error.1 — the corpus demands
///   ERR_BRC40_ID_MAPPING_CONFLICT when two DIFFERENT producer-side primary
///   keys claim the same natural key. Neither the TS reference
///   (EntitySyncState.mergeIdMap only rejects remapping the SAME foreign id)
///   nor Rust (EntitySyncMap::map_id, same rule) implements that rejection;
///   both converge silently — which is indistinguishable from the convergence
///   flow.idmap.1 celebrates. Observed: replay succeeds, one basket row.
///   The vector's expectation exceeds both implementations; flag upstream.
const KNOWN_FLOW_DIVERGENCES: &[&str] =
    &["sync.brc40.flow.idmap.1", "sync.brc40.flow.idmap.error.1"];

#[tokio::test]
async fn flow_channel() {
    let vectors = vectors_for("brc40/flow");
    assert_eq!(vectors.len(), 4);
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &vectors {
        assert!(!v.skip, "{}: unexpected corpus skip", v.id);
        executed += 1;
        match v.id.as_str() {
            "sync.brc40.flow.idmap.1" => flow_idmap_convergence(v, &mut failures).await,
            "sync.brc40.flow.idmap.error.1" => flow_idmap_conflict(v, &mut failures).await,
            "sync.brc40.flow.regression.1" => flow_stale_regression(v, &mut failures).await,
            "sync.brc40.flow.since.inclusive.1" => flow_since_inclusive(v, &mut failures).await,
            other => panic!("unmapped flow vector {other} — extend the runner, don't skip"),
        }
    }

    assert_eq!(executed, 4, "all 4 flow vectors must execute");
    assert_known_divergences("flow", &failures, KNOWN_FLOW_DIVERGENCES);
}

/// Deserialize a replayed message's syncChunk. The flow rows are entity
/// fragments; build full rows in code (same as mergeExisting) keyed off the
/// fragment fields.
fn flow_chunks(v: &Vector) -> Vec<Value> {
    v.input
        .messages
        .as_ref()
        .expect("flow vector messages")
        .iter()
        .map(|m| m["syncChunk"].clone())
        .collect()
}

async fn flow_idmap_convergence(v: &Vector, failures: &mut Vec<String>) {
    let storage = setup_storage().await;
    storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user");
    let mut sync_map = SyncMap::new();

    let mut foreign_ids = Vec::new();
    let mut txid = String::new();
    for sc in flow_chunks(v) {
        let rows = sc["provenTxs"].as_array().expect("provenTxs");
        let mut chunk = empty_chunk();
        chunk.proven_txs = Some(
            rows.iter()
                .map(|r| {
                    foreign_ids.push(i(r, "provenTxId"));
                    txid = s(r, "txid");
                    proven_row(r, "hash")
                })
                .collect(),
        );
        process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
    }

    let rows = storage
        .find_proven_txs(
            &FindProvenTxsArgs {
                partial: ProvenTxPartial {
                    txid: Some(txid),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("find_proven_txs");
    if rows.len() != 1 {
        failures.push(format!(
            "sync.brc40.flow.idmap.1-convergence: expected 1 row, got {}",
            rows.len()
        ));
        return;
    }
    let locals: Vec<Option<i64>> = foreign_ids
        .iter()
        .map(|f| sync_map.proven_tx.get_local_id(*f))
        .collect();
    if !(locals.len() == 2 && locals[0].is_some() && locals[0] == locals[1]) {
        failures.push(format!(
            "sync.brc40.flow.idmap.1-convergence: producer ids {foreign_ids:?} map to {locals:?}, expected one shared local id"
        ));
        return;
    }

    // Corpus: surviving row's updated_at must equal the newer chunk's
    // 13:00:00Z. See KNOWN_FLOW_DIVERGENCES — proven txs are immutable on
    // merge in both this crate and the TS reference.
    let want = parse_iso("2026-04-23T13:00:00.000Z");
    if rows[0].updated_at != want {
        failures.push(format!(
            "sync.brc40.flow.idmap.1: merged row updated_at expected {want}, observed {} (proven txs are never updated on merge, matching TS EntityProvenTx.mergeExisting)",
            rows[0].updated_at
        ));
    }
}

async fn flow_idmap_conflict(v: &Vector, failures: &mut Vec<String>) {
    let storage = setup_storage().await;
    let user_id = storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user")
        .0
        .user_id;
    let mut sync_map = SyncMap::new();

    let mut errored = false;
    for sc in flow_chunks(v) {
        let rows = sc["outputBaskets"].as_array().expect("outputBaskets");
        let mut chunk = empty_chunk();
        chunk.output_baskets = Some(
            rows.iter()
                .map(|r| OutputBasket {
                    created_at: parse_iso(&s(r, "created_at")),
                    updated_at: parse_iso(&s(r, "updated_at")),
                    basket_id: i(r, "basketId"),
                    user_id,
                    name: s(r, "name"),
                    number_of_desired_utxos: 1,
                    minimum_desired_utxo_value: 1000,
                    is_deleted: false,
                })
                .collect(),
        );
        if process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .is_err()
        {
            errored = true;
            break;
        }
    }

    // Corpus expects ERR_BRC40_ID_MAPPING_CONFLICT. See KNOWN_FLOW_DIVERGENCES
    // — neither TS nor Rust rejects two different producer ids converging on
    // one natural key.
    if !errored {
        failures.push(
            "sync.brc40.flow.idmap.error.1: expected an id-mapping-conflict error, observed silent convergence (matches TS reference behavior; corpus expectation exceeds both impls)"
                .to_string(),
        );
    }
}

async fn flow_stale_regression(v: &Vector, failures: &mut Vec<String>) {
    let storage = setup_storage().await;
    let user_id = storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user")
        .0
        .user_id;
    let mut sync_map = SyncMap::new();

    // finalState pins provenTxId=1001 surviving; the mapping for producer id
    // 1001 is seeded exactly as in run_merge_transaction (see its doc).
    let local_proven = seed_proven_tx_mapping(&storage, &mut sync_map, 1001).await;

    for sc in flow_chunks(v) {
        let rows = sc["transactions"].as_array().expect("transactions");
        let mut chunk = empty_chunk();
        chunk.transactions = Some(rows.iter().map(|r| tx_row(r, user_id)).collect());
        process_sync_chunk(
            &storage,
            AuthenticatedIdentityKey::assert_authenticated(USER_KEY),
            chunk,
            &mut sync_map,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: process failed: {e}", v.id));
    }

    let final_state = v.expected.final_state.as_ref().expect("finalState");
    for want in final_state["transactions"]
        .as_array()
        .expect("transactions")
    {
        let reference = format!("vector-tx-{}", i(want, "transactionId"));
        let got = find_tx_by_reference(&storage, user_id, &reference).await;
        if got.status != status(want) {
            failures.push(format!(
                "{}: final status expected {:?}, observed {:?} — the stale chunk regressed state",
                v.id,
                status(want),
                got.status
            ));
        }
        let want_proven = want["provenTxId"].as_i64().map(|_| local_proven);
        if got.proven_tx_id != want_proven {
            failures.push(format!(
                "{}: final provenTxId expected {want_proven:?}, observed {:?} — the stale chunk cleared the proven-tx pointer",
                v.id, got.proven_tx_id
            ));
        }
        // Insert honors the chunk's updated_at (13:00Z); the stale replay must
        // not have touched the row at all.
        let want_updated = parse_iso(&s(want, "updated_at"));
        if got.updated_at != want_updated {
            failures.push(format!(
                "{}: final updated_at expected {want_updated}, observed {}",
                v.id, got.updated_at
            ));
        }
    }
}

/// Producer contract: `since` is an INCLUSIVE lower bound on updated_at, so a
/// row updated exactly at `since` must be returned. Seeds the row the
/// vector's response block describes and drives the real producer
/// (get_sync_chunk) with the vector's request.since.
async fn flow_since_inclusive(v: &Vector, failures: &mut Vec<String>) {
    let storage = setup_storage().await;
    let user_id = storage
        .find_or_insert_user(USER_KEY, None)
        .await
        .expect("user")
        .0
        .user_id;

    let request = v.input.request.as_ref().expect("request");
    let response = v.input.response.as_ref().expect("response");
    let since = s(request, "since");
    let want_rows = response["syncChunk"]["transactions"]
        .as_array()
        .expect("transactions");
    assert_eq!(want_rows.len(), 1, "{}: vector shape", v.id);
    let boundary = &want_rows[0];
    assert_eq!(
        s(boundary, "updated_at"),
        since,
        "{}: vector's boundary row must sit exactly at since",
        v.id
    );

    let mut tx = tx_row(boundary, user_id);
    tx.transaction_id = 0;
    storage
        .insert_transaction(&tx, None)
        .await
        .expect("seed tx");

    let mut args = request_args_for(USER_KEY);
    args.since = Some(parse_iso(&since));
    let chunk =
        bsv_wallet_toolbox::storage::traits::WalletStorageProvider::get_sync_chunk(&storage, &args)
            .await
            .unwrap_or_else(|e| panic!("{}: get_sync_chunk failed: {e}", v.id));

    let got = chunk.transactions.as_deref().unwrap_or_default();
    if !got.iter().any(|t| t.reference == tx.reference) {
        failures.push(format!(
            "{}: a row with updated_at == since was EXCLUDED — `since` must be an inclusive bound",
            v.id
        ));
    }
}
