//! Regenerate the funded corpus' `expected.tx` (AtomicBEEF hex) fields —
//! and ONLY those — after a deliberate change to BEEF composition/encoding.
//!
//! Recorded funded vectors pin the entire outcome byte-for-byte. When BEEF
//! assembly legitimately changes (e.g. rust-mpc#352: ancestors for
//! caller-named inputs now included; atomic serialization dependency-sorted),
//! the recorded `expected.tx` bytes go stale while everything the BEEF
//! carries proof FOR is unchanged. Regenerating a recorded expectation is
//! allowed ONLY for byte-fields that are the BEEF itself — never for txids,
//! raw transactions, or signatures. This harness enforces that rule:
//!
//! For every vector it replays offline and asserts
//!   * status, txid, rawTx, noSendChange, sendWithResults, signable
//!     reference, change derivations, and error message all EQUAL the
//!     recorded values (a divergence in any of these aborts — that is a real
//!     regression, not a BEEF encoding change), and
//!   * every full transaction in the RECORDED BEEF appears in the replayed
//!     BEEF with byte-identical raw transaction data (the new BEEF is a
//!     superset — it may add ancestors the recording was missing, it may
//!     not lose or alter anything),
//!
//! and only then rewrites `expected.tx` with the replayed bytes, appending a
//! dated note to the vector. `#[ignore]`d: run explicitly via
//! `cargo test --features sqlite --test regen_funded_expected_beef -- --ignored`.

mod funded_common;

#[cfg(feature = "sqlite")]
mod regen {
    use std::sync::Arc;

    use bsv::transaction::beef::Beef;

    use crate::funded_common::{
        from_hex, run_create_vector, run_sign_vector, FixtureServices, FundedFile, PostMode,
        RecordedOutcome,
    };

    const NOTE: &str = "2026-08-21: expected.tx regenerated — BEEF-only byte change \
        (bsv-sdk 0.5.2 / bsv-rust-sdk#44: Atomic serialization delegated to the SDK's \
        TS-parity toBinaryAtomic, whose sortTxs order differs from the toolbox's former \
        closure walk; same transaction set, same bumps, TS-canonical order — every \
        vector cross-checked byte-equal against @bsv/sdk 2.3.1 toBinaryAtomic of the \
        previously recorded bytes); status/txid/rawTx/change/signatures verified \
        unchanged and the new BEEF verified a superset of the recorded one before rewriting.";

    fn parse_beef(hex: &str, context: &str) -> Beef {
        let bytes = from_hex(hex);
        let mut cursor = std::io::Cursor::new(&bytes);
        Beef::from_binary(&mut cursor).unwrap_or_else(|e| panic!("{context}: BEEF parse: {e}"))
    }

    /// Every full transaction in `recorded` must appear byte-identically in
    /// `replayed`. `replayed` may carry MORE (the ancestors the recording
    /// was missing) — never less, never different bytes.
    fn assert_superset(vector_id: &str, recorded_hex: &str, replayed_hex: &str) {
        let recorded = parse_beef(recorded_hex, vector_id);
        let replayed = parse_beef(replayed_hex, vector_id);
        for rec_btx in &recorded.txs {
            let Some(rec_tx) = rec_btx.tx.as_ref() else {
                continue; // txid-only entries carry no bytes to preserve
            };
            let rep_btx = replayed.find_txid(&rec_btx.txid).unwrap_or_else(|| {
                panic!(
                    "{vector_id}: replayed BEEF LOST recorded tx {} — not a BEEF-only change",
                    rec_btx.txid
                )
            });
            let rep_tx = rep_btx.tx.as_ref().unwrap_or_else(|| {
                panic!(
                    "{vector_id}: replayed BEEF downgraded {} to txid-only",
                    rec_btx.txid
                )
            });
            let (mut a, mut b) = (Vec::new(), Vec::new());
            rec_tx.to_binary(&mut a).expect("recorded tx serialize");
            rep_tx.to_binary(&mut b).expect("replayed tx serialize");
            assert_eq!(
                a, b,
                "{vector_id}: raw bytes of {} changed — not a BEEF-only change",
                rec_btx.txid
            );
        }
    }

    /// Everything except `tx` must be identical.
    fn assert_all_but_beef(
        vector_id: &str,
        recorded: &RecordedOutcome,
        replayed: &RecordedOutcome,
    ) {
        assert_eq!(recorded.status, replayed.status, "{vector_id}: status");
        assert_eq!(recorded.txid, replayed.txid, "{vector_id}: txid");
        assert_eq!(recorded.raw_tx, replayed.raw_tx, "{vector_id}: rawTx");
        assert_eq!(
            recorded.no_send_change, replayed.no_send_change,
            "{vector_id}: noSendChange"
        );
        assert_eq!(
            recorded.send_with_results, replayed.send_with_results,
            "{vector_id}: sendWithResults"
        );
        assert_eq!(
            recorded.signable_reference, replayed.signable_reference,
            "{vector_id}: signable reference"
        );
        assert_eq!(recorded.change, replayed.change, "{vector_id}: change");
        assert_eq!(recorded.message, replayed.message, "{vector_id}: message");
    }

    /// Apply one replayed outcome to a vector's expected block. Returns true
    /// if `expected.tx` was rewritten.
    fn apply(vector: &mut crate::funded_common::FundedVector, replayed: &RecordedOutcome) -> bool {
        let recorded: RecordedOutcome = serde_json::from_value(vector.expected.clone())
            .unwrap_or_else(|e| panic!("{}: expected block parse: {e}", vector.id));
        assert_all_but_beef(&vector.id, &recorded, replayed);
        match (&recorded.tx, &replayed.tx) {
            (Some(rec), Some(rep)) if rec != rep => {
                assert_superset(&vector.id, rec, rep);
                vector
                    .expected
                    .as_object_mut()
                    .expect("expected is an object")
                    .insert("tx".into(), serde_json::Value::String(rep.clone()));
                if !vector.notes.iter().any(|n| n == NOTE) {
                    vector.notes.push(NOTE.to_string());
                }
                true
            }
            (None, None) | (Some(_), Some(_)) => false,
            (rec, rep) => panic!(
                "{}: tx presence changed (recorded {:?}, replayed {:?}) — not a BEEF-only change",
                vector.id,
                rec.is_some(),
                rep.is_some()
            ),
        }
    }

    fn corpus_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("conformance/vectors/wallet/brc100")
            .join(name)
    }

    fn services_for(
        file: &FundedFile,
        vector: &crate::funded_common::FundedVector,
    ) -> Arc<FixtureServices> {
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
        vector: &crate::funded_common::FundedVector,
    ) -> Vec<crate::funded_common::FundingPayment> {
        match file.funding_sets.get(&vector.input.funding_set) {
            Some(set) => set.payments.clone(),
            None if vector.input.funding_set.is_empty() => vec![],
            None => panic!("{}: funding set missing", vector.id),
        }
    }

    #[tokio::test]
    #[ignore = "rewrites conformance vectors; run deliberately after a BEEF encoding change"]
    async fn regen_createaction_funded_beef() {
        let path = corpus_path("createaction-funded.json");
        let mut file: FundedFile =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read corpus"))
                .expect("corpus parse");
        let mut rewritten = 0usize;
        for i in 0..file.vectors.len() {
            let vector = file.vectors[i].clone();
            let services = services_for(&file, &vector);
            let funding = funding_for(&file, &vector);
            let run = run_create_vector(
                services,
                &vector.input.root_key,
                &vector.input.entropy_seed,
                &funding,
                &vector.input.args,
            )
            .await
            .unwrap_or_else(|e| panic!("{}: replay failed: {e}", vector.id));
            if apply(&mut file.vectors[i], &run.outcome) {
                rewritten += 1;
            }
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&file).expect("corpus serialize"),
        )
        .expect("corpus write");
        println!("createaction-funded: rewrote expected.tx on {rewritten} vectors");
    }

    #[tokio::test]
    #[ignore = "rewrites conformance vectors; run deliberately after a BEEF encoding change"]
    async fn regen_signaction_funded_beef() {
        let path = corpus_path("signaction-funded.json");
        let mut file: FundedFile =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read corpus"))
                .expect("corpus parse");
        let mut rewritten = 0usize;
        for i in 0..file.vectors.len() {
            let vector = file.vectors[i].clone();
            let services = services_for(&file, &vector);
            let funding = funding_for(&file, &vector);
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
                &funding,
                &setup_args,
                &vector.input.args,
            )
            .await
            .unwrap_or_else(|e| panic!("{}: replay failed: {e}", vector.id));
            // Setup pins (reference/txid) must still hold.
            for (j, (step, outcome)) in vector
                .input
                .setup
                .iter()
                .zip(run.setup_outcomes.iter())
                .enumerate()
            {
                if let Some(ref want) = step.reference {
                    assert_eq!(
                        Some(want),
                        outcome.signable_reference.as_ref(),
                        "{}: setup[{j}] reference",
                        vector.id
                    );
                }
                if let Some(ref want) = step.txid {
                    assert_eq!(
                        Some(want),
                        outcome.txid.as_ref(),
                        "{}: setup[{j}] txid",
                        vector.id
                    );
                }
            }
            if apply(&mut file.vectors[i], &run.outcome) {
                rewritten += 1;
            }
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&file).expect("corpus serialize"),
        )
        .expect("corpus write");
        println!("signaction-funded: rewrote expected.tx on {rewritten} vectors");
    }
}
