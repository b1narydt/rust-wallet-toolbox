//! Multi-writer double-spend reproduction for the UTXO allocation claim path.
//!
//! The hazard: the allocation SELECT reads spendable outputs with no row lock,
//! and the claim UPDATE is keyed on the primary key ONLY —
//! `UPDATE outputs SET spendable=0, spentBy=? WHERE outputId = ?` — with no
//! `AND spendable = 1`, no `AND spentBy IS NULL`, and no inspection of
//! `rows_affected`. The claim is therefore a blind write, not a
//! compare-and-swap. The only guard above it is `spend_lock`, a
//! `tokio::sync::Mutex`, which serializes within ONE process only.
//!
//! Two writers over one shared store both read the same spendable output, both
//! write it by primary key, and both are told they succeeded.
//!
//! `tb_claim_repro_second_claim_overwrites_first` demonstrates the missing
//! compare-and-swap DETERMINISTICALLY on SQLite, by interleaving the two
//! writers' reads and writes explicitly. That is the mechanism of the bug:
//! real concurrency merely makes this interleaving happen by accident.
//!
//! The Postgres tests additionally drive two genuinely concurrent
//! `WalletStorageManager`s — separate pools, separate `spend_lock`s — the way a
//! multi-writer deployment does. They need a server, so they are `#[ignore]`:
//!   cargo test --features postgres --test tb_claim_double_spend_repro -- --ignored --test-threads=1

use chrono::Utc;

use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::action_types::{
    StorageCreateActionArgs, StorageCreateActionOptions, StorageCreateActionOutput,
};
use bsv_wallet_toolbox::storage::find_args::*;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::StorageConfig;
use bsv_wallet_toolbox::tables::*;
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};

const IDENTITY: &str = "02claim1111111111111111111111111111111111111111111111111111111111";
const SOURCE_TXID: &str = "aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444";

/// Fresh schema, one user, one default basket, and `n` spendable change UTXOs
/// of `sats` each whose parent transaction is `completed`.
async fn seed<S>(storage: &S, n: i32, sats: i64) -> (i64, i64)
where
    S: StorageProvider + StorageReaderWriter + StorageReader,
{
    storage.migrate_database().await.expect("migrate");
    storage.drop_all_data().await.expect("drop_all_data");
    storage.make_available().await.expect("make_available");

    let (user, _) = storage
        .find_or_insert_user(IDENTITY, None)
        .await
        .expect("user");
    let user_id = user.user_id;
    let basket = storage
        .find_or_insert_output_basket(user_id, "default", None)
        .await
        .expect("basket");

    let now = Utc::now().naive_utc();
    let source_tx = Transaction {
        created_at: now,
        updated_at: now,
        transaction_id: 0,
        user_id,
        proven_tx_id: None,
        status: TransactionStatus::Completed,
        reference: "tb_claim_source".to_string(),
        is_outgoing: false,
        satoshis: sats * n as i64,
        description: "tb-claim source tx".to_string(),
        version: Some(1),
        lock_time: Some(0),
        txid: Some(SOURCE_TXID.to_string()),
        input_beef: None,
        raw_tx: None,
    };
    let source_tx_id = storage
        .insert_transaction(&source_tx, None)
        .await
        .expect("source tx");

    for i in 0..n {
        let output = Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id,
            transaction_id: source_tx_id,
            basket_id: Some(basket.basket_id),
            spendable: true,
            change: true,
            output_description: Some(format!("tb-claim utxo {i}")),
            vout: i,
            satoshis: sats,
            provided_by: StorageProvidedBy::Storage,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: Some(SOURCE_TXID.to_string()),
            sender_identity_key: None,
            derivation_prefix: Some("tbclaimprefix".to_string()),
            derivation_suffix: Some(format!("suffix{i}")),
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: Some(25),
            script_offset: None,
            locking_script: Some(vec![
                0x76, 0xa9, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0xac,
            ]),
        };
        storage.insert_output(&output, None).await.expect("utxo");
    }

    (user_id, basket.basket_id)
}

/// Insert an `unsigned` spending transaction, to be referenced by `spentBy`.
/// `outputs.spentBy` is a foreign key onto `transactions`, so a claim can only
/// name a transaction that exists.
async fn new_spender<S>(storage: &S, user_id: i64, reference: &str) -> i64
where
    S: StorageProvider + StorageReaderWriter + StorageReader,
{
    let now = Utc::now().naive_utc();
    let tx = Transaction {
        created_at: now,
        updated_at: now,
        transaction_id: 0,
        user_id,
        proven_tx_id: None,
        status: TransactionStatus::Unsigned,
        reference: reference.to_string(),
        is_outgoing: true,
        satoshis: 0,
        description: format!("tb-claim spender {reference}"),
        version: Some(1),
        lock_time: Some(0),
        txid: None,
        input_beef: None,
        raw_tx: None,
    };
    storage
        .insert_transaction(&tx, None)
        .await
        .expect("spender tx")
}

#[allow(dead_code)]
fn payment_args(sats: u64) -> StorageCreateActionArgs {
    StorageCreateActionArgs {
        description: "tb-claim concurrent payment".to_string(),
        inputs: vec![],
        outputs: vec![StorageCreateActionOutput {
            locking_script: "76a914000000000000000000000000000000000000000088ac".to_string(),
            satoshis: sats,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        }],
        lock_time: 0,
        version: 1,
        labels: vec![],
        options: StorageCreateActionOptions::default(),
        input_beef: None,
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

// ===========================================================================
// SQLite — deterministic proof that the claim UPDATE is not a compare-and-swap.
//
// Runs in CI with no server. This is the mechanism of the double-spend:
// writer B claims an output that writer A has already claimed, and the store
// reports success to both.
// ===========================================================================
#[cfg(feature = "sqlite")]
mod sqlite_proof {
    use super::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;

    async fn sqlite_storage() -> WalletResult<SqliteStorage> {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        SqliteStorage::new_sqlite(config, Chain::Test).await
    }

    /// The two writers' steps against the claim primitive, interleaved
    /// explicitly:  A reads -> B reads -> A claims -> B claims.
    ///
    /// Both reads see the same spendable output. Before the fix both claims
    /// reported success and the same UTXO was spent twice.
    #[tokio::test]
    async fn tb_claim_repro_second_claim_overwrites_first() {
        let storage = sqlite_storage().await.expect("storage");
        let (user_id, _basket_id) = seed(&storage, 1, 100_000).await;

        let spendable = |uid: i64| FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(uid),
                spendable: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };

        // Writer A reads the candidate set.
        let seen_by_a = storage
            .find_outputs(&spendable(user_id), None)
            .await
            .expect("A read");
        // Writer B reads it too, before A has written anything.
        let seen_by_b = storage
            .find_outputs(&spendable(user_id), None)
            .await
            .expect("B read");

        assert_eq!(seen_by_a.len(), 1, "seeded exactly one spendable output");
        assert_eq!(
            seen_by_a[0].output_id, seen_by_b[0].output_id,
            "both writers planned against the same output"
        );
        let output_id = seen_by_a[0].output_id;

        let tx_a = new_spender(&storage, user_id, "tb_claim_a").await;
        let tx_b = new_spender(&storage, user_id, "tb_claim_b").await;

        // Writer A claims it.
        let rows_a = storage
            .mark_inputs_spent(&[output_id], tx_a, user_id, None)
            .await
            .expect("A claim");
        // Writer B claims the SAME output.
        let rows_b = storage
            .mark_inputs_spent(&[output_id], tx_b, user_id, None)
            .await
            .expect("B claim");

        let after = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        output_id: Some(output_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("read back");

        println!("=== claim primitive must be a compare-and-swap ===");
        println!("outputId       = {output_id}");
        println!("A rows_claimed = {rows_a}   (for transactionId {tx_a})");
        println!("B rows_claimed = {rows_b}   (for transactionId {tx_b})");
        println!("final spentBy  = {:?}", after[0].spent_by);

        assert_eq!(rows_a, 1, "A claimed the output");
        assert_eq!(
            rows_b, 0,
            "claim must be a compare-and-swap: B claimed an output already \
             claimed by A (rows={rows_b}), so one UTXO is spent by both \
             transaction {tx_a} and transaction {tx_b}"
        );
        assert_eq!(
            after[0].spent_by,
            Some(tx_a),
            "A's claim must stand; B must not silently overwrite it"
        );
    }

    /// The `spendable` half of the guard: an output already retired without a
    /// recorded spender must not be re-claimable.
    #[tokio::test]
    async fn tb_claim_repro_unspendable_output_cannot_be_claimed() {
        let storage = sqlite_storage().await.expect("storage");
        let (user_id, _basket_id) = seed(&storage, 1, 100_000).await;

        let found = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(user_id),
                        spendable: Some(true),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("read");
        let output_id = found[0].output_id;

        // Retire the output without recording a spender (abort/relinquish shape).
        let rows_retire = storage
            .update_output(
                output_id,
                &OutputPartial {
                    spendable: Some(false),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("retire");
        assert_eq!(rows_retire, 1);

        let tx_c = new_spender(&storage, user_id, "tb_claim_c").await;
        let rows_claim = storage
            .mark_inputs_spent(&[output_id], tx_c, user_id, None)
            .await
            .expect("claim");

        println!("=== claim of an already-unspendable output ===");
        println!("rows_claimed = {rows_claim}");
        assert_eq!(
            rows_claim, 0,
            "claim must reject an output that is no longer spendable"
        );
    }

    #[tokio::test]
    async fn tb_claim_tenant_guard_rejects_foreign_user() {
        let storage = sqlite_storage().await.expect("storage");
        let (owner_user_id, _basket_id) = seed(&storage, 1, 100_000).await;

        let output_id = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(owner_user_id),
                        spendable: Some(true),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("find owner output")[0]
            .output_id;
        let (foreign_user, _) = storage
            .find_or_insert_user("foreign_claim_identity", None)
            .await
            .expect("foreign user");
        let foreign_tx = new_spender(&storage, foreign_user.user_id, "foreign_claim").await;

        let claimed = storage
            .mark_inputs_spent(&[output_id], foreign_tx, foreign_user.user_id, None)
            .await
            .expect("foreign claim");

        assert_eq!(claimed, 0, "a foreign tenant must not claim the output");
        let after = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        output_id: Some(output_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("read owner output");
        assert!(after[0].spendable);
        assert!(after[0].spent_by.is_none());
    }

    /// A partial claim must be reported as partial: of two planned outputs,
    /// one already taken, the rowcount must be 1 — that mismatch against the
    /// planned count of 2 is what triggers the replan.
    #[tokio::test]
    async fn tb_claim_repro_partial_claim_reports_partial_rowcount() {
        let storage = sqlite_storage().await.expect("storage");
        let (user_id, _basket_id) = seed(&storage, 2, 50_000).await;

        let found = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(user_id),
                        spendable: Some(true),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("read");
        assert_eq!(found.len(), 2);
        let (first, second) = (found[0].output_id, found[1].output_id);

        let tx_a = new_spender(&storage, user_id, "tb_claim_partial_a").await;
        let tx_b = new_spender(&storage, user_id, "tb_claim_partial_b").await;

        // A competing writer takes the first output.
        let taken = storage
            .mark_inputs_spent(&[first], tx_a, user_id, None)
            .await
            .expect("competitor claim");
        assert_eq!(taken, 1);

        // Our writer planned BOTH; only one is still free.
        let claimed = storage
            .mark_inputs_spent(&[first, second], tx_b, user_id, None)
            .await
            .expect("our claim");

        println!("=== partial claim ===");
        println!("planned 2, claimed {claimed}");
        assert_eq!(
            claimed, 1,
            "rowcount must report only the outputs actually claimed, so the \
             caller can detect the mismatch and replan"
        );
    }
}

// ===========================================================================
// Postgres — two genuinely concurrent managers over one shared store.
//
// Needs a server; `#[ignore]`d. This is the deployment shape the fix targets:
// separate processes, separate pools, separate `spend_lock`s.
// ===========================================================================
#[cfg(feature = "postgres")]
mod postgres_race {
    use super::*;
    use bsv_wallet_toolbox::storage::sqlx_impl::PgStorage;
    use bsv_wallet_toolbox::storage::{WalletStorageManager, WalletStorageProvider};
    use bsv_wallet_toolbox::wallet::types::AuthId;
    use std::sync::Arc;

    async fn pg_storage() -> WalletResult<PgStorage> {
        let url = std::env::var("POSTGRES_DATABASE_URL")
            .expect("POSTGRES_DATABASE_URL must be set to run this reproduction");
        let config = StorageConfig {
            url,
            ..Default::default()
        };
        PgStorage::new_postgres(config, Chain::Test).await
    }

    #[tokio::test]
    #[ignore]
    async fn tb_claim_repro_pg_second_claim_overwrites_first() {
        let storage = pg_storage().await.expect("storage");
        let (user_id, _basket_id) = seed(&storage, 1, 100_000).await;

        let found = StorageReader::find_outputs(
            &storage,
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_id),
                    spendable: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .expect("read");
        let output_id = found[0].output_id;
        let tx_a = new_spender(&storage, user_id, "tb_claim_pg_a").await;
        let tx_b = new_spender(&storage, user_id, "tb_claim_pg_b").await;

        let rows_a =
            StorageReaderWriter::mark_inputs_spent(&storage, &[output_id], tx_a, user_id, None)
                .await
                .expect("A");
        let rows_b =
            StorageReaderWriter::mark_inputs_spent(&storage, &[output_id], tx_b, user_id, None)
                .await
                .expect("B");

        println!("pg: rows_a={rows_a} rows_b={rows_b}");
        assert_eq!(rows_a, 1);
        assert_eq!(rows_b, 0, "claim UPDATE must be a compare-and-swap");
    }

    /// Two managers, two `spend_lock`s, one funded UTXO, concurrent
    /// `create_action`. Exactly one may claim it.
    #[tokio::test]
    #[ignore]
    async fn tb_claim_repro_pg_two_managers_one_utxo() {
        let setup = pg_storage().await.expect("setup");
        let (user_id, _basket_id) = seed(&setup, 1, 100_000).await;
        drop(setup);

        let store_a: Arc<dyn WalletStorageProvider> = Arc::new(pg_storage().await.expect("A"));
        let store_b: Arc<dyn WalletStorageProvider> = Arc::new(pg_storage().await.expect("B"));

        let mgr_a = Arc::new(WalletStorageManager::new(
            IDENTITY.to_string(),
            Some(store_a),
            vec![],
        ));
        let mgr_b = Arc::new(WalletStorageManager::new(
            IDENTITY.to_string(),
            Some(store_b),
            vec![],
        ));
        mgr_a.make_available().await.expect("A available");
        mgr_b.make_available().await.expect("B available");

        let auth = AuthId {
            identity_key: IDENTITY.to_string(),
            user_id: Some(user_id),
            is_active: Some(true),
        };

        // Each side holds ITS OWN spend_lock for the whole operation, as the
        // wallet layer does. The two locks do not exclude one another.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let run = |mgr: Arc<WalletStorageManager>,
                   auth: AuthId,
                   barrier: Arc<tokio::sync::Barrier>| async move {
            let _spend_guard = mgr.acquire_spend_lock().await.expect("spend lock");
            barrier.wait().await;
            mgr.create_action(&auth, &payment_args(5_000)).await
        };

        let (res_a, res_b) = tokio::join!(
            run(mgr_a.clone(), auth.clone(), barrier.clone()),
            run(mgr_b.clone(), auth.clone(), barrier.clone()),
        );

        let outpoints = |r: &WalletResult<
            bsv_wallet_toolbox::storage::action_types::StorageCreateActionResult,
        >| match r {
            Ok(v) => Some(
                v.inputs
                    .iter()
                    .map(|i| format!("{}:{}", i.source_txid, i.source_vout))
                    .collect::<Vec<_>>(),
            ),
            Err(_) => None,
        };
        println!("=== two managers, one UTXO ===");
        println!("A: {:?}", outpoints(&res_a));
        println!("B: {:?}", outpoints(&res_b));
        if let Err(e) = &res_a {
            println!("A error: {e}");
        }
        if let Err(e) = &res_b {
            println!("B error: {e}");
        }

        let winners = [&res_a, &res_b].iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            winners, 1,
            "exactly one manager may fund from a single UTXO; \
             both succeeding is a double spend"
        );
    }
}
