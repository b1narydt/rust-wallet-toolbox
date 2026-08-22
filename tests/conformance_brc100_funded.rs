//! Offline replay of the funded BRC-100 createAction/signAction vectors.
//!
//! Consumes `conformance/vectors/wallet/brc100/createaction-funded.json` and
//! `signaction-funded.json` — vectors recorded by `record_funded_vectors.rs`
//! against a real faucet-funded wallet on mainnet. Each vector carries its
//! funding as real AtomicBEEF fixtures and the merkle roots those BEEFs prove
//! against, so this test rebuilds the recorded wallet state from nothing and
//! must reproduce the recorded txid and transaction bytes exactly.
//!
//! Fully hermetic: the corpus is embedded at compile time, and the services
//! handed to the wallet are a [`FixtureServices`] whose chain tracker answers
//! only from the pinned merkle roots and whose every network method panics
//! (the tripwire). A replay that reaches for the network fails loudly.
//!
//! The assertion is byte-for-byte equality of the entire recorded `expected`
//! block — txid, AtomicBEEF bytes, raw tx bytes, change derivations, error
//! messages. Never weaken it to "txid only" or "parses": a fixture that
//! cannot reproduce its recorded bytes is incomplete and must be re-recorded.

mod funded_common;

use std::collections::BTreeMap;
use std::sync::Arc;

use funded_common::{
    run_create_vector, run_sign_vector, FixtureServices, FundedFile, PostMode, RecordedOutcome,
};

const CREATE_CORPUS: &str =
    include_str!("../conformance/vectors/wallet/brc100/createaction-funded.json");
const SIGN_CORPUS: &str =
    include_str!("../conformance/vectors/wallet/brc100/signaction-funded.json");

fn load(corpus: &str, expect_id: &str) -> FundedFile {
    let f: FundedFile = serde_json::from_str(corpus).expect("funded corpus JSON must parse");
    assert_eq!(f.id, expect_id);
    f
}

/// Compare a replayed outcome against the recorded `expected` block,
/// field-by-field so a mismatch names exactly what diverged.
fn assert_outcome(vector_id: &str, recorded: &serde_json::Value, replayed: &RecordedOutcome) {
    let recorded: RecordedOutcome = serde_json::from_value(recorded.clone())
        .unwrap_or_else(|e| panic!("{vector_id}: recorded expected block failed to parse: {e}"));

    assert_eq!(
        recorded.status, replayed.status,
        "{vector_id}: status diverged (replayed error: {:?})",
        replayed.message
    );
    assert_eq!(recorded.txid, replayed.txid, "{vector_id}: txid diverged");
    assert_eq!(
        recorded.raw_tx, replayed.raw_tx,
        "{vector_id}: raw tx bytes diverged"
    );
    assert_eq!(
        recorded.tx, replayed.tx,
        "{vector_id}: AtomicBEEF bytes diverged"
    );
    assert_eq!(
        recorded.no_send_change, replayed.no_send_change,
        "{vector_id}: noSendChange diverged"
    );
    assert_eq!(
        recorded.send_with_results, replayed.send_with_results,
        "{vector_id}: sendWithResults diverged"
    );
    assert_eq!(
        recorded.signable_reference, replayed.signable_reference,
        "{vector_id}: signable reference diverged"
    );
    assert_eq!(
        recorded.change, replayed.change,
        "{vector_id}: change outputs diverged"
    );
    assert_eq!(
        recorded.message, replayed.message,
        "{vector_id}: error message diverged"
    );
}

fn fixture_services(
    file: &FundedFile,
    vector: &funded_common::FundedVector,
) -> Arc<FixtureServices> {
    // A vector whose recorded run included an inline broadcast carries the
    // network's recorded response; everything else gets the tripwire.
    let post = match vector.expected.get("postBeefResponse") {
        Some(v) if !v.is_null() => PostMode::Recorded(
            serde_json::from_value(v.clone())
                .unwrap_or_else(|e| panic!("{}: bad postBeefResponse: {e}", vector.id)),
        ),
        _ => PostMode::Tripwire,
    };
    Arc::new(FixtureServices {
        roots: file.merkle_roots.clone(),
        post,
    })
}

fn funding_for<'a>(
    file: &'a FundedFile,
    vector: &funded_common::FundedVector,
) -> &'a [funded_common::FundingPayment] {
    match file.funding_sets.get(&vector.input.funding_set) {
        Some(set) => &set.payments,
        None if vector.input.funding_set.is_empty() => &[],
        None => panic!(
            "{}: funding set {:?} not present in file",
            vector.id, vector.input.funding_set
        ),
    }
}

#[tokio::test]
async fn replay_createaction_funded() {
    let file = load(CREATE_CORPUS, "wallet.brc100.createaction-funded");
    let mut replayed_count = 0usize;
    for vector in &file.vectors {
        let services = fixture_services(&file, vector);
        let run = run_create_vector(
            services,
            &vector.input.root_key,
            &vector.input.entropy_seed,
            funding_for(&file, vector),
            &vector.input.args,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: replay run failed: {e}", vector.id));
        assert_outcome(&vector.id, &vector.expected, &run.outcome);
        replayed_count += 1;
    }
    // Corpus shape: 90 upstream vectors recorded, plus the pinned
    // upstream-defect vectors (verbatim reserved-basket rejection and
    // top-level-flags demonstrations). The recorder writes them all; a
    // shrink here means vectors were lost.
    assert!(
        replayed_count >= 90,
        "expected at least 90 recorded createAction vectors, replayed {replayed_count}"
    );
}

#[tokio::test]
async fn replay_signaction_funded() {
    let file = load(SIGN_CORPUS, "wallet.brc100.signaction-funded");
    let mut replayed_count = 0usize;
    for vector in &file.vectors {
        let services = fixture_services(&file, vector);
        let setup_args: Vec<serde_json::Value> = vector
            .input
            .setup
            .iter()
            .map(|s| s.create_args.clone())
            .collect();
        let run = run_sign_vector(
            services,
            &vector.input.root_key,
            &vector.input.entropy_seed,
            funding_for(&file, vector),
            &setup_args,
            &vector.input.args,
        )
        .await
        .unwrap_or_else(|e| panic!("{}: replay run failed: {e}", vector.id));

        // Setup steps must land where the recording pinned them: the
        // signAction reference in the recorded args is only meaningful if
        // the precursor reproduced identically.
        for (i, (step, outcome)) in vector
            .input
            .setup
            .iter()
            .zip(run.setup_outcomes.iter())
            .enumerate()
        {
            if let Some(ref want_ref) = step.reference {
                assert_eq!(
                    Some(want_ref),
                    outcome.signable_reference.as_ref(),
                    "{}: setup[{i}] reference diverged",
                    vector.id
                );
            }
            if let Some(ref want_txid) = step.txid {
                assert_eq!(
                    Some(want_txid),
                    outcome.txid.as_ref(),
                    "{}: setup[{i}] txid diverged",
                    vector.id
                );
            }
        }

        assert_outcome(&vector.id, &vector.expected, &run.outcome);
        replayed_count += 1;
    }
    assert_eq!(
        replayed_count, 8,
        "expected all 8 signAction vectors recorded"
    );
}

/// The pinned merkle roots must be internally consistent with every funding
/// BEEF in both files: each bump's computed root equals the pin. This is what
/// lets the replay validate real SPV data with no network.
#[tokio::test]
async fn funding_fixture_bumps_match_pinned_roots() {
    for (corpus, id) in [
        (CREATE_CORPUS, "wallet.brc100.createaction-funded"),
        (SIGN_CORPUS, "wallet.brc100.signaction-funded"),
    ] {
        let file = load(corpus, id);
        let mut seen: BTreeMap<u32, String> = BTreeMap::new();
        for set in file.funding_sets.values() {
            for p in &set.payments {
                let roots = funded_common::bump_roots(&funded_common::from_hex(&p.beef))
                    .unwrap_or_else(|e| panic!("{id}: {e}"));
                for (h, r) in roots {
                    assert_eq!(
                        file.merkle_roots.get(&h),
                        Some(&r),
                        "{id}: bump root at height {h} not pinned or mismatched"
                    );
                    seen.insert(h, r);
                }
            }
        }
        assert!(
            !seen.is_empty(),
            "{id}: no bumps found in any funding fixture — BEEFs cannot chain to a proven ancestor"
        );
    }
}
