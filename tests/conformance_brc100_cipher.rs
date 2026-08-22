//! BRC-100 encrypt/decrypt conformance runner (87 vectors).
//!
//! AES-GCM encryption prepends a random 32-byte IV, so successful encryption
//! is asserted by decrypting the fresh ciphertext and recovering the fixture
//! plaintext, exactly as the official dispatcher does.

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
        // wallet.ts dispatchEncrypt lines 464-466 and dispatchDecrypt lines
        // 486-488 use `rejects.toThrow()`: only rejection is asserted.
        (true, Err(_)) => {}
        (true, Ok(actual)) => failures.push(format!(
            "{}: expected error {}, got success {}",
            vector.id, vector.expected, actual
        )),
    }
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
            let proto = wallet(vector);
            let ciphertext = proto.encrypt_sync(
                &args.plaintext,
                &args.protocol_id,
                &args.key_id,
                &args.counterparty,
            )?;
            let recovered = proto.decrypt_sync(
                &ciphertext,
                &args.protocol_id,
                &args.key_id,
                &args.counterparty,
            )?;
            Ok(json!({ "plaintext": recovered }))
        })();

        // wallet.ts dispatchEncrypt lines 454-476: SymmetricKey.encrypt uses a
        // random IV, so the official assertion is ciphertext presence plus
        // `decrypt(encrypt(plaintext)) == plaintext`, never fixture bytes.
        if vector.expected.get("error").and_then(Value::as_bool) == Some(true) {
            compare(vector, outcome, &mut failures);
        } else {
            match outcome {
                Ok(actual)
                    if actual
                        == json!({ "plaintext": encrypt_args(vector).expect("valid success args").plaintext }) =>
                    {}
                Ok(actual) => failures.push(format!(
                    "{}: round-trip did not recover plaintext: got {}",
                    vector.id, actual
                )),
                Err(error) => failures.push(format!(
                    "{}: expected successful encrypt round-trip, got error {error}",
                    vector.id
                )),
            }
        }
    }

    assert_eq!(file.vectors.len(), 36, "every encrypt vector must execute");
    assert!(
        failures.is_empty(),
        "{} of 36 encrypt vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
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
        // wallet.ts dispatchDecrypt lines 486-495: rejection for an error;
        // otherwise `plaintext` must exist and equal the fixture bytes.
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
