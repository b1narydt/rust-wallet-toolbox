//! Durability characterisation and regression tests for the SQLite UTXO store.
//!
//! Motivated by the owner-reported symptom on the TS store: it "locks up"
//! under Runar/STAS3-scale atomic compositions (large BEEF, many inputs and
//! outputs). These tests drive the real storage createAction / processAction
//! paths against a file-backed database with that load shape.
//!
//! `bench_profile` is the measurement harness (run explicitly):
//!   cargo test --test store_durability bench_profile -- --ignored --nocapture

mod common;

#[cfg(feature = "sqlite")]
mod store_durability {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use chrono::Utc;

    use bsv::script::locking_script::LockingScript;
    use bsv::transaction::beef::{Beef, BEEF_V2};
    use bsv::transaction::transaction::Transaction as BsvTransaction;
    use bsv::transaction::transaction_output::TransactionOutput;

    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::action_types::{
        StorageCreateActionArgs, StorageCreateActionOptions, StorageCreateActionOutput,
        StorageProcessActionArgs,
    };
    use bsv_wallet_toolbox::storage::find_args::{
        FindOutputsArgs, FindProvenTxReqsArgs, OutputPartial, ProvenTxReqPartial,
    };
    use bsv_wallet_toolbox::storage::methods::create_action::storage_create_action;
    use bsv_wallet_toolbox::storage::methods::process_action::storage_process_action;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
    use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::tables::{Output, ProvenTxReq, Transaction};
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

    // -------------------------------------------------------------------
    // Fixtures
    // -------------------------------------------------------------------

    /// A file-backed store in a fresh temp dir, migrated and available.
    async fn file_storage(tag: &str) -> (SqliteStorage, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "store-durability-{tag}-{}",
            std::process::id() as u64 * 1_000_000 + rand::random::<u32>() as u64
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("wallet.db");
        // BENCH_SYNC=normal switches the writer to synchronous=NORMAL for
        // A/B measurement of the durability trade-off.
        let sqlite_synchronous = match std::env::var("BENCH_SYNC").as_deref() {
            Ok("normal") => bsv_wallet_toolbox::storage::SqliteSyncMode::Normal,
            _ => bsv_wallet_toolbox::storage::SqliteSyncMode::Full,
        };
        let config = StorageConfig {
            url: format!("sqlite://{}", db_path.display()),
            sqlite_synchronous,
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test)
            .await
            .expect("create storage");
        storage.migrate_database().await.expect("migrate");
        storage.make_available().await.expect("make available");
        (storage, dir)
    }

    /// Seed a user, default basket, and `n_change` spendable change UTXOs.
    async fn seed_user(storage: &SqliteStorage, n_change: usize, sats_each: i64) -> i64 {
        let (user, _) = storage
            .find_or_insert_user("durability_bench_user", None)
            .await
            .expect("create user");
        let user_id = user.user_id;
        let basket = storage
            .find_or_insert_output_basket(user_id, "default", None)
            .await
            .expect("create basket");

        let now = Utc::now().naive_utc();
        // 100 change outputs per funding transaction row.
        let per_tx = 100usize;
        let mut created = 0usize;
        let mut tx_n = 0u32;
        while created < n_change {
            let batch = per_tx.min(n_change - created);
            let txid = format!("{:064x}", rand::random::<u64>() ^ ((tx_n as u64) << 32));
            let tx_id = storage
                .insert_transaction(
                    &Transaction {
                        created_at: now,
                        updated_at: now,
                        transaction_id: 0,
                        user_id,
                        proven_tx_id: None,
                        status: TransactionStatus::Completed,
                        reference: format!("seed-{tx_n}-{}", rand::random::<u32>()),
                        is_outgoing: false,
                        satoshis: sats_each * batch as i64,
                        description: "seed funding".to_string(),
                        version: Some(1),
                        lock_time: Some(0),
                        txid: Some(txid.clone()),
                        input_beef: None,
                        raw_tx: None,
                    },
                    None,
                )
                .await
                .expect("insert seed tx");
            for i in 0..batch {
                storage
                    .insert_output(
                        &Output {
                            created_at: now,
                            updated_at: now,
                            output_id: 0,
                            user_id,
                            transaction_id: tx_id,
                            basket_id: Some(basket.basket_id),
                            spendable: true,
                            change: true,
                            output_description: Some("seed change".to_string()),
                            vout: i as i32,
                            satoshis: sats_each,
                            provided_by: StorageProvidedBy::Storage,
                            purpose: "change".to_string(),
                            output_type: "P2PKH".to_string(),
                            txid: Some(txid.clone()),
                            sender_identity_key: None,
                            derivation_prefix: Some("cHJlZml4".to_string()),
                            derivation_suffix: Some(format!("c3VmZml4{i}")),
                            custom_instructions: None,
                            spent_by: None,
                            sequence_number: None,
                            spending_description: None,
                            script_length: Some(25),
                            script_offset: None,
                            locking_script: Some(vec![0x76, 0xa9, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x88, 0xac]),
                        },
                        None,
                    )
                    .await
                    .expect("insert seed output");
            }
            created += batch;
            tx_n += 1;
        }
        user_id
    }

    /// Valid BEEF of roughly `target_bytes`, built from real transactions
    /// carrying STAS3-scale locking scripts (unproven ancestors: raw txs, no
    /// bumps — the shape createAction receives for unbroadcast composition
    /// chains).
    fn big_beef(target_bytes: usize) -> Vec<u8> {
        let script_bytes = 5 * 1024; // STAS3-scale token script
        let outputs_per_tx = 40; // ~200 KB per transaction
        let mut beef = Beef::new(BEEF_V2);
        let mut total = 0usize;
        let mut salt = 0u8;
        while total < target_bytes {
            let mut tx = BsvTransaction::new();
            salt = salt.wrapping_add(1);
            for i in 0..outputs_per_tx {
                let mut script = vec![0x51u8; script_bytes]; // OP_1 filler
                script[0] = salt; // make each tx unique
                script[1] = i as u8;
                tx.add_output(TransactionOutput {
                    satoshis: Some(1_000),
                    locking_script: LockingScript::from_binary(&script),
                    change: false,
                });
            }
            let mut raw = Vec::new();
            tx.to_binary(&mut raw).expect("serialize beef tx");
            total += raw.len();
            beef.merge_raw_tx(&raw, None).expect("merge raw tx");
        }
        let mut out = Vec::new();
        beef.to_binary(&mut out).expect("serialize beef");
        out
    }

    /// Runar-shaped createAction args: many token-sized outputs plus a large
    /// input BEEF.
    fn runar_args(n_outputs: usize, script_bytes: usize, beef: Option<Vec<u8>>) -> StorageCreateActionArgs {
        let script_hex = {
            let mut s = vec![0x51u8; script_bytes];
            s[0] = 0x76;
            s.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        StorageCreateActionArgs {
            description: "runar composition".to_string(),
            inputs: vec![],
            outputs: (0..n_outputs)
                .map(|i| StorageCreateActionOutput {
                    locking_script: script_hex.clone(),
                    satoshis: 1_000,
                    output_description: format!("token output {i}"),
                    basket: Some("runar-tokens".to_string()),
                    custom_instructions: None,
                    tags: vec!["stas3".to_string()],
                })
                .collect(),
            lock_time: 0,
            version: 1,
            labels: vec!["runar".to_string()],
            options: StorageCreateActionOptions::default(),
            input_beef: beef,
            is_new_tx: true,
            is_sign_action: false,
            is_no_send: false,
            is_delayed: true,
            is_send_with: false,
            is_remix_change: false,
            is_test_werr_review_actions: None,
            include_all_source_transactions: false,
            random_vals: None,
        }
    }

    /// One full spend pipeline at the storage level: createAction (allocate +
    /// record, in one db trx) then processAction (ProvenTxReq + status flips,
    /// in another db trx). Returns (create_ms, process_ms).
    async fn one_spend(
        storage: &SqliteStorage,
        user_id: i64,
        n_outputs: usize,
        script_bytes: usize,
        beef: Option<Vec<u8>>,
        raw_tx_bytes: usize,
    ) -> Result<(f64, f64), bsv_wallet_toolbox::error::WalletError> {
        let args = runar_args(n_outputs, script_bytes, beef);
        let t0 = Instant::now();
        let dcr = storage_create_action(storage, user_id, &args, None).await?;
        let create_ms = t0.elapsed().as_secs_f64() * 1e3;

        let txid = format!("{:064x}", rand::random::<u64>());
        let p_args = StorageProcessActionArgs {
            is_new_tx: true,
            is_send_with: false,
            is_no_send: false,
            is_delayed: true,
            reference: Some(dcr.reference.clone()),
            txid: Some(txid),
            raw_tx: Some(vec![0xabu8; raw_tx_bytes]),
            send_with: vec![],
        };
        let t1 = Instant::now();
        storage_process_action(storage, user_id, &p_args, None).await?;
        let process_ms = t1.elapsed().as_secs_f64() * 1e3;
        Ok((create_ms, process_ms))
    }

    fn pct(sorted_ms: &[f64], p: f64) -> f64 {
        if sorted_ms.is_empty() {
            return f64::NAN;
        }
        let idx = ((sorted_ms.len() - 1) as f64 * p).round() as usize;
        sorted_ms[idx]
    }

    fn summarize(name: &str, mut ms: Vec<f64>, errors: &[String]) {
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  {name}: n={} p50={:.1}ms p95={:.1}ms max={:.1}ms errors={}",
            ms.len(),
            pct(&ms, 0.50),
            pct(&ms, 0.95),
            pct(&ms, 1.0),
            errors.len()
        );
        for e in errors.iter().take(5) {
            println!("    error: {e}");
        }
    }

    // -------------------------------------------------------------------
    // Measurement harness
    // -------------------------------------------------------------------

    /// Characterisation run. Prints latency profiles; makes no assertions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "measurement harness; run explicitly with --ignored --nocapture"]
    async fn bench_profile() {
        // ---- Phase 1: single-caller latency vs payload size ----
        println!("Phase 1: single-caller spend latency vs Runar payload size");
        let (storage, dir) = file_storage("p1").await;
        let user_id = seed_user(&storage, 4_000, 50_000).await;

        for (label, n_out, script_b, beef_mb, raw_kb) in [
            ("small (2 out, 25B scripts, no BEEF, 1KB rawTx)", 2usize, 25usize, 0usize, 1usize),
            ("runar-2MB (30 out, 5KB scripts, 2MB BEEF, 300KB rawTx)", 30, 5 * 1024, 2, 300),
            ("runar-8MB (60 out, 5KB scripts, 8MB BEEF, 800KB rawTx)", 60, 5 * 1024, 8, 800),
        ] {
            let beef = if beef_mb == 0 { None } else { Some(big_beef(beef_mb << 20)) };
            let mut creates = vec![];
            let mut processes = vec![];
            for _ in 0..10 {
                let (c, p) = one_spend(&storage, user_id, n_out, script_b, beef.clone(), raw_kb * 1024)
                    .await
                    .expect("spend");
                creates.push(c);
                processes.push(p);
            }
            summarize(&format!("{label} create"), creates, &[]);
            summarize(&format!("{label} process"), processes, &[]);
        }
        let db_size = std::fs::metadata(dir.join("wallet.db")).map(|m| m.len()).unwrap_or(0);
        println!("  db file: {:.1} MB", db_size as f64 / 1e6);
        drop(storage);

        // ---- Phase 2: concurrent mixed load ----
        // Knobs: BENCH_SPENDERS (default 4), BENCH_BEEF_MB (default 2),
        // BENCH_SECS (default 15).
        let n_spenders: usize = std::env::var("BENCH_SPENDERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let beef_mb: usize = std::env::var("BENCH_BEEF_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let secs: u64 = std::env::var("BENCH_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);
        println!(
            "Phase 2: {secs}s mixed load — {n_spenders} spenders ({beef_mb}MB BEEF), 1 monitor writer, 2 readers"
        );
        let (storage, _dir2) = file_storage("p2").await;
        let user_id = seed_user(&storage, 6_000, 50_000).await;
        let storage = Arc::new(storage);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let deadline = Duration::from_secs(secs);

        let mut handles = Vec::new();
        // Spenders: full storage-level pipeline with Runar payloads.
        for s in 0..n_spenders {
            let st = storage.clone();
            let stop = stop.clone();
            handles.push(tokio::spawn(async move {
                let beef = big_beef(beef_mb << 20);
                let mut lat = vec![];
                let mut errs = vec![];
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let t = Instant::now();
                    match one_spend(&st, user_id, 30, 5 * 1024, Some(beef.clone()), 300 * 1024).await {
                        Ok(_) => lat.push(t.elapsed().as_secs_f64() * 1e3),
                        Err(e) => errs.push(format!("{e}")),
                    }
                }
                (format!("spender-{s}"), lat, errs)
            }));
        }
        // Monitor-style writer: status flips with trx None (as monitor tasks do).
        {
            let st = storage.clone();
            let stop = stop.clone();
            handles.push(tokio::spawn(async move {
                let mut lat = vec![];
                let mut errs = vec![];
                let now = Utc::now().naive_utc();
                let mut n = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let t = Instant::now();
                    let req = ProvenTxReq {
                        created_at: now,
                        updated_at: now,
                        proven_tx_req_id: 0,
                        proven_tx_id: None,
                        status: bsv_wallet_toolbox::status::ProvenTxReqStatus::Unsent,
                        attempts: 0,
                        notified: false,
                        txid: format!("{:032x}{:032x}", n, rand::random::<u64>()),
                        batch: None,
                        history: "{}".to_string(),
                        notify: "{}".to_string(),
                        raw_tx: vec![0u8; 1024],
                        input_beef: None,
                    };
                    n += 1;
                    match StorageReaderWriter::insert_proven_tx_req(st.as_ref(), &req, None).await {
                        Ok(_) => lat.push(t.elapsed().as_secs_f64() * 1e3),
                        Err(e) => errs.push(format!("{e}")),
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                ("monitor-writer".to_string(), lat, errs)
            }));
        }
        // Readers: UTXO listings while writes are in flight.
        for r in 0..2 {
            let st = storage.clone();
            let stop = stop.clone();
            handles.push(tokio::spawn(async move {
                let mut lat = vec![];
                let mut errs = vec![];
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let t = Instant::now();
                    let args = FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            spendable: Some(true),
                            ..Default::default()
                        },
                        paged: Some(bsv_wallet_toolbox::storage::find_args::Paged {
                            limit: 100,
                            offset: 0,
                        }),
                        ..Default::default()
                    };
                    match StorageReader::find_outputs(st.as_ref(), &args, None).await {
                        Ok(_) => lat.push(t.elapsed().as_secs_f64() * 1e3),
                        Err(e) => errs.push(format!("{e}")),
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                (format!("reader-{r}"), lat, errs)
            }));
        }

        tokio::time::sleep(deadline).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in handles {
            let (name, lat, errs) = h.await.expect("task join");
            summarize(&name, lat, &errs);
        }

        // ---- Phase 3: reader latency while a write transaction holds the writer ----
        println!("Phase 3: read latency while a large write trx is open (WAL check)");
        {
            let st = storage.clone();
            let trx = st.begin_transaction().await.expect("begin");
            // Park a large insert inside the open transaction, uncommitted.
            let now = Utc::now().naive_utc();
            let tx = Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Unsigned,
                reference: format!("wal-check-{}", rand::random::<u32>()),
                is_outgoing: true,
                satoshis: 0,
                description: "open trx".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: None,
                input_beef: Some(vec![0u8; 4 << 20]),
                raw_tx: None,
            };
            storage
                .insert_transaction(&tx, Some(&trx))
                .await
                .expect("insert inside trx");
            let mut lat = vec![];
            for _ in 0..50 {
                let t = Instant::now();
                let args = FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial::default(),
                    paged: Some(bsv_wallet_toolbox::storage::find_args::Paged {
                        limit: 50,
                        offset: 0,
                    }),
                    ..Default::default()
                };
                StorageReader::find_proven_tx_reqs(storage.as_ref(), &args, None)
                    .await
                    .expect("read during open write trx");
                lat.push(t.elapsed().as_secs_f64() * 1e3);
            }
            summarize("reads during open 4MB write trx", lat, &[]);
            storage.commit_transaction(trx).await.expect("commit");
        }
    }

    // -------------------------------------------------------------------
    // Regression tests
    // -------------------------------------------------------------------

    /// A write arriving while another write transaction is in flight must
    /// queue until the writer connection frees up — not fail. With the writer
    /// acquire timeout at the 5s connect timeout, any burst whose queue
    /// exceeded 5s turned into `pool timed out` errors (measured: ~50% of
    /// operations failing at 32 concurrent Runar-scale spends).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn write_queues_behind_long_transaction_instead_of_erroring() {
        let (storage, _dir) = file_storage("queue").await;
        let user_id = seed_user(&storage, 1, 1_000).await;
        let storage = Arc::new(storage);
        let now = Utc::now().naive_utc();

        // Occupy the single writer connection with an open transaction.
        let trx = storage.begin_transaction().await.expect("begin");
        let hold_tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Unsigned,
            reference: "holder-ref".to_string(),
            is_outgoing: true,
            satoshis: 0,
            description: "long-held transaction".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: None,
            input_beef: None,
            raw_tx: None,
        };
        storage
            .insert_transaction(&hold_tx, Some(&trx))
            .await
            .expect("insert inside held trx");

        // A concurrent plain write must wait for the commit, then succeed.
        let st = storage.clone();
        let queued = tokio::spawn(async move {
            let t = Instant::now();
            let tx = Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Unsigned,
                reference: "queued-ref".to_string(),
                is_outgoing: true,
                satoshis: 0,
                description: "queued write".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: None,
                input_beef: None,
                raw_tx: None,
            };
            let r = st.insert_transaction(&tx, None).await;
            (r, t.elapsed())
        });

        // Hold past the old 5s cliff before committing.
        tokio::time::sleep(Duration::from_millis(6_500)).await;
        storage.commit_transaction(trx).await.expect("commit");

        let (result, waited) = queued.await.expect("join");
        result.expect(
            "a write queued behind a long transaction must succeed once the writer frees up",
        );
        assert!(
            waited >= Duration::from_secs(6),
            "the queued write should have actually waited for the held transaction, waited {waited:?}"
        );
    }

    /// The configured `synchronous` level must hold on every writer
    /// connection the pool ever opens. Applying it as a post-connect query
    /// (`PRAGMA synchronous=NORMAL` against the pool) configured only the
    /// connection that happened to serve that query; any replacement
    /// connection silently reverted to the SQLite default.
    #[tokio::test]
    async fn synchronous_pragma_applies_to_every_writer_connection() {
        let (storage, _dir) = file_storage("sync").await;

        // FULL (=2) is the durability-first default.
        let expected: i64 = 2;

        let first: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&storage.write_pool)
            .await
            .expect("read synchronous on pooled connection");
        assert_eq!(
            first, expected,
            "writer connection must run at the configured synchronous level"
        );

        // Force the pool to open a replacement connection.
        let conn = storage
            .write_pool
            .acquire()
            .await
            .expect("acquire writer connection");
        drop(conn.detach());

        let fresh: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&storage.write_pool)
            .await
            .expect("read synchronous on replacement connection");
        assert_eq!(
            fresh, expected,
            "a replacement writer connection must keep the configured synchronous level"
        );
    }

    /// The chaintracks header store must run in WAL mode with a busy
    /// timeout — its pool holds multiple connections that can all write, so
    /// rollback-journal mode made concurrent header traffic fail with
    /// SQLITE_BUSY (the exact failure shape of the TS store).
    #[tokio::test]
    async fn chaintracks_store_runs_in_wal_mode() {
        let dir = std::env::temp_dir().join(format!(
            "chaintracks-wal-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let url = format!("sqlite://{}", dir.join("headers.db").display());
        let store = bsv_wallet_toolbox::chaintracks::SqliteStorage::new(&url, Chain::Test)
            .await
            .expect("create chaintracks storage");

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(store.pool())
            .await
            .expect("read journal_mode");
        assert_eq!(mode, "wal", "chaintracks header store must use WAL");
    }
}

/// The spend lock must not be held across network broadcast: a spend-path
/// caller must not stall behind another caller's in-flight broadcast.
#[cfg(feature = "sqlite")]
mod spend_lock_scope {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use chrono::Utc;

    use bsv::primitives::private_key::PrivateKey;
    use bsv::transaction::chain_tracker::ChainTracker;
    use bsv::transaction::transaction::Transaction as BsvTransaction;
    use bsv::transaction::transaction_output::TransactionOutput;
    use bsv::transaction::Beef;
    use bsv::wallet::interfaces::{
        CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface,
    };
    use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue};

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::services::traits::WalletServices;
    use bsv_wallet_toolbox::services::types;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::tables::{Output, OutputBasket, Transaction};
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
    use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
    use bsv_wallet_toolbox::wallet::setup::WalletBuilder;

    use super::common::MockWalletServices;

    /// Delegates everything to `MockWalletServices`, but `post_beef` takes
    /// `delay` — a stand-in for a slow ARC round-trip (the real client allows
    /// up to 30s).
    struct SlowBroadcastServices {
        inner: MockWalletServices,
        delay: Duration,
    }

    #[async_trait]
    impl WalletServices for SlowBroadcastServices {
        fn chain(&self) -> Chain {
            self.inner.chain()
        }
        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            self.inner.get_chain_tracker().await
        }
        async fn get_merkle_path(&self, txid: &str, use_next: bool) -> types::GetMerklePathResult {
            self.inner.get_merkle_path(txid, use_next).await
        }
        async fn get_raw_tx(&self, txid: &str, use_next: bool) -> types::GetRawTxResult {
            self.inner.get_raw_tx(txid, use_next).await
        }
        async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Vec<types::PostBeefResult> {
            tokio::time::sleep(self.delay).await;
            self.inner.post_beef(beef, txids).await
        }
        async fn get_utxo_status(
            &self,
            output: &str,
            output_format: Option<types::GetUtxoStatusOutputFormat>,
            outpoint: Option<&str>,
            use_next: bool,
        ) -> types::GetUtxoStatusResult {
            self.inner
                .get_utxo_status(output, output_format, outpoint, use_next)
                .await
        }
        async fn get_status_for_txids(
            &self,
            txids: &[String],
            use_next: bool,
        ) -> types::GetStatusForTxidsResult {
            self.inner.get_status_for_txids(txids, use_next).await
        }
        async fn get_script_hash_history(
            &self,
            hash: &str,
            use_next: bool,
        ) -> types::GetScriptHashHistoryResult {
            self.inner.get_script_hash_history(hash, use_next).await
        }
        async fn hash_to_header(&self, hash: &str) -> WalletResult<types::BlockHeader> {
            self.inner.hash_to_header(hash).await
        }
        async fn get_header_for_height(&self, height: u32) -> WalletResult<Vec<u8>> {
            self.inner.get_header_for_height(height).await
        }
        async fn get_height(&self) -> WalletResult<u32> {
            self.inner.get_height().await
        }
        async fn n_lock_time_is_final(&self, input: types::NLockTimeInput) -> WalletResult<bool> {
            self.inner.n_lock_time_is_final(input).await
        }
        async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
            self.inner.get_bsv_exchange_rate().await
        }
        async fn get_fiat_exchange_rate(
            &self,
            currency: &str,
            base: Option<&str>,
        ) -> WalletResult<f64> {
            self.inner.get_fiat_exchange_rate(currency, base).await
        }
        async fn get_fiat_exchange_rates(
            &self,
            target_currencies: &[String],
        ) -> WalletResult<types::FiatExchangeRates> {
            self.inner.get_fiat_exchange_rates(target_currencies).await
        }
        fn get_services_call_history(&self, reset: bool) -> types::ServicesCallHistory {
            self.inner.get_services_call_history(reset)
        }
        async fn get_beef_for_txid(&self, txid: &str) -> WalletResult<Beef> {
            self.inner.get_beef_for_txid(txid).await
        }
        fn hash_output_script(&self, script: &[u8]) -> String {
            self.inner.hash_output_script(script)
        }
        async fn is_utxo(&self, locking_script: &[u8], txid: &str, vout: u32) -> WalletResult<bool> {
            self.inner.is_utxo(locking_script, txid, vout).await
        }
    }

    const SEED_PREFIX: &str = "c2VlZHByZWZpeA==";
    const SEED_SUFFIX: &str = "c2VlZHN1ZmZpeA==";

    /// Seed spendable BRC-29 change locked to the wallet's own key, backed by
    /// a real funding transaction so BEEF assembly works offline.
    async fn seed_spendable_change(
        storage: &WalletStorageManager,
        identity_key: &str,
        root_key: &PrivateKey,
        count: usize,
        satoshis: i64,
    ) {
        use bsv::script::locking_script::LockingScript;

        let locking_script = ScriptTemplateBRC29::new(SEED_PREFIX.to_string(), SEED_SUFFIX.to_string())
            .lock(root_key, &root_key.to_public_key())
            .expect("BRC-29 lock");

        let now = Utc::now().naive_utc();
        let (user, _) = storage
            .find_or_insert_user(identity_key)
            .await
            .expect("find_or_insert_user");

        let basket_id = storage
            .insert_output_basket(&OutputBasket {
                created_at: now,
                updated_at: now,
                basket_id: 0,
                user_id: user.user_id,
                name: "default".to_string(),
                number_of_desired_utxos: 10,
                minimum_desired_utxo_value: 1000,
                is_deleted: false,
            })
            .await
            .expect("insert basket");

        let mut funding = BsvTransaction::new();
        for _ in 0..count {
            funding.add_output(TransactionOutput {
                satoshis: Some(satoshis as u64),
                locking_script: LockingScript::from_binary(&locking_script),
                change: false,
            });
        }
        let mut funding_raw = Vec::new();
        funding.to_binary(&mut funding_raw).expect("serialize funding");
        let funding_txid = funding.id().expect("funding txid");

        let tx_id = storage
            .insert_transaction(&Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: format!("seed-{}", rand::random::<u32>()),
                is_outgoing: false,
                satoshis: satoshis * count as i64,
                description: "seed funding".to_string(),
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
                    output_description: Some(format!("seed change {i}")),
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
                .expect("insert seed utxo");
        }
    }

    fn payment_args(inline_broadcast: bool) -> CreateActionArgs {
        CreateActionArgs {
            description: "spend lock scope test".to_string(),
            inputs: vec![],
            outputs: vec![CreateActionOutput {
                locking_script: Some(vec![
                    0x76, 0xa9, 0x14, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
                    0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0x88, 0xac,
                ]),
                satoshis: 5_000,
                output_description: "payment".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: None,
            version: None,
            labels: vec![],
            options: Some(CreateActionOptions {
                sign_and_process: BooleanDefaultTrue(Some(true)),
                // false → broadcast happens inline, inside createAction
                accept_delayed_broadcast: BooleanDefaultTrue(Some(!inline_broadcast)),
                no_send: BooleanDefaultFalse(Some(false)),
                ..Default::default()
            }),
            input_beef: None,
            reference: None,
        }
    }

    /// One caller's in-flight (slow) broadcast must not block another
    /// caller's spend. Held pipeline-wide, the spend lock serialized every
    /// spend-path operation behind up to 30s of ARC round-trip per caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_spend_does_not_wait_for_in_flight_broadcast() {
        let broadcast_delay = Duration::from_secs(3);

        let root_key = PrivateKey::from_random().expect("random key");
        let setup = WalletBuilder::new()
            .chain(Chain::Test)
            .root_key(root_key.clone())
            .with_sqlite_memory()
            .with_services(Arc::new(SlowBroadcastServices {
                inner: MockWalletServices,
                delay: broadcast_delay,
            }))
            .without_monitor()
            .build()
            .await
            .expect("build wallet");
        seed_spendable_change(&setup.storage, &setup.identity_key, &root_key, 4, 50_000).await;

        let wallet = Arc::new(setup.wallet);

        // Caller A: inline broadcast — sits in post_beef for `broadcast_delay`.
        let wa = wallet.clone();
        let a = tokio::spawn(async move { wa.create_action(payment_args(true), None).await });

        // Give A time to finish allocation and reach the broadcast.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Caller B: delayed broadcast — no network in its path.
        let t = Instant::now();
        let b = wallet.create_action(payment_args(false), None).await;
        let b_latency = t.elapsed();

        b.expect("caller B's spend should succeed");
        assert!(
            b_latency < Duration::from_secs(2),
            "a spend must not queue behind another caller's in-flight broadcast \
             (took {b_latency:?} with a {broadcast_delay:?} broadcast in flight)"
        );

        // A comes back too (its broadcast outcome is the mock's business).
        let _ = a.await.expect("join caller A");
    }
}
