//! BRC-100 encrypt/decrypt conformance runner (87 vectors).
//!
//! AES-GCM encryption prepends a random 32-byte IV. Successful encrypt
//! vectors record that IV in the first 32 expected bytes; replaying it through
//! `encrypt_with_iv_sync` makes the entire official ciphertext assertable.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::error::WalletError;
use bsv::wallet::interfaces::{DecryptArgs, EncryptArgs};
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv::wallet::validation::{validate_decrypt_args, validate_encrypt_args};
use serde::Deserialize;
use serde_json::{json, Value};

const ENCRYPT: &str = include_str!("../conformance/vectors/wallet/brc100/encrypt.json");
const DECRYPT: &str = include_str!("../conformance/vectors/wallet/brc100/decrypt.json");

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

fn encrypt_args(vector: &Vector) -> Result<EncryptArgs, WalletError> {
    let mut args = vector.input.args.clone();
    let data = args["data"]
        .as_str()
        .ok_or_else(|| WalletError::Internal("expected UTF-8 string data fixture".to_string()))?;
    args["plaintext"] = json!(data.as_bytes());
    args.as_object_mut().expect("args object").remove("data");
    serde_json::from_value(args)
        .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))
}

fn protocol_error_matches(expected: &Value, actual: &WalletError) -> bool {
    match expected.get("message").and_then(Value::as_str) {
        Some("Protocol names can only contain letters, numbers and spaces") => {
            matches!(actual, WalletError::InvalidParameter(message) if
                message.contains("only lowercase letters, numbers, and spaces"))
        }
        Some("Protocol names must be 5 characters or more") => {
            matches!(actual, WalletError::InvalidParameter(message) if
                message.contains("protocol names must be 5 characters or more"))
        }
        Some(message) => actual.to_string().eq_ignore_ascii_case(message),
        None => true,
    }
}

fn compare(vector: &Vector, outcome: Result<Value, WalletError>, failures: &mut Vec<String>) {
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
        (true, Err(error)) if protocol_error_matches(&vector.expected, &error) => {}
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

/// The success vectors use a non-BRC-100 `data` field rather than
/// `plaintext`. Their 48-byte expectations are `IV || tag`, proving the
/// generator encrypted an empty plaintext after silently ignoring `data`.
/// The pinned TS runner later added the same UTF-8 conversion used here but
/// weakened the assertion to round-trip-only, leaving the stale ciphertexts.
const KNOWN_ENCRYPT_DIVERGENCES: &[&str] = &[
    "wallet.brc100.encrypt.1:",
    "wallet.brc100.encrypt.2:",
    "wallet.brc100.encrypt.3:",
    "wallet.brc100.encrypt.4:",
    "wallet.brc100.encrypt.5:",
    "wallet.brc100.encrypt.6:",
    "wallet.brc100.encrypt.13:",
    "wallet.brc100.encrypt.14:",
    "wallet.brc100.encrypt.15:",
    "wallet.brc100.encrypt.16:",
    "wallet.brc100.encrypt.17:",
    "wallet.brc100.encrypt.18:",
    "wallet.brc100.encrypt.19:",
    "wallet.brc100.encrypt.20:",
    "wallet.brc100.encrypt.21:",
    "wallet.brc100.encrypt.22:",
    "wallet.brc100.encrypt.23:",
    "wallet.brc100.encrypt.24:",
    "wallet.brc100.encrypt.31:",
    "wallet.brc100.encrypt.32:",
    "wallet.brc100.encrypt.33:",
    "wallet.brc100.encrypt.34:",
    "wallet.brc100.encrypt.35:",
    "wallet.brc100.encrypt.36:",
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
        (ENCRYPT, "wallet.brc100.encrypt", 36),
        (DECRYPT, "wallet.brc100.decrypt", 51),
    ] {
        let file = load(corpus, id);
        assert_eq!(file.vectors.len(), count, "{id}: vector count changed");
        assert_eq!(file.vectors.iter().filter(|v| v.skip).count(), 0);
    }
}

#[test]
fn encrypt_conformance() {
    let file = load(ENCRYPT, "wallet.brc100.encrypt");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args = encrypt_args(vector)?;
            validate_encrypt_args(&args)?;

            let mut iv = [0u8; 32];
            if let Some(expected) = vector.expected["ciphertext"].as_array() {
                assert!(
                    expected.len() >= iv.len(),
                    "{}: ciphertext too short",
                    vector.id
                );
                for (dst, src) in iv.iter_mut().zip(expected) {
                    *dst = src
                        .as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .unwrap_or_else(|| panic!("{}: invalid IV byte", vector.id));
                }
            }

            wallet(vector)
                .encrypt_with_iv_sync(
                    &args.plaintext,
                    &args.protocol_id,
                    &args.key_id,
                    &args.counterparty,
                    &iv,
                )
                .map(|ciphertext| json!({ "ciphertext": ciphertext }))
        })();
        compare(vector, outcome, &mut failures);
    }

    assert_eq!(file.vectors.len(), 36, "every encrypt vector must execute");
    assert_known_divergences("encrypt", &failures, KNOWN_ENCRYPT_DIVERGENCES);
}

#[test]
fn decrypt_conformance() {
    let file = load(DECRYPT, "wallet.brc100.decrypt");
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let args: DecryptArgs = serde_json::from_value(vector.input.args.clone())
                .map_err(|e| WalletError::Internal(format!("args failed to deserialize: {e}")))?;
            validate_decrypt_args(&args)?;
            wallet(vector)
                .decrypt_sync(
                    &args.ciphertext,
                    &args.protocol_id,
                    &args.key_id,
                    &args.counterparty,
                )
                .map(|plaintext| json!({ "plaintext": plaintext }))
        })();
        compare(vector, outcome, &mut failures);
    }

    assert_eq!(file.vectors.len(), 51, "every decrypt vector must execute");
    assert!(
        failures.is_empty(),
        "{} of 51 decrypt vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
