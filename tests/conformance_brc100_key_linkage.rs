//! BRC-100 key-linkage conformance runner (72 vectors).
//!
//! The corpus omits `verifier`. The official TypeScript `ProtoWallet` passes
//! that `undefined` value into encryption, whose counterparty defaults to
//! `self`; this runner represents the same effective verifier with the vector
//! root's identity key.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::proto_wallet::{ProtoWallet, RevealCounterpartyResult, RevealSpecificResult};
use bsv::wallet::types::{Counterparty, Protocol};
use serde::Deserialize;
use serde_json::{json, Value};

const COUNTERPARTY: &str =
    include_str!("../conformance/vectors/wallet/brc100/revealcounterpartykeylinkage.json");
const SPECIFIC: &str =
    include_str!("../conformance/vectors/wallet/brc100/revealspecifickeylinkage.json");

#[derive(Deserialize)]
struct VectorFile {
    id: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: Input,
    expected: Value,
    #[serde(default)]
    skip: bool,
}

#[derive(Deserialize)]
struct Input {
    root_key: String,
    args: Value,
}

fn load(corpus: &str, want_id: &str) -> VectorFile {
    let file: VectorFile = serde_json::from_str(corpus).expect("corpus JSON must parse");
    assert_eq!(file.id, want_id);
    file
}

fn wallet(vector: &Vector) -> ProtoWallet {
    ProtoWallet::new(
        PrivateKey::from_hex(&vector.input.root_key)
            .unwrap_or_else(|e| panic!("{}: invalid root_key: {e:?}", vector.id)),
    )
}

fn counterparty(vector: &Vector) -> Counterparty {
    serde_json::from_value(vector.input.args["counterparty"].clone())
        .unwrap_or_else(|e| panic!("{}: invalid counterparty fixture: {e}", vector.id))
}

fn protocol(vector: &Vector) -> Protocol {
    serde_json::from_value(vector.input.args["protocolID"].clone())
        .unwrap_or_else(|e| panic!("{}: invalid protocolID fixture: {e}", vector.id))
}

fn counterparty_json(result: RevealCounterpartyResult) -> Value {
    json!({
        "prover": result.prover.to_der_hex(),
        "encryptedLinkage": result.encrypted_linkage,
    })
}

fn specific_json(result: RevealSpecificResult) -> Value {
    json!({
        "prover": result.prover.to_der_hex(),
        "counterparty": result.counterparty,
        "protocolID": result.protocol,
        "keyID": result.key_id,
        "encryptedLinkage": result.encrypted_linkage,
        "encryptedLinkageProof": result.encrypted_linkage_proof,
    })
}

fn specific_matches_official_dispatcher(expected: &Value, actual: &Value) -> bool {
    // wallet.ts dispatchRevealSpecificKeyLinkage lines 528-535 asserts these
    // three properties, then only fixture fields prover/counterparty/protocolID/keyID.
    for property in ["prover", "encryptedLinkage", "encryptedLinkageProof"] {
        if actual.get(property).is_none() {
            return false;
        }
    }
    for property in ["prover", "counterparty", "protocolID", "keyID"] {
        if let Some(expected_value) = expected.get(property) {
            if actual.get(property) != Some(expected_value) {
                return false;
            }
        }
    }
    true
}

#[test]
fn corpus_shape() {
    for (corpus, id) in [
        (COUNTERPARTY, "wallet.brc100.revealcounterpartykeylinkage"),
        (SPECIFIC, "wallet.brc100.revealspecifickeylinkage"),
    ] {
        let file = load(corpus, id);
        assert_eq!(file.vectors.len(), 36, "{id}: vector count changed");
        assert_eq!(file.vectors.iter().filter(|v| v.skip).count(), 0);
    }
}

#[test]
fn revealcounterpartykeylinkage_conformance() {
    let file = load(COUNTERPARTY, "wallet.brc100.revealcounterpartykeylinkage");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let proto = wallet(vector);
        let verifier = PrivateKey::from_hex(&vector.input.root_key)
            .expect("validated root")
            .to_public_key();
        let outcome = proto
            .reveal_counterparty_key_linkage_sync(&counterparty(vector), &verifier)
            .map(counterparty_json);

        // wallet.ts dispatchRevealCounterpartyKeyLinkage lines 505-513: error
        // vectors only need `rejects.toThrow()`; a success needs `prover` and
        // `encryptedLinkage` properties. Error text/identity is not asserted.
        let expects_error = vector.expected.get("error").and_then(Value::as_bool) == Some(true);
        match (expects_error, outcome) {
            (true, Err(_)) => {}
            (true, Ok(actual)) => failures.push(format!(
                "{}: expected rejection, got success {}",
                vector.id, actual
            )),
            (false, Ok(actual))
                if actual.get("prover").is_some() && actual.get("encryptedLinkage").is_some() => {}
            (false, Ok(actual)) => failures.push(format!(
                "{}: result lacks official dispatcher properties: {}",
                vector.id, actual
            )),
            (false, Err(error)) => failures.push(format!(
                "{}: expected linkage result shape, got error {error}",
                vector.id
            )),
        }
    }

    assert_eq!(file.vectors.len(), 36, "every vector must execute");
    assert!(
        failures.is_empty(),
        "{} of 36 revealCounterpartyKeyLinkage vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn revealspecifickeylinkage_conformance() {
    let file = load(SPECIFIC, "wallet.brc100.revealspecifickeylinkage");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let proto = wallet(vector);
        let verifier = PrivateKey::from_hex(&vector.input.root_key)
            .expect("validated root")
            .to_public_key();
        let key_id = vector.input.args["keyID"]
            .as_str()
            .unwrap_or_else(|| panic!("{}: missing keyID", vector.id));
        let outcome = proto
            .reveal_specific_key_linkage_sync(
                &counterparty(vector),
                &verifier,
                &protocol(vector),
                key_id,
            )
            .map(specific_json);
        let expects_error = vector.expected.get("error").and_then(Value::as_bool) == Some(true);

        // wallet.ts dispatchRevealSpecificKeyLinkage lines 523-535: error
        // vectors require any throw; success asserts only the documented shape
        // and four deterministic fixture fields, never randomized ciphertext.
        match (expects_error, outcome) {
            (false, Ok(actual))
                if specific_matches_official_dispatcher(&vector.expected, &actual) => {}
            (false, Ok(actual)) => failures.push(format!(
                "{}: result does not satisfy official dispatcher shape: {}",
                vector.id, actual
            )),
            (false, Err(error)) => failures.push(format!(
                "{}: expected {}, got error {error}",
                vector.id, vector.expected
            )),
            (true, Err(_)) => {}
            (true, Ok(actual)) => failures.push(format!(
                "{}: expected error {}, got success {}",
                vector.id, vector.expected, actual
            )),
        }
    }

    assert_eq!(file.vectors.len(), 36, "every vector must execute");
    assert!(
        failures.is_empty(),
        "{} of 36 revealSpecificKeyLinkage vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
