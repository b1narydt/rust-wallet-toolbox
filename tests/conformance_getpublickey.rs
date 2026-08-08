//! BRC-100 `getPublicKey` conformance runner — the BRC-42/43 derivation surface.
//!
//! Consumes the vendored corpus embedded at compile time from
//! `conformance/vectors/wallet/brc100/getpublickey.json` (pinned via
//! `conformance/SOURCE` to bsv-blockchain/ts-stack @ 1920a9c1; reference impl
//! `@bsv/sdk@2.0.14 + wallet-toolbox`).
//!
//! Each vector supplies `input.root_key` (hex private key) and BRC-100
//! `getPublicKey` args; `expected` is either the derived compressed public key
//! (DER hex) or `{error: true, message}`.
//!
//! Surface under test: the exact chain `Wallet::get_public_key` delegates to
//! for the default (non-privileged, no signing provider) configuration —
//! `bsv::wallet::validation::validate_get_public_key_args` followed by
//! `ProtoWallet::get_public_key_sync` over a `KeyDeriver` rooted at
//! `root_key` (`src/wallet/wallet.rs`). A wrong answer here makes customer
//! funds permanently unspendable, so every vector is asserted — none skipped.
//!
//! The `privileged` flag: the TS reference dispatcher derives these vectors
//! through `ProtoWallet`, which ignores `privileged` entirely, and the 75
//! privileged vectors expect the same key as their non-privileged twins. Both
//! toolbox `Wallet`s (TS `src/Wallet.ts` and Rust `src/wallet/wallet.rs`)
//! identically reject privileged calls when no privileged key manager is
//! configured, so that gate is outside this corpus; here privileged vectors
//! run through the same deriver, pinning that the flag does not perturb the
//! BRC-42 math.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::GetPublicKeyArgs;
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv::wallet::validation::validate_get_public_key_args;
use serde::Deserialize;

const CORPUS: &str = include_str!("../conformance/vectors/wallet/brc100/getpublickey.json");

#[derive(Deserialize)]
struct VectorFile {
    id: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: VectorInput,
    expected: Expected,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    skip: bool,
}

#[derive(Deserialize)]
struct VectorInput {
    root_key: String,
    args: serde_json::Value,
}

#[derive(Deserialize)]
struct Expected {
    #[serde(rename = "publicKey")]
    public_key: Option<String>,
    #[serde(default)]
    error: bool,
    message: Option<String>,
}

fn load_corpus() -> VectorFile {
    let f: VectorFile = serde_json::from_str(CORPUS).expect("corpus JSON must parse");
    assert_eq!(f.id, "wallet.brc100.getpublickey");
    f
}

/// The corpus shape itself is an assertion: 201 vectors, none marked skip,
/// split 75 standard / 75 privileged / 51 error. If a refresh changes these
/// numbers this test fails and the counts below must be re-verified by hand.
#[test]
fn corpus_shape() {
    let f = load_corpus();
    assert_eq!(f.vectors.len(), 201, "vector count changed on refresh");
    assert_eq!(f.vectors.iter().filter(|v| v.skip).count(), 0);
    let tag_count = |t: &str| {
        f.vectors
            .iter()
            .filter(|v| v.tags.iter().any(|x| x == t))
            .count()
    };
    assert_eq!(tag_count("standard"), 75);
    assert_eq!(tag_count("privileged"), 75);
    assert_eq!(tag_count("error"), 51);
}

/// Run one vector through the toolbox derivation chain.
/// Returns Ok(derived DER hex) or Err(error message).
fn run_vector(v: &Vector) -> Result<String, String> {
    let root = PrivateKey::from_hex(&v.input.root_key)
        .unwrap_or_else(|e| panic!("{}: bad root_key: {e:?}", v.id));

    let args: GetPublicKeyArgs = serde_json::from_value(v.input.args.clone())
        .unwrap_or_else(|e| panic!("{}: args failed to deserialize: {e}", v.id));

    validate_get_public_key_args(&args).map_err(|e| e.to_string())?;

    let protocol = args
        .protocol_id
        .unwrap_or_else(|| panic!("{}: validated args missing protocolID", v.id));
    let key_id = args
        .key_id
        .unwrap_or_else(|| panic!("{}: validated args missing keyID", v.id));
    let counterparty = args.counterparty.unwrap_or_default();

    ProtoWallet::new(root)
        .get_public_key_sync(
            &protocol,
            &key_id,
            &counterparty,
            args.for_self.unwrap_or(false),
            args.identity_key,
        )
        .map(|pk| pk.to_der_hex())
        .map_err(|e| e.to_string())
}

#[test]
fn getpublickey_conformance() {
    let f = load_corpus();
    let mut executed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for v in &f.vectors {
        executed += 1;
        let outcome = run_vector(v);

        if v.expected.error {
            let want_msg = v
                .expected
                .message
                .as_deref()
                .unwrap_or_else(|| panic!("{}: error vector without message", v.id));
            match &outcome {
                Ok(got) => failures.push(format!(
                    "{}: expected error {want_msg:?}, but derivation succeeded with {got}",
                    v.id
                )),
                Err(got_msg) => {
                    // The Rust KeyDeriver emits the TS message with a
                    // lowercase leading "protocol"; compare case-insensitively
                    // so only a REAL message change fails, not the known
                    // capitalization difference.
                    if !got_msg.to_ascii_lowercase().contains(&want_msg.to_ascii_lowercase()) {
                        failures.push(format!(
                            "{}: expected error message {want_msg:?}, got {got_msg:?}",
                            v.id
                        ));
                    }
                }
            }
        } else {
            let want = v
                .expected
                .public_key
                .as_deref()
                .unwrap_or_else(|| panic!("{}: success vector without publicKey", v.id));
            match &outcome {
                Ok(got) if got == want => {}
                Ok(got) => failures.push(format!(
                    "{}: DERIVATION MISMATCH expected {want} got {got}",
                    v.id
                )),
                Err(e) => failures.push(format!(
                    "{}: expected publicKey {want}, got error {e:?}",
                    v.id
                )),
            }
        }
    }

    assert_eq!(executed, 201, "every vector must execute — no silent filtering");
    assert!(
        failures.is_empty(),
        "{} of 201 getPublicKey vectors diverged from the TS reference:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
