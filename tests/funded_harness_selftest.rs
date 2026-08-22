//! Self-test of the funded-vector harness, no network and no real funding.
//!
//! Proves, before a single satoshi moves, that the record/replay machinery is
//! deterministic: two runs of the same vector (same seed, same funding
//! fixture) produce byte-identical transactions, and a different seed
//! produces different change derivations. The funding BEEF here is fabricated
//! (a synthetic proven parent and a synthetic BRC-29 payment) and exists only
//! inside this test — recorded conformance fixtures never use it. What is
//! being tested is the harness, not the corpus.

mod funded_common;

use std::collections::BTreeMap;
use std::sync::Arc;

use bsv::primitives::private_key::PrivateKey;
use bsv::script::locking_script::LockingScript;
use bsv::script::unlocking_script::UnlockingScript;
use bsv::transaction::beef::{Beef, BEEF_V2};
use bsv::transaction::merkle_path::{MerklePath, MerklePathLeaf};
use bsv::transaction::transaction::Transaction;
use bsv::transaction::transaction_input::TransactionInput;
use bsv::transaction::transaction_output::TransactionOutput;

use bsv::wallet::interfaces::WalletInterface;
use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;

use funded_common::{
    caller_unlock_script, p2pkh_lock, run_create_vector, run_sign_vector, to_hex, FixtureServices,
    FundingPayment, PostMode,
};

const ROOT_1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SENDER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000007";
const CALLER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000005";
const FAKE_HEIGHT: u32 = 880_000;

/// Build a synthetic funding fixture: proven parent P (single-tx block, so
/// the merkle root is its txid) and payment F carrying a real BRC-29 lock to
/// wallet(root 1).
fn fabricate_funding() -> (FundingPayment, BTreeMap<u32, String>) {
    let sender = PrivateKey::from_hex(SENDER_KEY).expect("sender key");
    let receiver = PrivateKey::from_hex(ROOT_1).expect("root1").to_public_key();

    // Parent transaction: pretend-coinbase paying an arbitrary script.
    let mut parent = Transaction::new();
    parent.add_input(TransactionInput {
        source_transaction: None,
        source_txid: Some("00".repeat(32)),
        source_output_index: 0xffff_ffff,
        unlocking_script: Some(UnlockingScript::from_binary(&[0x51])),
        sequence: 0xffff_ffff,
    });
    parent.add_output(TransactionOutput {
        satoshis: Some(10_000),
        locking_script: LockingScript::from_binary(&[0x51]),
        change: false,
    });
    let parent_txid = parent.id().expect("parent txid");

    // Payment transaction: spends the parent, pays 5000 sats to a real
    // BRC-29 lock (sender key 7 -> root 1). The unlocking script is garbage;
    // SPV verification checks chain shape, not scripts, exactly like a real
    // BEEF whose ancestors we did not author.
    let prefix = "c2VsZnRlc3QtcHJlZml4".to_string();
    let suffix = "c2VsZnRlc3Qtc3VmZml4".to_string();
    let lock = ScriptTemplateBRC29::new(prefix.clone(), suffix.clone())
        .lock(&sender, &receiver)
        .expect("brc29 lock");

    let mut payment = Transaction::new();
    payment.add_input(TransactionInput {
        source_transaction: None,
        source_txid: Some(parent_txid.clone()),
        source_output_index: 0,
        unlocking_script: Some(UnlockingScript::from_binary(&[0x51])),
        sequence: 0xffff_ffff,
    });
    payment.add_output(TransactionOutput {
        satoshis: Some(5_000),
        locking_script: LockingScript::from_binary(&lock),
        change: false,
    });
    // Output 1: P2PKH to the caller key, for signAction caller-input tests.
    let caller = PrivateKey::from_hex(CALLER_KEY).expect("caller key");
    payment.add_output(TransactionOutput {
        satoshis: Some(300),
        locking_script: LockingScript::from_binary(&p2pkh_lock(&caller)),
        change: false,
    });
    let payment_txid = payment.id().expect("payment txid");

    let bump = MerklePath::new(
        FAKE_HEIGHT,
        vec![vec![MerklePathLeaf {
            offset: 0,
            hash: Some(parent_txid.clone()),
            txid: true,
            duplicate: false,
        }]],
    )
    .expect("bump");
    let root = bump.compute_root(Some(&parent_txid)).expect("root");

    let mut beef = Beef::new(BEEF_V2);
    let bump_index = beef.merge_bump(&bump).expect("merge bump");
    let mut parent_bytes = Vec::new();
    parent.to_binary(&mut parent_bytes).expect("parent bytes");
    beef.merge_raw_tx(&parent_bytes, Some(bump_index))
        .expect("merge parent");
    let mut payment_bytes = Vec::new();
    payment
        .to_binary(&mut payment_bytes)
        .expect("payment bytes");
    beef.merge_raw_tx(&payment_bytes, None)
        .expect("merge payment");

    let atomic = beef.to_binary_atomic(&payment_txid).expect("atomic");

    let sender_identity = sender.to_public_key().to_der_hex();
    (
        FundingPayment {
            beef: to_hex(&atomic),
            output_index: 0,
            derivation_prefix: prefix,
            derivation_suffix: suffix,
            sender_identity_key: sender_identity,
            satoshis: 5_000,
            txid: payment_txid,
            description: "harness self-test synthetic funding".to_string(),
        },
        BTreeMap::from([(FAKE_HEIGHT, root)]),
    )
}

fn services(roots: &BTreeMap<u32, String>) -> Arc<FixtureServices> {
    Arc::new(FixtureServices {
        roots: roots.clone(),
        post: PostMode::Tripwire,
    })
}

fn nosend_args() -> serde_json::Value {
    serde_json::json!({
        "description": "harness self-test action",
        "outputs": [{
            "lockingScript": "76a914a3dbcdd15d94b7fec6f80879369cf57ffda0eeca88ac",
            "satoshis": 1000,
            "outputDescription": "out0",
            "basket": "corpus",
            "tags": ["selftest"]
        }],
        "labels": ["selftest"],
        "options": {"noSend": true, "acceptDelayedBroadcast": true}
    })
}

#[tokio::test]
async fn same_seed_reproduces_byte_identical_transactions() {
    let (payment, roots) = fabricate_funding();
    let args = nosend_args();

    let a = run_create_vector(
        services(&roots),
        ROOT_1,
        "seed-a",
        std::slice::from_ref(&payment),
        &args,
    )
    .await
    .expect("run a");
    assert_eq!(a.outcome.status, "success", "{:?}", a.outcome.message);
    assert!(a.outcome.txid.is_some());
    assert!(a.outcome.tx.is_some());
    assert!(
        !a.outcome.no_send_change.is_empty(),
        "noSend must report change"
    );
    assert!(!a.outcome.change.is_empty());

    let b = run_create_vector(
        services(&roots),
        ROOT_1,
        "seed-a",
        std::slice::from_ref(&payment),
        &args,
    )
    .await
    .expect("run b");
    assert_eq!(
        a.outcome, b.outcome,
        "same seed must reproduce byte-identically"
    );

    let c = run_create_vector(services(&roots), ROOT_1, "seed-c", &[payment], &args)
        .await
        .expect("run c");
    assert_ne!(
        a.outcome.txid, c.outcome.txid,
        "different seed must derive different change, hence a different txid"
    );
}

#[tokio::test]
async fn unpinned_root_fails_internalize() {
    let (payment, _) = fabricate_funding();
    let empty = BTreeMap::new();
    let err = run_create_vector(services(&empty), ROOT_1, "seed", &[payment], &nosend_args())
        .await
        .expect_err("internalize must fail without the pinned root");
    assert!(err.contains("internalize"), "unexpected error: {err}");
}

#[tokio::test]
async fn sign_vector_round_trip_is_deterministic() {
    let (payment, roots) = fabricate_funding();
    let caller = PrivateKey::from_hex(CALLER_KEY).expect("caller key");

    // Precursor: signable createAction spending the caller P2PKH outpoint
    // (vin 0) plus wallet change, one small output, noSend.
    let precursor = serde_json::json!({
        "description": "selftest signAction precursor",
        "inputBEEF": funded_common::from_hex(&payment.beef),
        "inputs": [{
            "outpoint": format!("{}.1", payment.txid),
            "inputDescription": "selftest caller input",
            "unlockingScriptLength": 108
        }],
        "outputs": [{
            "lockingScript": to_hex(&p2pkh_lock(&caller)),
            "satoshis": 100,
            "outputDescription": "selftest output"
        }],
        "labels": ["selftest"],
        "options": {"signAndProcess": false, "noSend": true}
    });

    // Discovery pass: learn the reference and signable tx, then compute the
    // real unlocking script — exactly what the recorder does.
    let (reference, signable_tx, signable_ref_bytes) = {
        let _entropy_guard = funded_common::ENTROPY_SESSION.lock().await;
        let setup = funded_common::build_vector_wallet(ROOT_1, services(&roots))
            .await
            .expect("wallet");
        funded_common::internalize_funding(&setup, &payment)
            .await
            .expect("funding");
        bsv_wallet_toolbox::utility::conformance_entropy::set_conformance_entropy("sign-selftest");
        let args: bsv::wallet::interfaces::CreateActionArgs =
            serde_json::from_value(precursor.clone()).expect("precursor args");
        let created = setup
            .wallet
            .create_action(args, None)
            .await
            .expect("precursor create");
        bsv_wallet_toolbox::utility::conformance_entropy::clear_conformance_entropy();
        let signable = created.signable_transaction.expect("signable");
        // Raw reference bytes → the base64 text storage keys the row by.
        let reference = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&signable.reference)
        };
        (reference, signable.tx, signable.reference)
    };
    let unlock = caller_unlock_script(&signable_tx, 0, 300, &caller).await;

    // SignActionArgs.reference serializes as base64 on the wire.
    use base64::Engine as _;
    let sign_args = serde_json::json!({
        "reference": base64::engine::general_purpose::STANDARD.encode(&signable_ref_bytes),
        "spends": {"0": {"unlockingScript": to_hex(&unlock)}}
    });

    // Canonical pass twice — byte-identical outcomes required.
    let a = run_sign_vector(
        services(&roots),
        ROOT_1,
        "sign-selftest",
        std::slice::from_ref(&payment),
        std::slice::from_ref(&precursor),
        &sign_args,
    )
    .await
    .expect("run a");
    assert_eq!(a.outcome.status, "success", "{:?}", a.outcome.message);
    assert_eq!(
        a.setup_outcomes[0].signable_reference.as_deref(),
        Some(reference.as_str()),
        "discovery and canonical passes must agree on the reference"
    );
    assert!(a.outcome.txid.is_some());
    assert!(a.outcome.tx.is_some());

    let b = run_sign_vector(
        services(&roots),
        ROOT_1,
        "sign-selftest",
        &[payment],
        &[precursor],
        &sign_args,
    )
    .await
    .expect("run b");
    assert_eq!(
        a.outcome, b.outcome,
        "same seed must reproduce byte-identically"
    );
}

#[tokio::test]
async fn delayed_send_vector_must_not_broadcast() {
    let (payment, roots) = fabricate_funding();
    let args = serde_json::json!({
        "description": "delayed send action",
        "outputs": [{
            "lockingScript": "76a914a3dbcdd15d94b7fec6f80879369cf57ffda0eeca88ac",
            "satoshis": 1000,
            "outputDescription": "out0"
        }],
        "labels": ["selftest"],
        "options": {"noSend": false, "acceptDelayedBroadcast": true}
    });
    let a = run_create_vector(
        services(&roots),
        ROOT_1,
        "seed-delayed",
        std::slice::from_ref(&payment),
        &args,
    )
    .await
    .expect("run");
    assert_eq!(a.outcome.status, "success", "{:?}", a.outcome.message);
    assert!(a.outcome.no_send_change.is_empty());
}

/// Pins the signAction isDelayed mapping: an explicit
/// options.acceptDelayedBroadcast=true is a DELAYED send — the tripwire
/// services prove no inline broadcast happens. Before the fix this mapping
/// was negated and this test panicked on WalletServices::post_beef.
#[tokio::test]
async fn sign_accept_delayed_true_does_not_broadcast_inline() {
    let (payment, roots) = fabricate_funding();
    let caller = PrivateKey::from_hex(CALLER_KEY).expect("caller key");
    let precursor = serde_json::json!({
        "description": "selftest signAction delayed precursor",
        "inputBEEF": funded_common::from_hex(&payment.beef),
        "inputs": [{
            "outpoint": format!("{}.1", payment.txid),
            "inputDescription": "selftest caller input",
            "unlockingScriptLength": 108
        }],
        "outputs": [{
            "lockingScript": to_hex(&p2pkh_lock(&caller)),
            "satoshis": 100,
            "outputDescription": "selftest output"
        }],
        "labels": ["selftest"],
        "options": {"signAndProcess": false}
    });

    let (signable_tx, signable_ref_bytes) = {
        let _entropy_guard = funded_common::ENTROPY_SESSION.lock().await;
        let setup = funded_common::build_vector_wallet(ROOT_1, services(&roots))
            .await
            .expect("wallet");
        funded_common::internalize_funding(&setup, &payment)
            .await
            .expect("funding");
        bsv_wallet_toolbox::utility::conformance_entropy::set_conformance_entropy("delayed-sign");
        let args: bsv::wallet::interfaces::CreateActionArgs =
            serde_json::from_value(precursor.clone()).expect("precursor args");
        let created = setup
            .wallet
            .create_action(args, None)
            .await
            .expect("precursor create");
        bsv_wallet_toolbox::utility::conformance_entropy::clear_conformance_entropy();
        let signable = created.signable_transaction.expect("signable");
        (signable.tx, signable.reference)
    };
    let unlock = caller_unlock_script(&signable_tx, 0, 300, &caller).await;

    use base64::Engine as _;
    let sign_args = serde_json::json!({
        "reference": base64::engine::general_purpose::STANDARD.encode(&signable_ref_bytes),
        "spends": {"0": {"unlockingScript": to_hex(&unlock)}},
        "options": {"acceptDelayedBroadcast": true}
    });

    let r = run_sign_vector(
        services(&roots),
        ROOT_1,
        "delayed-sign",
        std::slice::from_ref(&payment),
        std::slice::from_ref(&precursor),
        &sign_args,
    )
    .await
    .expect("run");
    assert_eq!(r.outcome.status, "success", "{:?}", r.outcome.message);
}

/// Pins BRC-100 noSend chaining: a second noSend action funded solely by the
/// first one's change, admitted via options.noSendChange, in a wallet whose
/// only basket UTXO is already consumed. Before the fix the storage layer
/// ignored options.noSendChange entirely and this failed with
/// WERR_INSUFFICIENT_FUNDS.
#[tokio::test]
async fn nosend_chaining_via_no_send_change() {
    let (payment, roots) = fabricate_funding();
    let _entropy_guard = funded_common::ENTROPY_SESSION.lock().await;
    let setup = funded_common::build_vector_wallet(ROOT_1, services(&roots))
        .await
        .expect("wallet");
    funded_common::internalize_funding(&setup, &payment)
        .await
        .expect("funding");
    bsv_wallet_toolbox::utility::conformance_entropy::set_conformance_entropy("nosend-chain");

    let out = serde_json::json!([{
        "lockingScript": "76a914a3dbcdd15d94b7fec6f80879369cf57ffda0eeca88ac",
        "satoshis": 100,
        "outputDescription": "chain out"
    }]);
    let ns1_args: bsv::wallet::interfaces::CreateActionArgs =
        serde_json::from_value(serde_json::json!({
            "description": "chain link 1",
            "outputs": out,
            "options": {"noSend": true}
        }))
        .expect("args");
    let ns1 = setup
        .wallet
        .create_action(ns1_args, None)
        .await
        .expect("ns1");
    assert!(!ns1.no_send_change.is_empty());

    let ns2_args: bsv::wallet::interfaces::CreateActionArgs =
        serde_json::from_value(serde_json::json!({
            "description": "chain link 2",
            "outputs": out,
            "options": {"noSend": true, "noSendChange": ns1.no_send_change}
        }))
        .expect("args");
    let ns2 = setup
        .wallet
        .create_action(ns2_args, None)
        .await
        .expect("ns2 must fund from ns1's chained change");
    assert!(ns2.txid.is_some());
    bsv_wallet_toolbox::utility::conformance_entropy::clear_conformance_entropy();
}
