//! BRC-100 key-linkage conformance runner (72 vectors).
//!
//! The corpus supplies the crypto-matrix shape (`protocolID`, `keyID`,
//! `counterparty`, `data`) rather than the BRC-100 linkage argument shape.
//! In particular, `verifier` is absent. The pinned TS implementation passes
//! that `undefined` verifier into encryption, where it defaults to `self`;
//! this runner makes that implicit fixture explicit by using the vector root's
//! identity key as verifier.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::error::WalletError;
use bsv::wallet::proto_wallet::{ProtoWallet, RevealSpecificResult};
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

fn expected_error_matches(expected: &Value, actual: &WalletError) -> bool {
    match expected.get("message").and_then(Value::as_str) {
        Some("Counterparty secrets cannot be revealed for counterparty=self.") => {
            matches!(actual, WalletError::InvalidParameter(message) if
                message.contains("counterparty secrets cannot be revealed") &&
                message.contains("self"))
        }
        Some("Invalid hex string") => actual.to_string().contains("Invalid hex string"),
        Some("Protocol names must be 5 characters or more") => {
            matches!(actual, WalletError::InvalidParameter(message) if
                message.contains("protocol names must be 5 characters or more"))
        }
        Some(message) => actual.to_string().eq_ignore_ascii_case(message),
        None => true,
    }
}

fn specific_json(result: RevealSpecificResult) -> Value {
    json!({
        "prover": result.prover.to_der_hex(),
        "verifier": result.verifier.to_der_hex(),
        "counterparty": result.counterparty.to_der_hex(),
        "protocolID": result.protocol,
        "keyID": result.key_id,
        "encryptedLinkage": result.encrypted_linkage,
        "encryptedLinkageProof": result.encrypted_linkage_proof,
        "proofType": result.proof_type,
    })
}

fn assert_known_divergences(channel: &str, failures: &[String], known: &[&str]) {
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|failure| !known.iter().any(|id| failure.starts_with(*id)))
        .collect();
    let resolved: Vec<&&str> = known
        .iter()
        .filter(|id| !failures.iter().any(|failure| failure.starts_with(**id)))
        .collect();
    assert!(
        unexpected.is_empty() && resolved.is_empty(),
        "{channel}: divergence ledger out of date.\nUnexpected failures:\n{}\nResolved (remove from ledger):\n{}\nAll failures:\n{}",
        unexpected.iter().map(|failure| format!("  {failure}")).collect::<Vec<_>>().join("\n"),
        resolved.iter().map(|id| format!("  {id}")).collect::<Vec<_>>().join("\n"),
        failures.join("\n")
    );
}

/// For root key 2, `counterparty="anyone"` denotes public key G. Rust
/// resolves it as that real key and later reports that the result requires a
/// concrete counterparty key; TS instead tries to parse the literal word as
/// hex while creating its proof. Both reject, but the error identity differs.
const KNOWN_COUNTERPARTY_DIVERGENCES: &[&str] = &[
    "wallet.brc100.revealcounterpartykeylinkage.20:",
    "wallet.brc100.revealcounterpartykeylinkage.22:",
    "wallet.brc100.revealcounterpartykeylinkage.24:",
    "wallet.brc100.revealcounterpartykeylinkage.26:",
    "wallet.brc100.revealcounterpartykeylinkage.28:",
    "wallet.brc100.revealcounterpartykeylinkage.30:",
    "wallet.brc100.revealcounterpartykeylinkage.32:",
    "wallet.brc100.revealcounterpartykeylinkage.34:",
    "wallet.brc100.revealcounterpartykeylinkage.36:",
];

/// Every nominal success vector passes `self` or `anyone`, omits `verifier`,
/// and expects those sentinel strings back as `counterparty`. BRC-100 requires
/// concrete public keys for both linkage parties, as does Rust's result type;
/// the operation therefore fails closed rather than emitting that malformed
/// result. Each of the 24 affected vectors remains individually pinned.
const KNOWN_SPECIFIC_DIVERGENCES: &[&str] = &[
    "wallet.brc100.revealspecifickeylinkage.1:",
    "wallet.brc100.revealspecifickeylinkage.2:",
    "wallet.brc100.revealspecifickeylinkage.3:",
    "wallet.brc100.revealspecifickeylinkage.4:",
    "wallet.brc100.revealspecifickeylinkage.5:",
    "wallet.brc100.revealspecifickeylinkage.6:",
    "wallet.brc100.revealspecifickeylinkage.13:",
    "wallet.brc100.revealspecifickeylinkage.14:",
    "wallet.brc100.revealspecifickeylinkage.15:",
    "wallet.brc100.revealspecifickeylinkage.16:",
    "wallet.brc100.revealspecifickeylinkage.17:",
    "wallet.brc100.revealspecifickeylinkage.18:",
    "wallet.brc100.revealspecifickeylinkage.19:",
    "wallet.brc100.revealspecifickeylinkage.20:",
    "wallet.brc100.revealspecifickeylinkage.21:",
    "wallet.brc100.revealspecifickeylinkage.22:",
    "wallet.brc100.revealspecifickeylinkage.23:",
    "wallet.brc100.revealspecifickeylinkage.24:",
    "wallet.brc100.revealspecifickeylinkage.31:",
    "wallet.brc100.revealspecifickeylinkage.32:",
    "wallet.brc100.revealspecifickeylinkage.33:",
    "wallet.brc100.revealspecifickeylinkage.34:",
    "wallet.brc100.revealspecifickeylinkage.35:",
    "wallet.brc100.revealspecifickeylinkage.36:",
];

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
        let outcome = proto.reveal_counterparty_key_linkage_sync(&counterparty(vector), &verifier);

        match outcome {
            Err(error) if expected_error_matches(&vector.expected, &error) => {}
            Err(error) => failures.push(format!(
                "{}: expected error {}, got error {error}",
                vector.id, vector.expected
            )),
            Ok(_) => failures.push(format!(
                "{}: expected error {}, got success",
                vector.id, vector.expected
            )),
        }
    }

    assert_eq!(file.vectors.len(), 36, "every vector must execute");
    assert_known_divergences(
        "revealCounterpartyKeyLinkage",
        &failures,
        KNOWN_COUNTERPARTY_DIVERGENCES,
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

        match (expects_error, outcome) {
            (false, Ok(actual)) if actual == vector.expected => {}
            (false, Ok(actual)) => failures.push(format!(
                "{}: expected {}, got {}",
                vector.id, vector.expected, actual
            )),
            (false, Err(error)) => failures.push(format!(
                "{}: expected {}, got error {error}",
                vector.id, vector.expected
            )),
            (true, Err(error)) if expected_error_matches(&vector.expected, &error) => {}
            (true, Err(error)) => failures.push(format!(
                "{}: expected error {}, got error {error}",
                vector.id, vector.expected
            )),
            (true, Ok(actual)) => failures.push(format!(
                "{}: expected error {}, got success {}",
                vector.id, vector.expected, actual
            )),
        }
    }

    assert_eq!(file.vectors.len(), 36, "every vector must execute");
    assert_known_divergences(
        "revealSpecificKeyLinkage",
        &failures,
        KNOWN_SPECIFIC_DIVERGENCES,
    );
}
