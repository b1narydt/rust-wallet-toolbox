//! Regression tests for rust-mpc#352 — BRC-95 closure of returned Atomic BEEF.
//!
//! The defect: `merge_input_beef_signer` fetched ancestor BEEF only for
//! `providedBy == storage` inputs, so a caller-named input (`providedBy: you`)
//! — e.g. a wallet-minted token output spent by outpoint — contributed NO
//! ancestor transaction to the assembled BEEF. The Atomic BEEF handed back by
//! createAction/signAction then violated the BRC-95 closure (the spend's
//! parent transaction was absent), and every downstream SPV verifier rejected
//! it.
//!
//! Shape (mirrors conformance/vectors/beef/beef_spend_closure.json): mint a
//! custom output in one action, spend it as a caller-named input in a second
//! action, and require the returned Atomic BEEF to contain the parent and to
//! satisfy the full closure — every transaction proven by a bump or chaining
//! (transitively, inside the BEEF) to one that is.
//!
//! Deterministic and offline: funding comes from the recorded mainnet
//! fixtures in the funded corpus, the chain tracker answers only from pinned
//! merkle roots, and any network touch panics (FixtureServices tripwire).

mod funded_common;

#[cfg(feature = "sqlite")]
mod beef_spend_closure {
    use std::collections::HashSet;
    use std::sync::Arc;

    use bsv::transaction::beef::Beef;
    use bsv::wallet::interfaces::{CreateActionArgs, WalletInterface};

    use crate::funded_common::{
        build_vector_wallet, from_hex, internalize_funding, FixtureServices, FundedFile, PostMode,
    };
    use bsv_wallet_toolbox::wallet::setup::SetupWallet;

    const CREATE_CORPUS: &str =
        include_str!("../conformance/vectors/wallet/brc100/createaction-funded.json");

    /// Build a funded offline wallet from the first funded-corpus vector's
    /// fixtures (root key, funding payments, pinned merkle roots).
    async fn funded_wallet() -> SetupWallet {
        let file: FundedFile =
            serde_json::from_str(CREATE_CORPUS).expect("funded corpus JSON must parse");
        let vector = file
            .vectors
            .iter()
            .find(|v| !v.input.funding_set.is_empty())
            .expect("a funded vector");
        let services = Arc::new(FixtureServices {
            roots: file.merkle_roots.clone(),
            post: PostMode::Tripwire,
        });
        let setup = build_vector_wallet(&vector.input.root_key, services)
            .await
            .expect("wallet build");
        let funding = &file
            .funding_sets
            .get(&vector.input.funding_set)
            .expect("funding set present")
            .payments;
        for p in funding.iter() {
            internalize_funding(&setup, p).await.expect("internalize");
        }
        setup
    }

    /// Assert the BRC-95 closure over a parsed BEEF: every transaction is
    /// either proven by a bump it is actually a leaf of, or every one of its
    /// input txids is present in the BEEF and (transitively) valid. Txid-only
    /// entries are rejected — an Atomic BEEF returned to a caller must be
    /// verifiable standalone.
    fn assert_closure(beef: &Beef, context: &str) {
        let mut valid: HashSet<String> = HashSet::new();

        for btx in &beef.txs {
            assert!(
                !btx.is_txid_only(),
                "{context}: txid-only entry {} breaks standalone verifiability",
                btx.txid
            );
            if let Some(bump_idx) = btx.bump_index {
                let bump = beef.bumps.get(bump_idx).unwrap_or_else(|| {
                    panic!(
                        "{context}: {} names bump {} which does not exist",
                        btx.txid, bump_idx
                    )
                });
                let is_leaf = bump.path.first().is_some_and(|l0| {
                    l0.iter()
                        .any(|l| l.hash.as_deref() == Some(btx.txid.as_str()))
                });
                assert!(
                    is_leaf,
                    "{context}: {} names bump {} it is not a leaf of",
                    btx.txid, bump_idx
                );
                valid.insert(btx.txid.clone());
            }
        }

        // Fixpoint: unproven txs become valid once all their inputs are valid.
        loop {
            let mut progressed = false;
            for btx in &beef.txs {
                if valid.contains(&btx.txid) {
                    continue;
                }
                if !btx.input_txids.is_empty() && btx.input_txids.iter().all(|i| valid.contains(i))
                {
                    valid.insert(btx.txid.clone());
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        for btx in &beef.txs {
            assert!(
                valid.contains(&btx.txid),
                "{context}: {} does not chain to a proven ancestor inside the BEEF \
                 (inputs: {:?})",
                btx.txid,
                btx.input_txids
            );
        }
    }

    /// Mint an anyone-can-spend (OP_1 OP_EQUAL) output into a custom basket, then
    /// spend it as a caller-named input. The returned Atomic BEEF must
    /// contain the mint transaction (the spend's parent) and satisfy the
    /// full BRC-95 closure.
    #[tokio::test]
    async fn spend_of_caller_named_input_returns_closed_beef() {
        let setup = funded_wallet().await;

        // --- Mint: one OP_1 output into a custom basket ---
        let mint_args: CreateActionArgs = serde_json::from_value(serde_json::json!({
            "description": "mint op-true token",
            "outputs": [{
                "lockingScript": "5187",
                "satoshis": 1000,
                "outputDescription": "op-true token",
                "basket": "regression 352"
            }]
        }))
        .expect("mint args");
        let mint = setup
            .wallet
            .create_action(mint_args, None)
            .await
            .expect("mint createAction");
        let mint_txid = mint.txid.clone().expect("mint txid");
        let mint_beef_bytes = mint.tx.clone().expect("mint atomic beef");

        // The mint's own returned BEEF must already be closed.
        let mut cursor = std::io::Cursor::new(&mint_beef_bytes);
        let mint_beef = Beef::from_binary(&mut cursor).expect("mint beef parses");
        assert_eq!(mint_beef.atomic_txid.as_deref(), Some(mint_txid.as_str()));
        assert_closure(&mint_beef, "mint BEEF");

        // Find the token's vout (output order is randomized).
        let token_vout = {
            let btx = mint_beef.find_txid(&mint_txid).expect("mint tx in beef");
            let tx = btx.tx.as_ref().expect("mint tx full");
            tx.outputs
                .iter()
                .position(|o| o.locking_script.to_binary() == from_hex("5187"))
                .expect("op-true output present") as u32
        };

        // --- Spend: the token as a caller-named input (providedBy: you) ---
        let spend_args: CreateActionArgs = serde_json::from_value(serde_json::json!({
            "description": "spend op-true token",
            "inputs": [{
                "outpoint": format!("{mint_txid}.{token_vout}"),
                "unlockingScript": "51",
                "inputDescription": "spend op-true token"
            }],
            "outputs": [{
                "lockingScript": "51",
                "satoshis": 500,
                "outputDescription": "op-true remainder"
            }]
        }))
        .expect("spend args");
        let spend = setup
            .wallet
            .create_action(spend_args, None)
            .await
            .expect("spend createAction");
        let spend_txid = spend.txid.clone().expect("spend txid");
        let spend_beef_bytes = spend.tx.clone().expect("spend atomic beef");

        let mut cursor = std::io::Cursor::new(&spend_beef_bytes);
        let spend_beef = Beef::from_binary(&mut cursor).expect("spend beef parses");
        assert_eq!(spend_beef.atomic_txid.as_deref(), Some(spend_txid.as_str()));

        // The #352 regression: the caller-named input's parent transaction
        // must be present in the returned BEEF, as a full transaction.
        let parent = spend_beef.find_txid(&mint_txid).unwrap_or_else(|| {
            panic!(
                "spend BEEF is missing the caller-named input's parent {mint_txid} \
                 (rust-mpc#352 regression)"
            )
        });
        assert!(
            !parent.is_txid_only(),
            "parent {mint_txid} must carry its full transaction, not txid-only"
        );

        assert_closure(&spend_beef, "spend BEEF");
    }

    /// The TS-parity hydration entry point (`getBeefForTransaction`):
    /// storage must hydrate a COMPLETE BEEF for a wallet-known txid — the
    /// call one-shot / monitor-off processes rely on — and error naming the
    /// txid for one it cannot hydrate.
    #[tokio::test]
    async fn get_beef_for_transaction_hydrates_a_closed_beef() {
        use bsv_wallet_toolbox::storage::beef::get_beef_for_transaction;
        use std::collections::HashSet;

        let setup = funded_wallet().await;

        let mint_args: bsv::wallet::interfaces::CreateActionArgs =
            serde_json::from_value(serde_json::json!({
                "description": "mint for hydration",
                "outputs": [{
                    "lockingScript": "5187",
                    "satoshis": 900,
                    "outputDescription": "hydration token",
                    "basket": "regression 352"
                }]
            }))
            .expect("mint args");
        let mint = setup
            .wallet
            .create_action(mint_args, None)
            .await
            .expect("mint createAction");
        let mint_txid = mint.txid.expect("mint txid");

        let provider = setup.storage.get_active().await.expect("active storage");
        let beef = get_beef_for_transaction(provider.as_ref(), &mint_txid, &HashSet::new())
            .await
            .expect("hydration succeeds for a wallet-known txid");
        let subject = beef.find_txid(&mint_txid).expect("subject present");
        assert!(!subject.is_txid_only(), "subject must be a full tx");
        assert_closure(&beef, "hydrated BEEF");

        let unknown = "11".repeat(32);
        let err = get_beef_for_transaction(provider.as_ref(), &unknown, &HashSet::new())
            .await
            .expect_err("unknown txid must fail closed");
        assert!(
            err.to_string().contains(&unknown),
            "error must name the txid, got: {err}"
        );
    }

    /// The monitor's broadcast rebuild (rust-mpc#352 P3-a): the BEEF posted
    /// by attempt_to_post_reqs_to_network must be PLAIN (no Atomic frame)
    /// and dependency-sorted — the subject was merged before its source
    /// BEEFs, so without the explicit sort it preceded its parents on the
    /// wire.
    #[tokio::test]
    async fn monitor_rebuild_posts_plain_sorted_beef() {
        use bsv_wallet_toolbox::monitor::helpers::attempt_to_post_reqs_to_network;
        use bsv_wallet_toolbox::storage::find_args::{FindProvenTxReqsArgs, ProvenTxReqPartial};

        // Same funded fixtures as funded_wallet(), but with a capturing
        // post_beef so the rebuilt broadcast bytes can be asserted.
        let file: FundedFile =
            serde_json::from_str(CREATE_CORPUS).expect("funded corpus JSON must parse");
        let vector = file
            .vectors
            .iter()
            .find(|v| !v.input.funding_set.is_empty())
            .expect("a funded vector");
        let services = Arc::new(FixtureServices {
            roots: file.merkle_roots.clone(),
            post: PostMode::Capture(std::sync::Mutex::new(Vec::new())),
        });
        let setup = build_vector_wallet(&vector.input.root_key, services.clone())
            .await
            .expect("wallet build");
        for p in &file
            .funding_sets
            .get(&vector.input.funding_set)
            .expect("funding set present")
            .payments
        {
            internalize_funding(&setup, p).await.expect("internalize");
        }

        // A delayed mint leaves a ProvenTxReq for the broadcast queue.
        let mint_args: CreateActionArgs = serde_json::from_value(serde_json::json!({
            "description": "mint for monitor rebuild",
            "outputs": [{
                "lockingScript": "5187",
                "satoshis": 700,
                "outputDescription": "monitor token",
                "basket": "regression 352"
            }]
        }))
        .expect("mint args");
        let mint = setup
            .wallet
            .create_action(mint_args, None)
            .await
            .expect("mint createAction");
        let mint_txid = mint.txid.expect("mint txid");

        let reqs = setup
            .storage
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    txid: Some(mint_txid.clone()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .expect("find reqs");
        assert_eq!(reqs.len(), 1, "delayed mint leaves one req");

        attempt_to_post_reqs_to_network(&setup.storage, services.as_ref(), &reqs)
            .await
            .expect("rebuild + post");

        let captured = match &services.post {
            PostMode::Capture(store) => store.lock().unwrap().clone(),
            _ => unreachable!(),
        };
        assert_eq!(captured.len(), 1, "exactly one broadcast");
        let bytes = &captured[0];

        let mut cursor = std::io::Cursor::new(bytes);
        let beef = Beef::from_binary(&mut cursor).expect("posted beef parses");
        assert!(
            beef.atomic_txid.is_none(),
            "broadcast BEEF must be plain, not Atomic-framed"
        );
        assert!(beef.find_txid(&mint_txid).is_some(), "subject present");
        assert_closure(&beef, "monitor broadcast BEEF");
        for (i, btx) in beef.txs.iter().enumerate() {
            for input_txid in &btx.input_txids {
                if let Some(pos) = beef.txs.iter().position(|t| &t.txid == input_txid) {
                    assert!(
                        pos < i,
                        "ancestor {input_txid} serialized after dependent {}",
                        btx.txid
                    );
                }
            }
        }
    }

    /// listOutputs(include=EntireTransactions) assembly (rust-mpc#352 P2-c):
    /// per-txid BEEFs merge through the SDK's merge_beef — bumps deduped by
    /// (height, root), no duplicate txids — and the wire order is
    /// dependency-sorted. Two minted tokens share their whole funding
    /// ancestry, so the hand-rolled bump-offset concat would have emitted
    /// duplicate bumps and relied on linear dedup.
    #[tokio::test]
    async fn list_outputs_entire_transactions_is_spec_clean() {
        use bsv::wallet::interfaces::{ListOutputsArgs, OutputInclude};
        use std::collections::HashSet as StdHashSet;

        let setup = funded_wallet().await;

        for i in 0..2u32 {
            let args: CreateActionArgs = serde_json::from_value(serde_json::json!({
                "description": format!("mint token {i}"),
                "outputs": [{
                    "lockingScript": "5187",
                    "satoshis": 800 + i,
                    "outputDescription": "listoutputs token",
                    "basket": "regression 352"
                }]
            }))
            .expect("mint args");
            let r = setup
                .wallet
                .create_action(args, None)
                .await
                .expect("mint createAction");
            // A delayed action rests at Unprocessed until the monitor's
            // broadcast queue picks it up, and listOutputs only lists outputs
            // of broadcast-eligible parents. The monitor is off in this
            // offline harness, so stand in for TaskSendWaiting's transition.
            let txid = r.txid.expect("mint txid");
            let rows = setup
                .storage
                .find_transactions(
                    &bsv_wallet_toolbox::storage::find_args::FindTransactionsArgs {
                        partial: bsv_wallet_toolbox::storage::find_args::TransactionPartial {
                            txid: Some(txid),
                            ..Default::default()
                        },
                        no_raw_tx: true,
                        ..Default::default()
                    },
                )
                .await
                .expect("find mint tx");
            setup
                .storage
                .update_transaction(
                    rows[0].transaction_id,
                    &bsv_wallet_toolbox::storage::find_args::TransactionPartial {
                        status: Some(bsv_wallet_toolbox::status::TransactionStatus::Sending),
                        ..Default::default()
                    },
                )
                .await
                .expect("flip mint status");
        }

        let list_args: ListOutputsArgs = serde_json::from_value(serde_json::json!({
            "basket": "regression 352",
            "include": "entire transactions"
        }))
        .expect("list args");
        assert!(matches!(
            list_args.include,
            Some(OutputInclude::EntireTransactions)
        ));
        let result = setup
            .wallet
            .list_outputs(list_args, None)
            .await
            .expect("listOutputs");
        assert_eq!(result.outputs.len(), 2, "both tokens listed");

        let beef_bytes = result.beef.expect("EntireTransactions returns beef");
        let mut cursor = std::io::Cursor::new(&beef_bytes);
        let beef = Beef::from_binary(&mut cursor).expect("beef parses");

        // No duplicate txids.
        let mut seen: StdHashSet<&str> = StdHashSet::new();
        for btx in &beef.txs {
            assert!(seen.insert(btx.txid.as_str()), "duplicate tx {}", btx.txid);
        }
        // No duplicate bumps: (height, root) pairs are unique.
        let mut roots: StdHashSet<(u32, String)> = StdHashSet::new();
        for bump in &beef.bumps {
            let root = bump.compute_root(None).expect("root");
            assert!(
                roots.insert((bump.block_height, root)),
                "duplicate bump for height {}",
                bump.block_height
            );
        }
        // Both token txs present, closure holds, and ancestors precede
        // children on the wire.
        for output in &result.outputs {
            let txid = output.outpoint.split('.').next().unwrap();
            assert!(beef.find_txid(txid).is_some(), "token tx {txid} in beef");
        }
        assert_closure(&beef, "listOutputs BEEF");
        for (i, btx) in beef.txs.iter().enumerate() {
            for input_txid in &btx.input_txids {
                if let Some(pos) = beef.txs.iter().position(|t| &t.txid == input_txid) {
                    assert!(
                        pos < i,
                        "ancestor {input_txid} serialized after dependent {}",
                        btx.txid
                    );
                }
            }
        }
    }
}
