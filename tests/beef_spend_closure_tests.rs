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
}
