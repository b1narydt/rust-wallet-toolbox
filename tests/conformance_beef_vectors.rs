//! TS-golden BEEF vectors (conformance/vectors/beef/) against the toolbox's
//! Atomic BEEF serialization path — the P0 surface of rust-mpc#352.
//!
//! `beef_spend_closure.json` (the #352 regression shape) and
//! `beef_atomic_closure.json` pin byte-exact `toBinaryAtomic` output of the
//! normative TS `@bsv/sdk`. The toolbox path under test is
//! `serialize_beef_atomic` — exercised here via `build_beef_bytes`'s
//! serializer seam at the Beef level: parse `input_beef_hex`, serialize
//! atomically for `subject_txid`, compare with `expected_atomic_hex`.
//!
//! Byte comparison first; on divergence the test falls back to a STRUCTURAL
//! equivalence check (same subject, same closure tx set with identical raw
//! bytes, same set of retained (height, root) bumps) and fails only if that
//! diverges too — TS merge/serialize output is not byte-stable under TS
//! itself (see SURPRISES.json), and the Rust SDK's `to_binary_atomic` does
//! not yet prune unrelated txs/bumps the way TS does. Structural passes with
//! byte divergence are reported via println so the parity gap stays visible.

use bsv::transaction::beef::Beef;

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_beef(bytes: &[u8], context: &str) -> Beef {
    let mut cursor = std::io::Cursor::new(bytes);
    Beef::from_binary(&mut cursor).unwrap_or_else(|e| panic!("{context}: BEEF parse: {e}"))
}

#[derive(serde::Deserialize)]
struct VectorFile {
    id: String,
    vectors: Vec<ClosureVector>,
}

#[derive(serde::Deserialize)]
struct ClosureVector {
    id: String,
    name: String,
    input_beef_hex: String,
    subject_txid: String,
    expected_atomic_hex: String,
}

fn load(name: &str) -> VectorFile {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("conformance/vectors/beef")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("{name}: read failed ({e}) — vectors are committed alongside this test")
    }))
    .unwrap_or_else(|e| panic!("{name}: parse failed: {e}"))
}

/// Structural equivalence of two Atomic BEEFs: same subject, same tx set
/// with byte-identical raw transactions (txid-only entries compared by
/// txid), same set of (block_height, computed_root) bumps.
fn structurally_equal(expected: &Beef, actual: &Beef) -> Result<(), String> {
    if expected.atomic_txid != actual.atomic_txid {
        return Err(format!(
            "subject diverged: expected {:?}, got {:?}",
            expected.atomic_txid, actual.atomic_txid
        ));
    }

    let mut exp_txids: Vec<&str> = expected.txs.iter().map(|t| t.txid.as_str()).collect();
    let mut act_txids: Vec<&str> = actual.txs.iter().map(|t| t.txid.as_str()).collect();
    exp_txids.sort_unstable();
    act_txids.sort_unstable();
    if exp_txids != act_txids {
        return Err(format!(
            "tx set diverged: expected {exp_txids:?}, got {act_txids:?}"
        ));
    }
    for exp_btx in &expected.txs {
        let act_btx = actual.find_txid(&exp_btx.txid).expect("set-checked above");
        match (&exp_btx.tx, &act_btx.tx) {
            (Some(e), Some(a)) => {
                let (mut eb, mut ab) = (Vec::new(), Vec::new());
                e.to_binary(&mut eb).map_err(|e| e.to_string())?;
                a.to_binary(&mut ab).map_err(|e| e.to_string())?;
                if eb != ab {
                    return Err(format!("raw bytes of {} diverged", exp_btx.txid));
                }
            }
            (None, None) => {}
            _ => {
                return Err(format!(
                    "txid-only status of {} diverged (expected full: {}, got full: {})",
                    exp_btx.txid,
                    exp_btx.tx.is_some(),
                    act_btx.tx.is_some()
                ))
            }
        }
        // A tx the normative output proves via a bump must be proven here too.
        if exp_btx.bump_index.is_some() != act_btx.bump_index.is_some() {
            return Err(format!(
                "bump-proven status of {} diverged (expected proven: {}, got proven: {})",
                exp_btx.txid,
                exp_btx.bump_index.is_some(),
                act_btx.bump_index.is_some()
            ));
        }
    }

    let roots = |b: &Beef| -> Result<Vec<(u32, String)>, String> {
        let mut v = b
            .bumps
            .iter()
            .map(|bump| {
                bump.compute_root(None)
                    .map(|r| (bump.block_height, r))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        v.sort();
        v.dedup();
        Ok(v)
    };
    let (er, ar) = (roots(expected)?, roots(actual)?);
    if er != ar {
        return Err(format!("bump set diverged: expected {er:?}, got {ar:?}"));
    }
    Ok(())
}

fn run_closure_file(name: &str, expect_id: &str) {
    let file = load(name);
    assert_eq!(file.id, expect_id);
    let mut byte_exact = 0usize;
    let mut structural_only: Vec<String> = Vec::new();
    for v in &file.vectors {
        let beef = parse_beef(&from_hex(&v.input_beef_hex), &v.id);
        let atomic = bsv_wallet_toolbox::signer::methods::create_action::serialize_beef_atomic(
            &beef,
            &v.subject_txid,
        )
        .unwrap_or_else(|e| panic!("{} ({}): serialization failed: {e}", v.id, v.name));

        if to_hex(&atomic) == v.expected_atomic_hex {
            byte_exact += 1;
            continue;
        }

        let expected = parse_beef(&from_hex(&v.expected_atomic_hex), &v.id);
        let actual = parse_beef(&atomic, &v.id);
        structurally_equal(&expected, &actual).unwrap_or_else(|why| {
            panic!(
                "{} ({}): diverges from normative TS output structurally: {why}\n  \
                 expected: {}\n  actual:   {}",
                v.id,
                v.name,
                v.expected_atomic_hex,
                to_hex(&atomic)
            )
        });
        structural_only.push(format!("{} ({})", v.id, v.name));
    }
    println!(
        "{name}: {} byte-exact, {} structural-only {:?}",
        byte_exact,
        structural_only.len(),
        structural_only
    );
}

/// The rust-mpc#352 regression shape: a caller-named input's parent must
/// ride in the child's Atomic BEEF.
#[test]
fn spend_closure_vectors() {
    run_closure_file("beef_spend_closure.json", "transaction.beef.spend_closure");
}

/// toBinaryAtomic dependency-closure semantics.
#[test]
fn atomic_closure_vectors() {
    run_closure_file(
        "beef_atomic_closure.json",
        "transaction.beef.atomic_closure",
    );
}
