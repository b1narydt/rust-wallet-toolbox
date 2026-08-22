//! BRC-100 signature and HMAC conformance runner (223 vectors).
//!
//! The official files use UTF-8 strings for create-operation `data` and byte
//! arrays for verify-operation `data`; [`data_bytes`] performs exactly that
//! wire-fixture conversion before the SDK argument structs are built.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::error::WalletError;
use bsv::wallet::interfaces::{
    CreateHmacArgs, CreateSignatureArgs, VerifyHmacArgs, VerifySignatureArgs,
};
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv::wallet::validation::{
    validate_create_hmac_args, validate_create_signature_args, validate_verify_hmac_args,
    validate_verify_signature_args,
};
use serde::Deserialize;
use serde_json::{json, Value};

const CREATE_HMAC: &str = include_str!("../conformance/vectors/wallet/brc100/createhmac.json");
const VERIFY_HMAC: &str = include_str!("../conformance/vectors/wallet/brc100/verifyhmac.json");
const CREATE_SIGNATURE: &str =
    include_str!("../conformance/vectors/wallet/brc100/createsignature.json");
const VERIFY_SIGNATURE: &str =
    include_str!("../conformance/vectors/wallet/brc100/verifysignature.json");

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
    let root = PrivateKey::from_hex(&vector.input.root_key)
        .unwrap_or_else(|e| panic!("{}: invalid root_key: {e:?}", vector.id));
    ProtoWallet::new(root)
}

fn args_with_utf8_data(vector: &Vector) -> Value {
    let mut args = vector.input.args.clone();
    let data = args["data"]
        .as_str()
        .unwrap_or_else(|| panic!("{}: expected UTF-8 string data fixture", vector.id));
    args["data"] = json!(data.as_bytes());
    args
}

fn expected_error_matches(expected: &Value, actual: &WalletError) -> bool {
    match expected.get("code").and_then(Value::as_str) {
        Some("ERR_INVALID_HMAC") => matches!(actual, WalletError::InvalidHmac),
        Some("ERR_INVALID_SIGNATURE") => matches!(actual, WalletError::InvalidSignature),
        Some(_) => false,
        None => match expected.get("message").and_then(Value::as_str) {
            Some("Protocol names can only contain letters, numbers and spaces") => {
                matches!(actual, WalletError::InvalidParameter(message) if
                    message.contains("only lowercase letters, numbers, and spaces"))
            }
            Some("Protocol names must be 5 characters or more") => {
                matches!(actual, WalletError::InvalidParameter(message) if
                    message.contains("protocol names must be 5 characters or more"))
            }
            Some("Signature is not valid") => matches!(
                actual,
                WalletError::InvalidSignature | WalletError::Internal(_)
            ),
            Some(message) => actual.to_string().eq_ignore_ascii_case(message),
            None => true,
        },
    }
}

fn compare_outcome(
    vector: &Vector,
    outcome: Result<Value, WalletError>,
    failures: &mut Vec<String>,
) {
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

fn assert_channel(channel: &str, executed: usize, want: usize, failures: &[String]) {
    assert_eq!(executed, want, "every {channel} vector must execute");
    assert!(
        failures.is_empty(),
        "{} of {want} {channel} vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// These eight vectors pass an empty signature and expect the generic TS
/// `Signature is not valid` error. Rust rejects the empty byte array at the
/// BRC-100 argument-validation boundary, before DER verification. Both fail,
/// but the error identity differs and that difference is pinned here.
const KNOWN_VERIFY_SIGNATURE_DIVERGENCES: &[&str] = &[
    "wallet.brc100.verifysignature.46:",
    "wallet.brc100.verifysignature.50:",
    "wallet.brc100.verifysignature.54:",
    "wallet.brc100.verifysignature.64:",
    "wallet.brc100.verifysignature.68:",
    "wallet.brc100.verifysignature.72:",
    "wallet.brc100.verifysignature.76:",
    "wallet.brc100.verifysignature.80:",
];

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

#[test]
fn corpus_shape() {
    for (corpus, id, count) in [
        (CREATE_HMAC, "wallet.brc100.createhmac", 36),
        (VERIFY_HMAC, "wallet.brc100.verifyhmac", 70),
        (CREATE_SIGNATURE, "wallet.brc100.createsignature", 36),
        (VERIFY_SIGNATURE, "wallet.brc100.verifysignature", 81),
    ] {
        let file = load(corpus, id);
        assert_eq!(file.vectors.len(), count, "{id}: vector count changed");
        assert_eq!(file.vectors.iter().filter(|v| v.skip).count(), 0);
    }
}

#[test]
fn createhmac_conformance() {
    let file = load(CREATE_HMAC, "wallet.brc100.createhmac");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args: CreateHmacArgs = serde_json::from_value(args_with_utf8_data(vector))
                .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))?;
            validate_create_hmac_args(&args)?;
            wallet(vector)
                .create_hmac_sync(
                    &args.data,
                    &args.protocol_id,
                    &args.key_id,
                    &args.counterparty,
                )
                .map(|hmac| json!({ "hmac": hmac }))
        })();
        compare_outcome(vector, outcome, &mut failures);
    }

    assert_channel("createHmac", file.vectors.len(), 36, &failures);
}

#[test]
fn verifyhmac_conformance() {
    let file = load(VERIFY_HMAC, "wallet.brc100.verifyhmac");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args: VerifyHmacArgs = serde_json::from_value(vector.input.args.clone())
                .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))?;
            validate_verify_hmac_args(&args)?;
            match wallet(vector).verify_hmac_sync(
                &args.data,
                &args.hmac,
                &args.protocol_id,
                &args.key_id,
                &args.counterparty,
            )? {
                true => Ok(json!({ "valid": true })),
                false => Err(WalletError::InvalidHmac),
            }
        })();
        compare_outcome(vector, outcome, &mut failures);
    }

    assert_channel("verifyHmac", file.vectors.len(), 70, &failures);
}

#[test]
fn createsignature_conformance() {
    let file = load(CREATE_SIGNATURE, "wallet.brc100.createsignature");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args: CreateSignatureArgs = serde_json::from_value(args_with_utf8_data(vector))
                .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))?;
            validate_create_signature_args(&args)?;
            wallet(vector)
                .create_signature_sync(
                    args.data.as_deref(),
                    args.hash_to_directly_sign.as_deref(),
                    &args.protocol_id,
                    &args.key_id,
                    &args.counterparty,
                )
                .map(|signature| json!({ "signature": signature }))
        })();
        compare_outcome(vector, outcome, &mut failures);
    }

    assert_channel("createSignature", file.vectors.len(), 36, &failures);
}

#[test]
fn verifysignature_conformance() {
    let file = load(VERIFY_SIGNATURE, "wallet.brc100.verifysignature");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args: VerifySignatureArgs = serde_json::from_value(vector.input.args.clone())
                .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))?;
            validate_verify_signature_args(&args)?;
            match wallet(vector).verify_signature_sync(
                args.data.as_deref(),
                args.hash_to_directly_verify.as_deref(),
                &args.signature,
                &args.protocol_id,
                &args.key_id,
                &args.counterparty,
                args.for_self.unwrap_or(false),
            )? {
                true => Ok(json!({ "valid": true })),
                false => Err(WalletError::InvalidSignature),
            }
        })();
        compare_outcome(vector, outcome, &mut failures);
    }

    assert_eq!(
        file.vectors.len(),
        81,
        "every verifySignature vector must execute"
    );
    assert_known_divergences(
        "verifySignature",
        &failures,
        KNOWN_VERIFY_SIGNATURE_DIVERGENCES,
    );
}
