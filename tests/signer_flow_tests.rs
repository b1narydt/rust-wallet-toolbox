//! Behavioral tests for the signer broadcast flow.
//!
//! Covers three areas called out in the PR review "Test coverage gaps":
//!
//! 1. `spend_lock` concurrency on `WalletStorageManager` (proves the lock actually
//!    serializes concurrent UTXO-mutating callers, and that two `Wallet`
//!    instances sharing an `Arc<WalletStorageManager>` share the lock).
//! 2. `handle_permanent_broadcast_failure` behavior for `InvalidTx` and
//!    `DoubleSpend` classified outcomes — restoring consumed inputs, marking
//!    tx outputs unspendable, transitioning statuses.
//! 3. `OrphanMempool` outcome keeping the ProvenTxReq at `Sending` so the
//!    monitor retries (via the extracted `apply_success_or_orphan_outcome`
//!    helper that create_action / sign_action share).

mod common;

#[cfg(feature = "sqlite")]
mod signer_flow_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;

    use bsv::transaction::chain_tracker::ChainTracker;
    use bsv::transaction::Beef;

    use bsv_wallet_toolbox::error::{WalletError, WalletResult};
    use bsv_wallet_toolbox::services::traits::WalletServices;
    use bsv_wallet_toolbox::services::types;
    use bsv_wallet_toolbox::signer::broadcast_outcome::{
        apply_service_error_outcome, apply_success_or_orphan_outcome,
        handle_permanent_broadcast_failure, BroadcastOutcome,
    };
    use bsv_wallet_toolbox::status::{ProvenTxReqStatus, TransactionStatus};
    use bsv_wallet_toolbox::storage::find_args::{
        FindOutputsArgs, FindProvenTxReqsArgs, FindTransactionsArgs, OutputPartial,
        ProvenTxReqPartial, TransactionPartial,
    };
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
    use bsv_wallet_toolbox::storage::{StorageConfig, StorageReaderWriter};
    use bsv_wallet_toolbox::tables::{Output, OutputBasket, ProvenTxReq, Transaction};
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

    use super::common::{self, MockWalletServices};

    // -----------------------------------------------------------------------
    // Shared helpers
    // -----------------------------------------------------------------------

    const IDENTITY_KEY: &str =
        "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

    /// Build a migrated in-memory SQLite provider, returning both the concrete
    /// Arc (for direct ReaderWriter access) and the dyn-provider Arc (for the
    /// manager). They point at the same underlying SQLite instance.
    async fn create_provider_pair() -> (Arc<SqliteStorage>, Arc<dyn WalletStorageProvider>) {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await.unwrap();
        StorageProvider::migrate_database(&storage).await.unwrap();
        let concrete = Arc::new(storage);
        let as_provider: Arc<dyn WalletStorageProvider> = concrete.clone();
        (concrete, as_provider)
    }

    /// Build an initialized `WalletStorageManager` with one active store, and
    /// return both the manager Arc and the concrete SqliteStorage Arc pointing
    /// at the same DB (so tests can call ReaderWriter methods directly).
    async fn build_manager_pair() -> (Arc<WalletStorageManager>, Arc<SqliteStorage>) {
        let (concrete, as_provider) = create_provider_pair().await;
        let manager =
            WalletStorageManager::new(IDENTITY_KEY.to_string(), Some(as_provider), vec![]);
        manager.make_available().await.unwrap();
        manager
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .expect("find_or_insert_user");
        (Arc::new(manager), concrete)
    }

    /// Convenience: manager-only build for tests that don't need direct RW access.
    async fn build_manager() -> Arc<WalletStorageManager> {
        build_manager_pair().await.0
    }

    // -----------------------------------------------------------------------
    // Suite 1: spend_lock concurrency
    // -----------------------------------------------------------------------

    /// Multiple concurrent acquirers of the same Arc<WalletStorageManager>'s
    /// spend_lock must be serialized: at most one task may hold the lock at a
    /// time. If the lock were dropped (e.g., reverted to per-Wallet), the
    /// max-concurrent observed count would exceed 1.
    #[tokio::test]
    async fn spend_lock_serializes_concurrent_acquirers() {
        let manager = build_manager().await;

        let counter = Arc::new(tokio::sync::Mutex::new(0u32));
        let max_concurrent = Arc::new(tokio::sync::Mutex::new(0u32));

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let m = manager.clone();
            let c = counter.clone();
            let mx = max_concurrent.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = m.acquire_spend_lock().await.expect("acquire_spend_lock");
                {
                    let mut cur = c.lock().await;
                    *cur += 1;
                    let mut mxv = mx.lock().await;
                    if *cur > *mxv {
                        *mxv = *cur;
                    }
                }
                // Hold the lock long enough that other tasks would race if
                // serialization were broken.
                tokio::time::sleep(Duration::from_millis(20)).await;
                {
                    let mut cur = c.lock().await;
                    *cur -= 1;
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        let max = *max_concurrent.lock().await;
        assert_eq!(
            max, 1,
            "spend_lock failed to serialize: observed {} concurrent holders",
            max
        );
    }

    /// Two `Wallet` instances built from the SAME `Arc<WalletStorageManager>`
    /// must share the spend_lock, so operations across both wallets serialize.
    /// This is the architectural fix from PR #17 commit 4c0ff46: moving the
    /// lock from per-Wallet to manager-level.
    #[tokio::test]
    async fn spend_lock_shared_across_wallets_on_same_manager() {
        // Build a wallet, then build a second wallet that shares the same
        // underlying storage manager. We bypass the builder for the second
        // wallet and drive the lock directly through the shared Arc -- the
        // behavior we want to verify is solely that the Arc<Manager>-level
        // mutex serializes all callers regardless of how many Wallet wrappers
        // point at it.
        let setup = common::create_test_wallet().await;
        let manager_a: Arc<WalletStorageManager> = setup.storage.clone();
        let manager_b: Arc<WalletStorageManager> = setup.storage.clone();

        let counter = Arc::new(tokio::sync::Mutex::new(0u32));
        let max_concurrent = Arc::new(tokio::sync::Mutex::new(0u32));

        let mut tasks = Vec::new();
        // 4 tasks through manager_a, 4 through manager_b -- same underlying Arc.
        for i in 0..8 {
            let m = if i % 2 == 0 {
                manager_a.clone()
            } else {
                manager_b.clone()
            };
            let c = counter.clone();
            let mx = max_concurrent.clone();
            tasks.push(tokio::spawn(async move {
                let _guard = m.acquire_spend_lock().await.expect("acquire_spend_lock");
                {
                    let mut cur = c.lock().await;
                    *cur += 1;
                    let mut mxv = mx.lock().await;
                    if *cur > *mxv {
                        *mxv = *cur;
                    }
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
                {
                    let mut cur = c.lock().await;
                    *cur -= 1;
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }

        let max = *max_concurrent.lock().await;
        assert_eq!(
            max, 1,
            "spend_lock is not shared across Arc<Manager> clones: observed {} concurrent holders",
            max
        );
    }

    // -----------------------------------------------------------------------
    // Suite 2: handle_permanent_broadcast_failure behavior
    // -----------------------------------------------------------------------

    /// Seed a failing transaction with `n_inputs` consumed parent outputs.
    ///
    /// Returns (failed_tx_txid, failed_tx_id, parent_output_ids, failed_tx_output_ids).
    async fn seed_failing_tx_with_consumed_inputs(
        manager: &WalletStorageManager,
        concrete: &SqliteStorage,
        n_inputs: usize,
        provide_locking_scripts: bool,
    ) -> (String, i64, Vec<i64>, Vec<i64>) {
        let now = Utc::now().naive_utc();

        let (user, _) = manager
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .expect("find_or_insert_user");
        let user_id = user.user_id;

        // Basket
        let basket_id =
            StorageReaderWriter::insert_output_basket(
                concrete,
                &OutputBasket {
                    created_at: now,
                    updated_at: now,
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
            .expect("insert basket");

        // Parent transaction (source of inputs)
        let parent_txid_hex = format!("{:064x}", rand::random::<u64>());
        let parent_tx_id = StorageReaderWriter::insert_transaction(
            concrete,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: format!("parent-{}", rand::random::<u32>()),
                is_outgoing: false,
                satoshis: (1000 * n_inputs) as i64,
                description: "parent tx".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(parent_txid_hex.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert parent tx");

        // The failing transaction itself
        let failed_txid_hex = format!("{:064x}", rand::random::<u64>());
        let failed_tx_id = StorageReaderWriter::insert_transaction(
            concrete,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Sending,
                reference: format!("failed-{}", rand::random::<u32>()),
                is_outgoing: true,
                satoshis: 900,
                description: "failing tx".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(failed_txid_hex.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert failed tx");

        // Parent outputs -- consumed by the failing tx (spent_by, spendable=false)
        let mut parent_output_ids = Vec::with_capacity(n_inputs);
        for i in 0..n_inputs {
            // P2PKH-ish locking script bytes: OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG
            let mut hash160 = [0u8; 20];
            hash160[0] = i as u8;
            hash160[1] = 0xAB;
            let locking_script: Option<Vec<u8>> = if provide_locking_scripts {
                let mut s = Vec::with_capacity(25);
                s.push(0x76); // OP_DUP
                s.push(0xa9); // OP_HASH160
                s.push(0x14); // push 20
                s.extend_from_slice(&hash160);
                s.push(0x88); // OP_EQUALVERIFY
                s.push(0xac); // OP_CHECKSIG
                Some(s)
            } else {
                None
            };
            let script_len: Option<i64> =
                locking_script.as_ref().map(|s| s.len() as i64);
            let oid = StorageReaderWriter::insert_output(
                concrete,
                &Output {
                    created_at: now,
                    updated_at: now,
                    output_id: 0,
                    user_id,
                    transaction_id: parent_tx_id,
                    basket_id: Some(basket_id),
                    spendable: false, // consumed
                    change: false,
                    output_description: Some(format!("parent output {}", i)),
                    vout: i as i32,
                    satoshis: 1000,
                    provided_by: StorageProvidedBy::You,
                    purpose: "change".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(parent_txid_hex.clone()),
                    sender_identity_key: None,
                    derivation_prefix: None,
                    derivation_suffix: None,
                    custom_instructions: None,
                    spent_by: Some(failed_tx_id),
                    sequence_number: None,
                    spending_description: None,
                    script_length: script_len,
                    script_offset: None,
                    locking_script,
                },
                None,
            )
            .await
            .expect("insert parent output");
            parent_output_ids.push(oid);
        }

        // Failed tx's own outputs -- initially spendable (will be marked unspendable)
        let mut failed_tx_output_ids = Vec::with_capacity(2);
        for i in 0i32..2 {
            let oid = StorageReaderWriter::insert_output(
                concrete,
                &Output {
                    created_at: now,
                    updated_at: now,
                    output_id: 0,
                    user_id,
                    transaction_id: failed_tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: i == 1,
                    output_description: Some(format!("failed tx output {}", i)),
                    vout: i,
                    satoshis: 400,
                    provided_by: StorageProvidedBy::You,
                    purpose: if i == 1 {
                        "change".to_string()
                    } else {
                        "payment".to_string()
                    },
                    output_type: "P2PKH".to_string(),
                    txid: Some(failed_txid_hex.clone()),
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
            .expect("insert failed tx output");
            failed_tx_output_ids.push(oid);
        }

        // A ProvenTxReq referencing the failed tx (so status transitions are observable).
        StorageReaderWriter::insert_proven_tx_req(
            concrete,
            &ProvenTxReq {
                created_at: now,
                updated_at: now,
                proven_tx_req_id: 0,
                proven_tx_id: None,
                status: ProvenTxReqStatus::Unprocessed,
                attempts: 0,
                notified: false,
                txid: failed_txid_hex.clone(),
                batch: None,
                history: "{}".to_string(),
                notify: "{}".to_string(),
                raw_tx: vec![0u8; 4],
                input_beef: None,
            },
            None,
        )
        .await
        .expect("insert proven_tx_req");

        (
            failed_txid_hex,
            failed_tx_id,
            parent_output_ids,
            failed_tx_output_ids,
        )
    }

    /// Convenience: read an output row by ID.
    async fn get_output(manager: &WalletStorageManager, id: i64) -> Output {
        let outputs = manager
            .find_outputs(&FindOutputsArgs {
                partial: OutputPartial {
                    output_id: Some(id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .expect("find_outputs");
        outputs.into_iter().next().expect("output exists")
    }

    /// Convenience: read tx by txid.
    async fn get_tx(manager: &WalletStorageManager, txid: &str) -> Transaction {
        let txs = manager
            .find_transactions(&FindTransactionsArgs {
                partial: TransactionPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .expect("find_transactions");
        txs.into_iter().next().expect("tx exists")
    }

    /// Convenience: read ProvenTxReq by txid.
    async fn get_req(manager: &WalletStorageManager, txid: &str) -> ProvenTxReq {
        let reqs = manager
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .expect("find_proven_tx_reqs");
        reqs.into_iter().next().expect("req exists")
    }

    /// InvalidTx path: tx was malformed — all consumed inputs must be restored
    /// (safe since the tx never touched the chain), the tx's own outputs must
    /// be marked unspendable, and the tx status becomes Failed.
    #[tokio::test]
    async fn invalid_tx_restores_all_inputs_and_marks_outputs_unspendable() {
        let (manager, concrete) = build_manager_pair().await;
        let services = Arc::new(MockWalletServices);

        let (txid, _tx_id, parent_ids, failed_output_ids) =
            seed_failing_tx_with_consumed_inputs(&manager, &concrete, 2, true).await;

        let result = handle_permanent_broadcast_failure(
            &manager,
            &*services,
            &txid,
            &BroadcastOutcome::InvalidTx {
                details: vec!["mock: bad-txns".to_string()],
            },
        )
        .await
        .expect("handler should succeed");

        assert!(
            matches!(result, BroadcastOutcome::InvalidTx { .. }),
            "reconciliation should not override InvalidTx when chain is silent"
        );

        // Tx -> Failed
        let tx = get_tx(&manager, &txid).await;
        assert_eq!(tx.status, TransactionStatus::Failed);

        // ProvenTxReq -> Invalid
        let req = get_req(&manager, &txid).await;
        assert_eq!(req.status, ProvenTxReqStatus::Invalid);

        // All consumed inputs restored to spendable=true
        for parent_id in &parent_ids {
            let o = get_output(&manager, *parent_id).await;
            assert!(
                o.spendable,
                "InvalidTx must restore parent input {} to spendable",
                parent_id
            );
            // spent_by is left as-is per the handler's current contract (no
            // explicit clear); we assert only the spendable flag.
        }

        // Failed tx's own outputs marked unspendable
        for out_id in &failed_output_ids {
            let o = get_output(&manager, *out_id).await;
            assert!(
                !o.spendable,
                "InvalidTx must mark failed tx output {} unspendable",
                out_id
            );
        }
    }

    // ---- DoubleSpend mock: controlled is_utxo responses --------------------

    /// Mock that returns per-(txid,vout) configured is_utxo results. Used to
    /// test DoubleSpend recovery: input #1 is confirmed still-unspent (restore),
    /// input #2 is confirmed consumed on-chain (don't restore).
    struct DoubleSpendUtxoMock {
        /// (parent_txid, vout) -> is_utxo result
        map: std::collections::HashMap<(String, u32), bool>,
    }

    #[async_trait]
    impl WalletServices for DoubleSpendUtxoMock {
        fn chain(&self) -> Chain {
            Chain::Test
        }
        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_merkle_path(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> types::GetMerklePathResult {
            types::GetMerklePathResult {
                name: Some("mock".to_string()),
                merkle_path: None,
                header: None,
                error: None,
            }
        }
        async fn get_raw_tx(&self, txid: &str, _use_next: bool) -> types::GetRawTxResult {
            types::GetRawTxResult {
                txid: txid.to_string(),
                name: Some("mock".to_string()),
                raw_tx: None,
                error: None,
            }
        }
        async fn post_beef(
            &self,
            _beef: &[u8],
            _txids: &[String],
        ) -> Vec<types::PostBeefResult> {
            vec![]
        }
        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<types::GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> types::GetUtxoStatusResult {
            types::GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                is_utxo: Some(false),
                details: vec![],
            }
        }
        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> types::GetStatusForTxidsResult {
            // Chain says "unknown" for all -- no reconciliation override.
            types::GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                results: vec![],
            }
        }
        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> types::GetScriptHashHistoryResult {
            types::GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                history: vec![],
            }
        }
        async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Ok(vec![0u8; 80])
        }
        async fn get_height(&self) -> WalletResult<u32> {
            Ok(100_000)
        }
        async fn n_lock_time_is_final(
            &self,
            _input: types::NLockTimeInput,
        ) -> WalletResult<bool> {
            Ok(true)
        }
        async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
            Ok(types::BsvExchangeRate::default())
        }
        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }
        async fn get_fiat_exchange_rates(
            &self,
            _target_currencies: &[String],
        ) -> WalletResult<types::FiatExchangeRates> {
            Ok(types::FiatExchangeRates::default())
        }
        fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
            types::ServicesCallHistory { services: vec![] }
        }
        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }
        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            txid: &str,
            vout: u32,
        ) -> WalletResult<bool> {
            Ok(self
                .map
                .get(&(txid.to_string(), vout))
                .copied()
                .unwrap_or(false))
        }
    }

    /// DoubleSpend path: only inputs chain-verified as STILL unspent may be
    /// restored. Inputs that `is_utxo` returns false for (i.e. consumed by the
    /// competing tx) must remain `spendable=false`.
    ///
    /// BUG SURFACED: this test currently FAILS because
    /// `handle_permanent_broadcast_failure` calls
    /// `update_transaction_status_trx(txid, Failed)` in step 1, which at the
    /// SQLite layer (see `src/storage/sqlx_impl/trait_impls.rs:387-414`) has a
    /// side-effect of eagerly restoring ALL `spent_by=tx_id` outputs to
    /// `spendable=true, spent_by=NULL` — BEFORE the per-outpoint `is_utxo`
    /// filtering in step 4 runs. Net effect: every consumed input is
    /// restored regardless of whether the competing tx took it on chain, which
    /// enables a subsequent double-spend by the wallet. The fix (future wave)
    /// is either to (a) not auto-release in `update_transaction_status` when
    /// the handler supplies its own input-restoration plan, or (b) have the
    /// handler mark inputs unspendable AGAIN after step 1 for any outpoint
    /// not in `safe_ids`.
    #[tokio::test]
    async fn double_spend_restores_only_chain_verified_unspent_inputs() {
        let (manager, concrete) = build_manager_pair().await;

        let (txid, _tx_id, parent_ids, failed_output_ids) =
            seed_failing_tx_with_consumed_inputs(&manager, &concrete, 2, true).await;

        // Look up the parent txid hex via the parent outputs (they share parent txid).
        let parent_output_0 = get_output(&manager, parent_ids[0]).await;
        let parent_output_1 = get_output(&manager, parent_ids[1]).await;
        let parent_txid_hex = parent_output_0.txid.clone().expect("parent txid set");
        assert_eq!(parent_txid_hex, parent_output_1.txid.as_deref().unwrap());

        let mut utxo_map = std::collections::HashMap::new();
        // Input #0 (vout=0): still unspent on chain -> restore
        utxo_map.insert((parent_txid_hex.clone(), 0u32), true);
        // Input #1 (vout=1): consumed on chain by competing tx -> do NOT restore
        utxo_map.insert((parent_txid_hex.clone(), 1u32), false);

        let services = Arc::new(DoubleSpendUtxoMock { map: utxo_map });

        let result = handle_permanent_broadcast_failure(
            &manager,
            &*services,
            &txid,
            &BroadcastOutcome::DoubleSpend {
                competing_txs: vec!["competitor_txid".to_string()],
                details: vec!["mock: DOUBLE_SPEND_ATTEMPTED".to_string()],
            },
        )
        .await
        .expect("handler should succeed");

        assert!(matches!(result, BroadcastOutcome::DoubleSpend { .. }));

        // Tx -> Failed
        let tx = get_tx(&manager, &txid).await;
        assert_eq!(tx.status, TransactionStatus::Failed);

        // ProvenTxReq -> DoubleSpend
        let req = get_req(&manager, &txid).await;
        assert_eq!(req.status, ProvenTxReqStatus::DoubleSpend);

        // Input #0: restored
        let restored = get_output(&manager, parent_ids[0]).await;
        assert!(
            restored.spendable,
            "chain-verified-unspent input must be restored"
        );

        // Input #1: NOT restored (competing tx consumed it on chain)
        let still_consumed = get_output(&manager, parent_ids[1]).await;
        assert!(
            !still_consumed.spendable,
            "input consumed by competing tx must stay spendable=false"
        );

        // Failed tx's outputs all unspendable
        for out_id in &failed_output_ids {
            let o = get_output(&manager, *out_id).await;
            assert!(!o.spendable);
        }
    }

    /// Chain-reconciliation override: if `get_status_for_txids` reports the
    /// failed txid is "mined" or "known", the handler returns `Success` and
    /// performs no DB writes. Guards against false positives from single-provider
    /// flakes.
    struct ReconcileToSuccessMock;
    #[async_trait]
    impl WalletServices for ReconcileToSuccessMock {
        fn chain(&self) -> Chain {
            Chain::Test
        }
        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_merkle_path(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> types::GetMerklePathResult {
            types::GetMerklePathResult {
                name: Some("mock".to_string()),
                merkle_path: None,
                header: None,
                error: None,
            }
        }
        async fn get_raw_tx(&self, txid: &str, _use_next: bool) -> types::GetRawTxResult {
            types::GetRawTxResult {
                txid: txid.to_string(),
                name: Some("mock".to_string()),
                raw_tx: None,
                error: None,
            }
        }
        async fn post_beef(
            &self,
            _beef: &[u8],
            _txids: &[String],
        ) -> Vec<types::PostBeefResult> {
            vec![]
        }
        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<types::GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> types::GetUtxoStatusResult {
            types::GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                is_utxo: Some(false),
                details: vec![],
            }
        }
        async fn get_status_for_txids(
            &self,
            txids: &[String],
            _use_next: bool,
        ) -> types::GetStatusForTxidsResult {
            // Report each txid as "mined" -> reconciliation must override to Success.
            let results = txids
                .iter()
                .map(|t| types::StatusForTxidResult {
                    txid: t.clone(),
                    depth: Some(1),
                    status: "mined".to_string(),
                })
                .collect();
            types::GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                results,
            }
        }
        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> types::GetScriptHashHistoryResult {
            types::GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                history: vec![],
            }
        }
        async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Ok(vec![0u8; 80])
        }
        async fn get_height(&self) -> WalletResult<u32> {
            Ok(100_000)
        }
        async fn n_lock_time_is_final(
            &self,
            _input: types::NLockTimeInput,
        ) -> WalletResult<bool> {
            Ok(true)
        }
        async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
            Ok(types::BsvExchangeRate::default())
        }
        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }
        async fn get_fiat_exchange_rates(
            &self,
            _target_currencies: &[String],
        ) -> WalletResult<types::FiatExchangeRates> {
            Ok(types::FiatExchangeRates::default())
        }
        fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
            types::ServicesCallHistory { services: vec![] }
        }
        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }
        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            _txid: &str,
            _vout: u32,
        ) -> WalletResult<bool> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn chain_reconciliation_overrides_failure_to_success() {
        let (manager, concrete) = build_manager_pair().await;
        let services = Arc::new(ReconcileToSuccessMock);

        let (txid, _tx_id, parent_ids, failed_output_ids) =
            seed_failing_tx_with_consumed_inputs(&manager, &concrete, 2, true).await;

        let tx_before = get_tx(&manager, &txid).await;
        let result = handle_permanent_broadcast_failure(
            &manager,
            &*services,
            &txid,
            &BroadcastOutcome::InvalidTx {
                details: vec!["stale miner rejection".to_string()],
            },
        )
        .await
        .expect("handler should succeed");

        assert!(
            matches!(result, BroadcastOutcome::Success),
            "chain reconciliation must override to Success"
        );

        // No state changes should have been written.
        let tx_after = get_tx(&manager, &txid).await;
        assert_eq!(
            tx_after.status, tx_before.status,
            "tx status must be unchanged after reconcile-to-Success"
        );

        // Parent inputs stay consumed; failed-tx outputs stay as they were.
        for parent_id in &parent_ids {
            let o = get_output(&manager, *parent_id).await;
            assert!(!o.spendable);
        }
        for out_id in &failed_output_ids {
            let o = get_output(&manager, *out_id).await;
            assert!(o.spendable, "failed tx output untouched on reconcile-success");
        }
    }

    // -----------------------------------------------------------------------
    // Suite 3: OrphanMempool keeps req at Sending
    // -----------------------------------------------------------------------
    //
    // Tests the helper extracted from create_action / sign_action:
    //   `apply_success_or_orphan_outcome(storage, txid, outcome)`
    //
    // Desired behavior per PR review §"IMPORTANT #5":
    //   - Transaction status: Sending (stay — parent not yet propagated; monitor retries)
    //   - ProvenTxReq status: Sending (retry)
    //
    // Current (buggy) behavior: Transaction -> Unproven, ProvenTxReq -> Sending.
    // The first test asserts the CORRECT desired behavior and will FAIL against
    // the current implementation — that failure is the bug documented in the
    // PR review and will be fixed in a later wave. The second test asserts the
    // Success-path expectation that still holds.

    /// Seed a transaction at status `Sending` plus a matching `ProvenTxReq` at
    /// status `Unprocessed` (the state after processAction). Returns the txid.
    async fn seed_sending_tx_and_req(
        manager: &WalletStorageManager,
        concrete: &SqliteStorage,
    ) -> String {
        let now = Utc::now().naive_utc();
        let (user, _) = manager
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .expect("find_or_insert_user");

        let txid_hex = format!("{:064x}", rand::random::<u64>());
        StorageReaderWriter::insert_transaction(
            concrete,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                status: TransactionStatus::Sending,
                reference: format!("send-{}", rand::random::<u32>()),
                is_outgoing: true,
                satoshis: 100,
                description: "sending".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(txid_hex.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert tx");

        StorageReaderWriter::insert_proven_tx_req(
            concrete,
            &ProvenTxReq {
                created_at: now,
                updated_at: now,
                proven_tx_req_id: 0,
                proven_tx_id: None,
                status: ProvenTxReqStatus::Unprocessed,
                attempts: 0,
                notified: false,
                txid: txid_hex.clone(),
                batch: None,
                history: "{}".to_string(),
                notify: "{}".to_string(),
                raw_tx: vec![0u8; 4],
                input_beef: None,
            },
            None,
        )
        .await
        .expect("insert req");

        txid_hex
    }

    /// OrphanMempool outcome: parent tx not yet propagated — the signer must
    /// leave Transaction at `Sending` so the monitor can re-broadcast once
    /// the parent arrives, and flip ProvenTxReq to `Sending` for retry.
    ///
    /// This asserts the DESIRED behavior. Current code sets Transaction to
    /// `Unproven`, which is the bug from PR review §"IMPORTANT #5". Until the
    /// fix lands, this test SHOULD FAIL on the Transaction.status assertion.
    #[tokio::test]
    async fn orphan_mempool_outcome_keeps_req_at_sending() {
        let (manager, concrete) = build_manager_pair().await;
        let txid = seed_sending_tx_and_req(&manager, &concrete).await;

        apply_success_or_orphan_outcome(
            &manager,
            &txid,
            &BroadcastOutcome::OrphanMempool {
                details: vec!["mock: SEEN_IN_ORPHAN_MEMPOOL".to_string()],
            },
        )
        .await
        .expect("helper should succeed");

        let req = get_req(&manager, &txid).await;
        assert_eq!(
            req.status,
            ProvenTxReqStatus::Sending,
            "OrphanMempool must leave req at Sending for monitor retry"
        );

        let tx = get_tx(&manager, &txid).await;
        assert_eq!(
            tx.status,
            TransactionStatus::Sending,
            "OrphanMempool must keep tx at Sending (parent not propagated yet) — \
             current code sets Unproven which is the bug from PR review IMPORTANT #5"
        );
    }

    /// Success outcome: Transaction -> Unproven, ProvenTxReq -> Unmined.
    /// Verifies the helper also handles the non-orphan arm correctly.
    #[tokio::test]
    async fn success_outcome_transitions_to_unproven_and_unmined() {
        let (manager, concrete) = build_manager_pair().await;
        let txid = seed_sending_tx_and_req(&manager, &concrete).await;

        apply_success_or_orphan_outcome(&manager, &txid, &BroadcastOutcome::Success)
            .await
            .expect("helper should succeed");

        let req = get_req(&manager, &txid).await;
        assert_eq!(
            req.status,
            ProvenTxReqStatus::Unmined,
            "Success must flip req to Unmined"
        );

        let tx = get_tx(&manager, &txid).await;
        assert_eq!(
            tx.status,
            TransactionStatus::Unproven,
            "Success must flip tx to Unproven"
        );
    }

    // -----------------------------------------------------------------------
    // Suite 4: ServiceError retry transitions (Bug #4)
    //
    // Covers apply_service_error_outcome: a transient service failure must
    // leave req+tx in the Sending state with req.attempts bumped so that
    // TaskSendWaiting (which only scans [Unsent, Sending]) picks the tx up.
    // Previously the signer emitted a "will retry" log and did nothing,
    // stranding the tx at Unprocessed.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn service_error_outcome_transitions_req_and_tx_to_sending() {
        let (manager, concrete) = build_manager_pair().await;

        // Seed a tx + matching req, both at pre-broadcast state: tx=Sending,
        // req=Unprocessed (the state process_action hands off). We then apply
        // the service-error transition and assert the TS attemptToPostReqs
        // semantics: both -> Sending, attempts incremented by 1.
        let now = Utc::now().naive_utc();
        let (user, _) = manager
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .expect("find_or_insert_user");

        let txid_hex = format!("{:064x}", rand::random::<u64>());
        StorageReaderWriter::insert_transaction(
            &*concrete,
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                // Note: real process_action leaves tx at Unprocessed before
                // broadcast. We seed Sending here because that's the status
                // the service-error transition should *preserve/restore* to;
                // the invariant under test is "after ServiceError, tx=Sending",
                // independent of its prior value.
                status: TransactionStatus::Sending,
                reference: format!("svc-err-{}", rand::random::<u32>()),
                is_outgoing: true,
                satoshis: 100,
                description: "service-error retry seed".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(txid_hex.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert tx");

        StorageReaderWriter::insert_proven_tx_req(
            &*concrete,
            &ProvenTxReq {
                created_at: now,
                updated_at: now,
                proven_tx_req_id: 0,
                proven_tx_id: None,
                status: ProvenTxReqStatus::Unprocessed,
                attempts: 0,
                notified: false,
                txid: txid_hex.clone(),
                batch: None,
                history: "{}".to_string(),
                notify: "{}".to_string(),
                raw_tx: vec![0u8; 4],
                input_beef: None,
            },
            None,
        )
        .await
        .expect("insert req");

        apply_service_error_outcome(
            &manager,
            &txid_hex,
            vec!["timeout".to_string()],
        )
        .await
        .expect("service error transition should succeed");

        // req: Unprocessed -> Sending, attempts 0 -> 1
        let req = get_req(&manager, &txid_hex).await;
        assert_eq!(
            req.status,
            ProvenTxReqStatus::Sending,
            "ServiceError must flip req to Sending so TaskSendWaiting retries"
        );
        assert_eq!(
            req.attempts, 1,
            "ServiceError must bump req.attempts (was 0, expected 1)"
        );

        // tx: remains (or is set to) Sending
        let tx = get_tx(&manager, &txid_hex).await;
        assert_eq!(
            tx.status,
            TransactionStatus::Sending,
            "ServiceError must leave tx at Sending for monitor retry"
        );

        // Calling again must bump attempts to 2 (covers the case where a
        // second broadcast attempt also returns a service error).
        apply_service_error_outcome(
            &manager,
            &txid_hex,
            vec!["network unreachable".to_string()],
        )
        .await
        .expect("second service error transition should succeed");

        let req2 = get_req(&manager, &txid_hex).await;
        assert_eq!(
            req2.attempts, 2,
            "Second ServiceError must bump attempts from 1 to 2"
        );
    }

    // -----------------------------------------------------------------------
    // Suite 5: reconcile_tx_status wrapper-failure handling (Bug #6)
    //
    // When get_status_for_txids fails at the wrapper level (all providers
    // down, network timeout, etc.), we cannot conclude the tx is invalid.
    // reconcile_tx_status must downgrade the outcome to ServiceError so the
    // retry path runs instead of locking in a spurious InvalidTx verdict.
    //
    // We exercise this through handle_permanent_broadcast_failure — its
    // observable contract is that it returns the effective outcome after
    // reconciliation, which the test asserts is ServiceError, NOT InvalidTx.
    // -----------------------------------------------------------------------

    /// Mock services whose `get_status_for_txids` reports a wrapper-level
    /// failure: status="error", empty results, error=Some("all providers down").
    struct WrapperFailureStatusMock;
    #[async_trait]
    impl WalletServices for WrapperFailureStatusMock {
        fn chain(&self) -> Chain {
            Chain::Test
        }
        async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_merkle_path(
            &self,
            _txid: &str,
            _use_next: bool,
        ) -> types::GetMerklePathResult {
            types::GetMerklePathResult {
                name: Some("mock".to_string()),
                merkle_path: None,
                header: None,
                error: None,
            }
        }
        async fn get_raw_tx(&self, txid: &str, _use_next: bool) -> types::GetRawTxResult {
            types::GetRawTxResult {
                txid: txid.to_string(),
                name: Some("mock".to_string()),
                raw_tx: None,
                error: None,
            }
        }
        async fn post_beef(
            &self,
            _beef: &[u8],
            _txids: &[String],
        ) -> Vec<types::PostBeefResult> {
            vec![]
        }
        async fn get_utxo_status(
            &self,
            _output: &str,
            _output_format: Option<types::GetUtxoStatusOutputFormat>,
            _outpoint: Option<&str>,
            _use_next: bool,
        ) -> types::GetUtxoStatusResult {
            types::GetUtxoStatusResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                is_utxo: Some(false),
                details: vec![],
            }
        }
        async fn get_status_for_txids(
            &self,
            _txids: &[String],
            _use_next: bool,
        ) -> types::GetStatusForTxidsResult {
            types::GetStatusForTxidsResult {
                name: "mock".to_string(),
                status: "error".to_string(),
                error: Some("all providers down".to_string()),
                results: vec![],
            }
        }
        async fn get_script_hash_history(
            &self,
            _hash: &str,
            _use_next: bool,
        ) -> types::GetScriptHashHistoryResult {
            types::GetScriptHashHistoryResult {
                name: "mock".to_string(),
                status: "success".to_string(),
                error: None,
                history: vec![],
            }
        }
        async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
            Ok(vec![0u8; 80])
        }
        async fn get_height(&self) -> WalletResult<u32> {
            Ok(100_000)
        }
        async fn n_lock_time_is_final(
            &self,
            _input: types::NLockTimeInput,
        ) -> WalletResult<bool> {
            Ok(true)
        }
        async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
            Ok(types::BsvExchangeRate::default())
        }
        async fn get_fiat_exchange_rate(
            &self,
            _currency: &str,
            _base: Option<&str>,
        ) -> WalletResult<f64> {
            Ok(1.0)
        }
        async fn get_fiat_exchange_rates(
            &self,
            _target_currencies: &[String],
        ) -> WalletResult<types::FiatExchangeRates> {
            Ok(types::FiatExchangeRates::default())
        }
        fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
            types::ServicesCallHistory { services: vec![] }
        }
        async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
            Err(WalletError::NotImplemented("mock".to_string()))
        }
        fn hash_output_script(&self, _script: &[u8]) -> String {
            String::new()
        }
        async fn is_utxo(
            &self,
            _locking_script: &[u8],
            _txid: &str,
            _vout: u32,
        ) -> WalletResult<bool> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn reconcile_tx_status_returns_service_error_on_wrapper_failure() {
        // Exercise reconcile_tx_status via its only public caller,
        // handle_permanent_broadcast_failure. With an InvalidTx input and a
        // wrapper-failing status mock, the returned effective outcome must be
        // ServiceError (NOT InvalidTx) and the error detail must propagate.
        let (manager, concrete) = build_manager_pair().await;
        let services = Arc::new(WrapperFailureStatusMock);

        let (txid, _tx_id, parent_ids, failed_output_ids) =
            seed_failing_tx_with_consumed_inputs(&manager, &concrete, 1, true).await;

        let result = handle_permanent_broadcast_failure(
            &manager,
            &*services,
            &txid,
            &BroadcastOutcome::InvalidTx {
                details: vec!["provider said reject".to_string()],
            },
        )
        .await
        .expect("handler should succeed");

        match result {
            BroadcastOutcome::ServiceError { details } => {
                assert!(
                    details.iter().any(|d| d.contains("all providers down")),
                    "wrapper error detail must propagate into ServiceError: {:?}",
                    details
                );
            }
            other => panic!(
                "wrapper failure must downgrade InvalidTx to ServiceError, got {:?}",
                other
            ),
        }

        // And the downgraded ServiceError must have applied the retry
        // transition: req -> Sending with attempts bumped. The tx row is
        // at Sending because the seed helper sets it there.
        let req = get_req(&manager, &txid).await;
        assert_eq!(
            req.status,
            ProvenTxReqStatus::Sending,
            "downgraded ServiceError must flip req to Sending"
        );
        assert!(
            req.attempts >= 1,
            "downgraded ServiceError must bump req.attempts (got {})",
            req.attempts
        );

        // Parent inputs and failed-tx outputs must NOT have been touched —
        // the InvalidTx failure path was short-circuited before its writes.
        for pid in &parent_ids {
            let o = get_output(&manager, *pid).await;
            assert!(
                !o.spendable,
                "wrapper-failure downgrade must NOT restore inputs (parent_id={})",
                pid
            );
        }
        for oid in &failed_output_ids {
            let o = get_output(&manager, *oid).await;
            assert!(
                o.spendable,
                "wrapper-failure downgrade must NOT mark failed-tx outputs unspendable (output_id={})",
                oid
            );
        }
    }

}
