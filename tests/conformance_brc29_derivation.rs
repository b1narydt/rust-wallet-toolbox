//! BRC-29 payment-key derivation conformance runner (27 vectors).

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::GetPublicKeyArgs;
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv::wallet::validation::validate_get_public_key_args;
use serde::Deserialize;

const CORPUS: &str = include_str!("../conformance/vectors/wallet/brc29/payment-derivation.json");

#[derive(Deserialize)]
struct VectorFile {
    id: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    input: Input,
    expected: Expected,
    #[serde(default)]
    skip: bool,
}

#[derive(Deserialize)]
struct Input {
    root_key: String,
    args: serde_json::Value,
}

#[derive(Deserialize)]
struct Expected {
    #[serde(rename = "publicKey")]
    public_key: String,
}

fn load() -> VectorFile {
    let file: VectorFile = serde_json::from_str(CORPUS).expect("corpus JSON must parse");
    assert_eq!(file.id, "wallet.brc29.payment-derivation");
    file
}

#[test]
fn corpus_shape() {
    let file = load();
    assert_eq!(file.vectors.len(), 27, "vector count changed on refresh");
    assert_eq!(file.vectors.iter().filter(|v| v.skip).count(), 0);
}

#[test]
fn payment_derivation_conformance() {
    let file = load();
    let mut failures = Vec::new();

    for vector in &file.vectors {
        let outcome = (|| {
            let root = PrivateKey::from_hex(&vector.input.root_key)
                .map_err(|e| format!("invalid root_key: {e:?}"))?;
            let args: GetPublicKeyArgs = serde_json::from_value(vector.input.args.clone())
                .map_err(|e| format!("args failed to deserialize: {e}"))?;
            validate_get_public_key_args(&args).map_err(|e| e.to_string())?;
            let protocol = args
                .protocol_id
                .ok_or("validated args missing protocolID")?;
            let key_id = args.key_id.ok_or("validated args missing keyID")?;
            ProtoWallet::new(root)
                .get_public_key_sync(
                    &protocol,
                    &key_id,
                    &args.counterparty.unwrap_or_default(),
                    args.for_self.unwrap_or(false),
                    args.identity_key,
                )
                .map(|key| key.to_der_hex())
                .map_err(|e| e.to_string())
        })();

        match outcome {
            // wallet.ts dispatchPaymentDerivation lines 991-1000 invokes the
            // real ProtoWallet derivation and asserts exact `publicKey` when
            // the fixture provides it; all 27 fixtures do.
            Ok(actual) if actual == vector.expected.public_key => {}
            Ok(actual) => failures.push(format!(
                "{}: expected {}, got {actual}",
                vector.id, vector.expected.public_key
            )),
            Err(error) => failures.push(format!(
                "{}: expected {}, got error {error}",
                vector.id, vector.expected.public_key
            )),
        }
    }

    assert_eq!(file.vectors.len(), 27, "every vector must execute");
    assert!(
        failures.is_empty(),
        "{} of 27 BRC-29 vectors diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
