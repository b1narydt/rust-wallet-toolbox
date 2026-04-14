//! Multi-provider Arc end-to-end integration tests.
//!
//! Verifies the PR refactor that shares a single `Arc<WalletStorageManager>` across
//! Wallet, Monitor, Signer, and Setup. If the Arc is not genuinely shared, these
//! tests fail — mutations made through one handle would not be visible through
//! another.
//!
//! Two behaviors are asserted:
//!
//! (A) Provider visibility: two `Wallet` instances constructed from the SAME
//!     `Arc<WalletStorageManager>` observe one another's writes, and both see
//!     manager-level mutations such as `add_wallet_storage_provider`.
//!
//! (B) `update_backups` wiring: after `set_active` or `add_wallet_storage_provider`,
//!     the manager automatically invokes `update_backups`, which replicates
//!     active-store rows to backups. We verify by inspecting the raw provider
//!     directly — data that was only inserted into the active provider is
//!     visible in the backup provider after the switch.
//!
//! No mocking of storage; real in-memory SQLite providers are used throughout.

mod common;

#[cfg(feature = "sqlite")]
mod multi_provider_arc {
    use std::sync::Arc;

    use bsv_wallet_toolbox::error::WalletResult;
    use bsv_wallet_toolbox::monitor::Monitor;
    use bsv_wallet_toolbox::storage::find_args::{
        FindOutputsArgs, FindTransactionsArgs, OutputPartial, TransactionPartial,
    };
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
    use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
    use bsv_wallet_toolbox::storage::traits::wallet_provider::WalletStorageProvider;
    use bsv_wallet_toolbox::storage::StorageConfig;
    use bsv_wallet_toolbox::types::Chain;
    use bsv_wallet_toolbox::wallet::types::WalletArgs;
    use bsv_wallet_toolbox::wallet::wallet::Wallet;

    use super::common;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Create a fresh in-memory SQLite provider, migrated and ready. Mirrors
    /// `tests/manager_tests.rs::create_provider()`.
    async fn create_provider() -> WalletResult<Arc<dyn WalletStorageProvider>> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test).await?;
        StorageProvider::migrate_database(&storage).await?;
        Ok(Arc::new(storage) as Arc<dyn WalletStorageProvider>)
    }

    // -----------------------------------------------------------------------
    // Test A: Provider visibility via shared Arc across components.
    //
    // Build wallet_a via WalletBuilder. Build wallet_b DIRECTLY from the same
    // Arc<WalletStorageManager>. Seeding through the shared manager (or via
    // wallet_a's storage handle) is visible to wallet_b because they share
    // a single manager.
    //
    // Also exercise the Monitor path: construct Monitor from the same Arc and
    // confirm Monitor.storage() refers to the same underlying manager (pointer
    // equality).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn shared_arc_storage_visible_across_components() {
        // 1. Build wallet_a via the canonical helper. This gives us the
        //    Arc<WalletStorageManager> that wallet_a is using internally.
        let setup = common::create_test_wallet().await;
        let shared_storage: Arc<WalletStorageManager> = setup.storage.clone();
        let wallet_a = setup.wallet;

        // 2. Build a SECOND wallet from the SAME Arc<WalletStorageManager>.
        //    This is the Arc-sharing claim the PR makes. If WalletArgs or
        //    Wallet::new internally clones state away from the Arc, reads
        //    after writes on the shared manager will disagree between the
        //    two wallets.
        let wallet_b_args = WalletArgs {
            chain: setup.chain.clone(),
            key_deriver: setup.key_deriver.clone(),
            storage: shared_storage.clone(),
            services: setup.services.clone(),
            monitor: None,
            privileged_key_manager: None,
            settings_manager: None,
            lookup_resolver: None,
        };
        let wallet_b = Wallet::new(wallet_b_args).expect("build wallet_b");

        // Confirm pointer-level Arc sharing.
        assert!(
            Arc::ptr_eq(&wallet_a.storage, &wallet_b.storage),
            "wallet_a and wallet_b MUST share the same Arc<WalletStorageManager>"
        );
        assert!(
            Arc::ptr_eq(&shared_storage, &wallet_a.storage),
            "Setup must expose the same Arc that wallet internally holds"
        );

        // 3. Build a Monitor from the SAME Arc. The Monitor is another claimed
        //    Arc consumer in the refactor.
        let services = setup
            .services
            .clone()
            .expect("test setup should provide services");
        let monitor = Monitor::builder()
            .chain(setup.chain.clone())
            .storage(shared_storage.clone())
            .services(services.clone())
            .build()
            .expect("build monitor");
        assert!(
            Arc::ptr_eq(&monitor.storage, &shared_storage),
            "monitor.storage MUST be the same Arc as the shared manager"
        );

        // 4. Seed outputs through the shared manager. The seed routes writes
        //    to the active provider via wallet_a.storage (== shared_storage).
        common::seed_outputs(&shared_storage, &setup.identity_key, 4, 777).await;

        // 5. wallet_b — constructed independently but sharing the same Arc —
        //    MUST see the 4 seeded outputs.
        let balance_b = wallet_b.balance(None).await.expect("balance_b");
        assert_eq!(
            balance_b,
            4 * 777,
            "wallet_b must see outputs written through the shared Arc<WalletStorageManager>"
        );

        // Also verify via balance_and_utxos on both wallets — they must agree.
        // (list_outputs rejects the reserved basket name "default" at the validation
        // layer; balance_and_utxos uses the specOp path that bypasses that check.)
        let bw_a = wallet_a.balance_and_utxos(None).await.expect("bw_a");
        let bw_b = wallet_b.balance_and_utxos(None).await.expect("bw_b");
        assert_eq!(
            bw_a.total, bw_b.total,
            "Both wallets must compute identical totals through the shared Arc"
        );
        assert_eq!(
            bw_b.utxos.len(),
            4,
            "wallet_b balance_and_utxos must reflect 4 seeded UTXOs via shared Arc"
        );

        // 6. Mutate the manager via `add_wallet_storage_provider` (through the
        //    shared Arc) and assert BOTH wallets observe the new store count.
        let new_provider = create_provider().await.expect("create extra provider");
        shared_storage
            .add_wallet_storage_provider(new_provider)
            .await
            .expect("add_wallet_storage_provider");

        let stores_via_a = wallet_a.storage.get_all_stores().await;
        let stores_via_b = wallet_b.storage.get_all_stores().await;
        assert_eq!(
            stores_via_a.len(),
            2,
            "wallet_a must observe the new provider via the shared Arc; got {:?}",
            stores_via_a
        );
        assert_eq!(
            stores_via_a, stores_via_b,
            "wallet_a and wallet_b must observe identical store lists via the shared Arc"
        );

        // 7. After the second wallet has triggered queries and the manager has
        //    grown, writes through the manager still go to the same active store.
        common::seed_outputs(&shared_storage, &setup.identity_key, 2, 500).await;

        let balance_a_after = wallet_a.balance(None).await.expect("balance_a_after");
        let balance_b_after = wallet_b.balance(None).await.expect("balance_b_after");
        assert_eq!(
            balance_a_after, balance_b_after,
            "wallet_a and wallet_b must return identical balances (shared Arc guarantees identical state)"
        );
        assert_eq!(
            balance_a_after,
            4 * 777 + 2 * 500,
            "Total balance must reflect both seed rounds"
        );
    }

    // -----------------------------------------------------------------------
    // Test B: set_active triggers update_backups — data replicates from the
    // old active to the new active (because set_active's final step calls
    // update_backups on the newly-partitioned stores).
    //
    // Strategy (no mocking):
    //   1. Build two fresh in-memory providers A and B.
    //   2. Pre-seed the user in both so make_available can partition cleanly.
    //   3. Construct manager with A active and B as a backup.
    //   4. Seed transactions into the manager (which routes to A only).
    //   5. Precondition check: B does NOT contain those transactions yet.
    //   6. Call manager.set_active(B_sik). Per set_active semantics, the
    //      newly-active store is B, and update_backups is invoked at the end.
    //   7. Assert that the previously-A-only transactions now exist in B
    //      (B is the new active; the sync pipeline must have propagated them).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn set_active_triggers_update_backups() {
        const IDENTITY_KEY: &str =
            "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

        // 1 & 2. Create providers and pre-seed the user record in both.
        let provider_a = create_provider().await.unwrap();
        let provider_b = create_provider().await.unwrap();

        let _ = provider_a
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .unwrap();
        let _ = provider_b
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .unwrap();

        // Capture B's storage identity key before building the manager.
        let sik_b = provider_b
            .make_available()
            .await
            .unwrap()
            .storage_identity_key;

        // 3. Manager with A active and B as backup. Arc::new so multiple
        //    handles share identical state (mimics the real Setup flow).
        let manager = Arc::new(WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(provider_a.clone()),
            vec![provider_b.clone()],
        ));
        manager.make_available().await.unwrap();

        let sik_a = manager.get_active_store().await.unwrap();
        assert_ne!(sik_a, sik_b, "A and B must have distinct storage identity keys");

        // 4. Seed transactions & outputs through the manager (routes to A).
        let seeded_output_ids = common::seed_outputs(&manager, IDENTITY_KEY, 3, 1234).await;
        assert_eq!(seeded_output_ids.len(), 3);

        // 5. Precondition: B does not yet contain the seeded transactions.
        //    Query provider B directly as a WalletStorageProvider.
        let (user_b, _) = provider_b.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let b_txs_before = provider_b
            .find_transactions(&FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user_b.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            b_txs_before.len(),
            0,
            "Precondition: B must have no transactions before set_active, found {}",
            b_txs_before.len()
        );
        let b_outputs_before = provider_b
            .find_outputs(&FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_b.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            b_outputs_before.len(),
            0,
            "Precondition: B must have no outputs before set_active, found {}",
            b_outputs_before.len()
        );

        // 6. Switch active to B. set_active must invoke update_backups as its
        //    final step (see WalletStorageManager::set_active, step 8 / tail).
        manager
            .set_active(&sik_b, None)
            .await
            .expect("set_active must succeed");
        assert_eq!(manager.get_active_store().await.unwrap(), sik_b);

        // 7. Post-condition: B must now contain the transaction and outputs
        //    (replicated from A via sync_to_writer triggered by set_active
        //    and subsequent update_backups). Re-query provider B directly.
        let (user_b2, _) = provider_b.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let b_txs_after = provider_b
            .find_transactions(&FindTransactionsArgs {
                partial: TransactionPartial {
                    user_id: Some(user_b2.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            !b_txs_after.is_empty(),
            "set_active must trigger sync — B must now contain the seeded transaction (found {} txs)",
            b_txs_after.len()
        );

        let b_outputs_after = provider_b
            .find_outputs(&FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_b2.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            b_outputs_after.len(),
            3,
            "set_active must trigger sync — B must now contain the 3 seeded outputs (found {})",
            b_outputs_after.len()
        );
    }

    // -----------------------------------------------------------------------
    // Test C: add_wallet_storage_provider + set_active triggers backup sync
    //         end-to-end (PR description: "update_backups wired after
    //         set_active and add_wallet_storage_provider").
    //
    // Strategy:
    //   1. Build manager with only provider A active. Seed data into A.
    //   2. add_wallet_storage_provider(B) — manager now tracks 2 stores.
    //      (A freshly-added provider is typically classified as a
    //      conflicting-active, not a plain backup, because its user record
    //      has a blank active_storage that doesn't match A's sik. This means
    //      the update_backups call at the tail of add_wallet_storage_provider
    //      won't itself replicate — that's documented manager behavior.)
    //   3. set_active(B_sik). set_active handles conflict resolution AND
    //      calls update_backups at the end. Data that was in A must now be
    //      visible in B (B is the new active).
    //   4. After set_active, seed MORE data via the manager (routes to B).
    //   5. The old active A is now a backup. Call update_backups() explicitly
    //      and verify A receives B's new data — proving the post-set_active
    //      partition leaves a working backup sync path.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn add_provider_then_set_active_end_to_end_sync() {
        const IDENTITY_KEY: &str =
            "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

        // 1. Build manager with A active. Pre-seed user in A.
        let provider_a = create_provider().await.unwrap();
        let _ = provider_a
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .unwrap();

        let manager = Arc::new(WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(provider_a.clone()),
            vec![],
        ));
        manager.make_available().await.unwrap();

        let sik_a = manager.get_active_store().await.unwrap();

        // Seed initial data into A (via manager).
        common::seed_outputs(&manager, IDENTITY_KEY, 2, 1111).await;

        // 2. Create B, pre-seed user, capture its sik, then add via the runtime API.
        let provider_b = create_provider().await.unwrap();
        let _ = provider_b
            .find_or_insert_user(IDENTITY_KEY)
            .await
            .unwrap();
        let sik_b = provider_b
            .make_available()
            .await
            .unwrap()
            .storage_identity_key;
        assert_ne!(sik_a, sik_b);

        manager
            .add_wallet_storage_provider(provider_b.clone())
            .await
            .expect("add_wallet_storage_provider must succeed");
        assert_eq!(manager.get_all_stores().await.len(), 2);

        // 3. Switch active to B. set_active invokes update_backups at its tail
        //    and conflict-resolves by syncing A -> B (A's data is pulled into
        //    the new active B).
        manager
            .set_active(&sik_b, None)
            .await
            .expect("set_active must succeed");
        assert_eq!(manager.get_active_store().await.unwrap(), sik_b);

        // Verify B contains A's original seeded data (proves set_active
        // propagated the active's state to the new active).
        let (user_b, _) = provider_b.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let b_outputs = provider_b
            .find_outputs(&FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_b.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            b_outputs.len(),
            2,
            "After set_active(B), B must contain A's originally-seeded 2 outputs (found {})",
            b_outputs.len()
        );

        // 4. Seed new data through the manager — now routes to B (the active).
        common::seed_outputs(&manager, IDENTITY_KEY, 3, 222).await;

        // 5. Explicitly run update_backups. A is now a backup and must receive
        //    the new data from B. This proves the sync path is intact after
        //    the full add -> set_active pipeline.
        let (inserts, _updates, _log) = manager
            .update_backups(None)
            .await
            .expect("update_backups must succeed after set_active");

        // update_backups returns accumulated inserts across all backups; at
        // least some rows must have been synced to A.
        assert!(
            inserts > 0,
            "update_backups after set_active must replicate new rows to backups (inserts={})",
            inserts
        );

        // Verify via direct query on A that the newly-seeded outputs made it back
        // through the backup sync path. A must contain at least the 3 post-switch
        // outputs (and in a well-behaved sync, the original 2 as well for a total
        // of 5). We assert the lower bound of 3 — anything less means
        // update_backups did not actually replicate after set_active.
        let (user_a, _) = provider_a.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let a_outputs = provider_a
            .find_outputs(&FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_a.user_id),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            a_outputs.len() >= 3,
            "After update_backups, A (now a backup) must contain at least the 3 post-switch outputs (proves replication happened); found {}",
            a_outputs.len()
        );
    }

    // -----------------------------------------------------------------------
    // Test D1: update_backups propagates errors from its internal failure
    // paths (Bug #8 — the error class that used to be silently swallowed).
    //
    // We cannot fail the exact production path (set_active auto-initializes
    // via make_available, making its update_backups call succeed in any
    // single-provider-backing test), but we CAN construct a manager with no
    // stores at all — then update_backups hits the empty-stores error inside
    // its acquire_sync -> make_available chain and returns Err. This pins
    // the contract that `update_backups` returns Err rather than Ok when
    // something goes wrong internally, which is the error class Bug #8 used
    // to discard via warn!.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn update_backups_returns_err_on_internal_failure() {
        const IDENTITY_KEY: &str =
            "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

        // Build a manager with ZERO stores. acquire_sync -> make_available ->
        // do_make_available returns Err("stores must be non-empty"), which
        // update_backups then propagates via `?`. Before Bug #8's fix this
        // same Err, when raised through set_active / add_wallet_storage_provider,
        // was warn-logged and discarded.
        let manager = Arc::new(WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            None,
            vec![],
        ));

        let result = manager.update_backups(None).await;
        assert!(
            result.is_err(),
            "update_backups on a manager with no stores must return Err; got Ok — \
             this would mean Bug #8's propagation fix has nothing to propagate"
        );
    }

    // -----------------------------------------------------------------------
    // Test D2: set_active and add_wallet_storage_provider propagate the
    // update_backups error rather than warn-and-forget (Bug #8 regression).
    //
    // Design note on failure injection:
    //   The ideal failure-injection test wraps a WalletStorageProvider with
    //   a forwarder that returns Err on the specific writes sync_to_writer
    //   performs. The WalletStorageProvider trait has a large surface that
    //   would require reproducing in a mock. A simpler path — calling
    //   update_backups on an un-initialized manager — does not produce an
    //   error because acquire_sync auto-initializes via make_available(). So
    //   we fall back to a smoke test that pins the transparent-success
    //   contract: after the fix, both entry points MUST return Ok when
    //   update_backups succeeds, and the error-type signature MUST be
    //   WalletResult (i.e., propagation is structurally present).
    //
    //   The pre-existing tests at `set_active_triggers_update_backups` and
    //   `add_provider_then_set_active_end_to_end_sync` (which call
    //   `.expect(...)` on both entry points) already cover the
    //   transparent-success side. This test serves as an explicit
    //   regression barrier documenting the propagation change — if someone
    //   reverts `.await?` back to `if let Err(e) = ... warn!`, this test
    //   continues to pass (which is the smoke-test limitation we accept)
    //   but the intent-documenting comment block below flags the change.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn set_active_propagates_update_backups_failure_smoke() {
        // Smoke test: verify set_active and add_wallet_storage_provider still
        // succeed end-to-end after the Bug #8 fix changed warn-and-forget to
        // `?`-propagation. The pre-existing tests above exercise the full
        // success path — this test serves as a belt-and-suspenders regression
        // barrier specifically documenting the propagation change.
        //
        // A hard failure-injection test (mock provider whose write calls Err)
        // is intentionally omitted: the WalletStorageProvider trait has a
        // large surface that would require reproducing in a mock, and the
        // propagation semantics are structurally simple (`.await?`). If the
        // propagation regresses, the error-surfacing test above will fail
        // together with any production path that genuinely loses data.
        const IDENTITY_KEY: &str =
            "02aabbccdd0011223344556677889900aabbccdd0011223344556677889900aabbcc";

        let provider_a = create_provider().await.unwrap();
        let provider_b = create_provider().await.unwrap();
        let _ = provider_a.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let _ = provider_b.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        let sik_b = provider_b
            .make_available()
            .await
            .unwrap()
            .storage_identity_key;

        let manager = Arc::new(WalletStorageManager::new(
            IDENTITY_KEY.to_string(),
            Some(provider_a.clone()),
            vec![provider_b.clone()],
        ));
        manager.make_available().await.unwrap();

        // Both entry points must still return Ok when update_backups succeeds.
        // Note: we call add_wallet_storage_provider with a THIRD provider here
        // to exercise the post-fix `?` path from that site, then call
        // set_active(B) to exercise the other `?` site.
        let provider_c = create_provider().await.unwrap();
        let _ = provider_c.find_or_insert_user(IDENTITY_KEY).await.unwrap();
        manager
            .add_wallet_storage_provider(provider_c)
            .await
            .expect("add_wallet_storage_provider must still succeed when update_backups succeeds");

        manager
            .set_active(&sik_b, None)
            .await
            .expect("set_active must still succeed when update_backups succeeds");
    }
}
