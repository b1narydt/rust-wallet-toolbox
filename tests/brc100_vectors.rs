//! Round-trip every BRC-100 wire vector through the Rust SDK types that this
//! crate re-exports.
//!
//! This closes PR review §"Test coverage gaps" §5: the 62 vector files in
//! `testdata/brc100/` had no Rust consumer, so field renames or drops in the
//! args/result types could silently diverge from the TypeScript spec.
//!
//! Each vector is a JSON object of the shape:
//! ```json
//! { "json": { ...method payload... }, "wire": "<hex>" }
//! ```
//! We extract the `json` sub-object, deserialize it into the corresponding
//! Rust type, re-serialize to `serde_json::Value`, and assert structural
//! equality against the parsed raw payload. Any rename, drop, or casing
//! divergence between the TS-derived vector and the Rust serde representation
//! causes the test to fail.
//!
//! One `#[test]` per vector file so `cargo test` reports failures individually.
//!
//! Methods that take no arguments (`isAuthenticated`, `waitForAuthentication`,
//! `getHeight`, `getNetwork`, `getVersion`) have empty-object args vectors
//! (`{"json": {}}`). Those files have no corresponding Rust "args" struct, so
//! we only round-trip their result.
//!
//! All 57 tests pass against bsv-sdk 0.2.81. Earlier drift in the SDK's
//! `reference` byte-array encoding and missing `skip_serializing_if` on
//! default-valued fields was tracked as `bsv-rust-sdk` issue #24 and fixed
//! upstream.

use std::path::PathBuf;

use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, AcquireCertificateArgs, AuthenticatedResult, Certificate,
    CreateActionArgs, CreateActionResult, CreateHmacArgs, CreateHmacResult, CreateSignatureArgs,
    CreateSignatureResult, DecryptArgs, DecryptResult, DiscoverByAttributesArgs,
    DiscoverByIdentityKeyArgs, DiscoverCertificatesResult, EncryptArgs, EncryptResult,
    GetHeaderArgs, GetHeaderResult, GetHeightResult, GetNetworkResult, GetPublicKeyArgs,
    GetPublicKeyResult, GetVersionResult, InternalizeActionArgs, InternalizeActionResult,
    ListActionsArgs, ListActionsResult, ListCertificatesArgs, ListCertificatesResult,
    ListOutputsArgs, ListOutputsResult, ProveCertificateArgs, ProveCertificateResult,
    RelinquishCertificateArgs, RelinquishCertificateResult, RelinquishOutputArgs,
    RelinquishOutputResult, RevealCounterpartyKeyLinkageArgs, RevealCounterpartyKeyLinkageResult,
    RevealSpecificKeyLinkageArgs, RevealSpecificKeyLinkageResult, SignActionArgs, SignActionResult,
    VerifyHmacArgs, VerifyHmacResult, VerifySignatureArgs, VerifySignatureResult,
};

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/brc100")
}

fn read_vector(name: &str) -> serde_json::Value {
    let path = vectors_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let wrapper: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{}: failed to parse outer JSON: {}", name, e));
    wrapper
        .get("json")
        .cloned()
        .unwrap_or_else(|| panic!("{}: vector has no `json` key: {:#}", name, wrapper))
}

/// Round-trip the `json` payload of `vector_name` through `T`.
///
/// Asserts structural equality between the original JSON payload and the
/// payload produced by `serde_json::to_value(T::deserialize(payload))`. We
/// compare as `serde_json::Value`, which uses `BTreeMap`-backed `Map`s (i.e.
/// keys sorted alphabetically) in both directions — this normalizes any
/// insertion-order differences that `HashMap<String, _>` fields would otherwise
/// introduce.
fn assert_roundtrip<T>(vector_name: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let raw = read_vector(vector_name);

    let typed: T = serde_json::from_value(raw.clone()).unwrap_or_else(|e| {
        panic!(
            "{vector_name}: deserialize into {ty} failed: {e}\nPayload:\n{pretty}",
            ty = std::any::type_name::<T>(),
            pretty = serde_json::to_string_pretty(&raw).unwrap_or_default(),
        )
    });

    let reserialized = serde_json::to_value(&typed).unwrap_or_else(|e| {
        panic!(
            "{vector_name}: reserialize from {ty} failed: {e}",
            ty = std::any::type_name::<T>()
        )
    });

    assert_eq!(
        reserialized,
        raw,
        "{vector_name}: round-trip through {ty} diverged from the TS-derived vector.\n\
         Original payload:\n{orig}\n\n\
         Reserialized payload:\n{got}",
        ty = std::any::type_name::<T>(),
        orig = serde_json::to_string_pretty(&raw).unwrap_or_default(),
        got = serde_json::to_string_pretty(&reserialized).unwrap_or_default(),
    );
}

// ---------------------------------------------------------------------------
// createAction
// ---------------------------------------------------------------------------

#[test]
fn create_action_1_out_args_roundtrip() {
    assert_roundtrip::<CreateActionArgs>("createAction-1-out-args.json");
}

#[test]
fn create_action_1_out_result_roundtrip() {
    assert_roundtrip::<CreateActionResult>("createAction-1-out-result.json");
}

#[test]
fn create_action_no_sign_and_process_args_roundtrip() {
    assert_roundtrip::<CreateActionArgs>("createAction-no-signAndProcess-args.json");
}

#[test]
fn create_action_no_sign_and_process_result_roundtrip() {
    assert_roundtrip::<CreateActionResult>("createAction-no-signAndProcess-result.json");
}

// ---------------------------------------------------------------------------
// signAction
// ---------------------------------------------------------------------------

#[test]
fn sign_action_simple_args_roundtrip() {
    assert_roundtrip::<SignActionArgs>("signAction-simple-args.json");
}

#[test]
fn sign_action_simple_result_roundtrip() {
    assert_roundtrip::<SignActionResult>("signAction-simple-result.json");
}

// ---------------------------------------------------------------------------
// abortAction
// ---------------------------------------------------------------------------

#[test]
fn abort_action_simple_args_roundtrip() {
    assert_roundtrip::<AbortActionArgs>("abortAction-simple-args.json");
}

#[test]
fn abort_action_simple_result_roundtrip() {
    assert_roundtrip::<AbortActionResult>("abortAction-simple-result.json");
}

// ---------------------------------------------------------------------------
// listActions
// ---------------------------------------------------------------------------

#[test]
fn list_actions_simple_args_roundtrip() {
    assert_roundtrip::<ListActionsArgs>("listActions-simple-args.json");
}

#[test]
fn list_actions_simple_result_roundtrip() {
    assert_roundtrip::<ListActionsResult>("listActions-simple-result.json");
}

// ---------------------------------------------------------------------------
// internalizeAction
// ---------------------------------------------------------------------------

#[test]
fn internalize_action_simple_args_roundtrip() {
    assert_roundtrip::<InternalizeActionArgs>("internalizeAction-simple-args.json");
}

#[test]
fn internalize_action_simple_result_roundtrip() {
    assert_roundtrip::<InternalizeActionResult>("internalizeAction-simple-result.json");
}

// ---------------------------------------------------------------------------
// listOutputs / relinquishOutput
// ---------------------------------------------------------------------------

#[test]
fn list_outputs_simple_args_roundtrip() {
    assert_roundtrip::<ListOutputsArgs>("listOutputs-simple-args.json");
}

#[test]
fn list_outputs_simple_result_roundtrip() {
    assert_roundtrip::<ListOutputsResult>("listOutputs-simple-result.json");
}

#[test]
fn relinquish_output_simple_args_roundtrip() {
    assert_roundtrip::<RelinquishOutputArgs>("relinquishOutput-simple-args.json");
}

#[test]
fn relinquish_output_simple_result_roundtrip() {
    assert_roundtrip::<RelinquishOutputResult>("relinquishOutput-simple-result.json");
}

// ---------------------------------------------------------------------------
// listCertificates
// ---------------------------------------------------------------------------

#[test]
fn list_certificates_simple_args_roundtrip() {
    assert_roundtrip::<ListCertificatesArgs>("listCertificates-simple-args.json");
}

#[test]
fn list_certificates_simple_result_roundtrip() {
    assert_roundtrip::<ListCertificatesResult>("listCertificates-simple-result.json");
}

#[test]
fn list_certificates_full_args_roundtrip() {
    assert_roundtrip::<ListCertificatesArgs>("listCertificates-full-args.json");
}

#[test]
fn list_certificates_full_result_roundtrip() {
    assert_roundtrip::<ListCertificatesResult>("listCertificates-full-result.json");
}

// ---------------------------------------------------------------------------
// acquireCertificate
// ---------------------------------------------------------------------------
//
// The `acquireCertificate` method returns a bare `Certificate` (see
// `bsv::wallet::interfaces::WalletInterface::acquire_certificate`), not a
// dedicated `AcquireCertificateResult` type — the task's type-mapping table
// called this out as "check the SDK". Confirmed: result round-trips through
// `Certificate`.

#[test]
fn acquire_certificate_simple_args_roundtrip() {
    assert_roundtrip::<AcquireCertificateArgs>("acquireCertificate-simple-args.json");
}

#[test]
fn acquire_certificate_simple_result_roundtrip() {
    assert_roundtrip::<Certificate>("acquireCertificate-simple-result.json");
}

#[test]
fn acquire_certificate_issuance_args_roundtrip() {
    assert_roundtrip::<AcquireCertificateArgs>("acquireCertificate-issuance-args.json");
}

#[test]
fn acquire_certificate_issuance_result_roundtrip() {
    assert_roundtrip::<Certificate>("acquireCertificate-issuance-result.json");
}

// ---------------------------------------------------------------------------
// proveCertificate / relinquishCertificate
// ---------------------------------------------------------------------------

#[test]
fn prove_certificate_simple_args_roundtrip() {
    assert_roundtrip::<ProveCertificateArgs>("proveCertificate-simple-args.json");
}

#[test]
fn prove_certificate_simple_result_roundtrip() {
    assert_roundtrip::<ProveCertificateResult>("proveCertificate-simple-result.json");
}

#[test]
fn relinquish_certificate_simple_args_roundtrip() {
    assert_roundtrip::<RelinquishCertificateArgs>("relinquishCertificate-simple-args.json");
}

#[test]
fn relinquish_certificate_simple_result_roundtrip() {
    assert_roundtrip::<RelinquishCertificateResult>("relinquishCertificate-simple-result.json");
}

// ---------------------------------------------------------------------------
// discover*
// ---------------------------------------------------------------------------

#[test]
fn discover_by_identity_key_simple_args_roundtrip() {
    assert_roundtrip::<DiscoverByIdentityKeyArgs>("discoverByIdentityKey-simple-args.json");
}

#[test]
fn discover_by_identity_key_simple_result_roundtrip() {
    assert_roundtrip::<DiscoverCertificatesResult>("discoverByIdentityKey-simple-result.json");
}

#[test]
fn discover_by_attributes_simple_args_roundtrip() {
    assert_roundtrip::<DiscoverByAttributesArgs>("discoverByAttributes-simple-args.json");
}

#[test]
fn discover_by_attributes_simple_result_roundtrip() {
    assert_roundtrip::<DiscoverCertificatesResult>("discoverByAttributes-simple-result.json");
}

// ---------------------------------------------------------------------------
// Authentication & network metadata (no-args methods)
// ---------------------------------------------------------------------------
//
// `isAuthenticated`, `waitForAuthentication`, `getHeight`, `getNetwork`, and
// `getVersion` take no arguments in the SDK (`async fn ...(&self) -> ...`).
// Their `*-args.json` vectors are `{"json": {}}` placeholders with no
// corresponding Rust struct, so there is nothing to round-trip. We only
// exercise the result vectors.

#[test]
fn is_authenticated_simple_result_roundtrip() {
    assert_roundtrip::<AuthenticatedResult>("isAuthenticated-simple-result.json");
}

#[test]
fn wait_for_authentication_simple_result_roundtrip() {
    assert_roundtrip::<AuthenticatedResult>("waitForAuthentication-simple-result.json");
}

#[test]
fn get_height_simple_result_roundtrip() {
    assert_roundtrip::<GetHeightResult>("getHeight-simple-result.json");
}

#[test]
fn get_header_for_height_simple_args_roundtrip() {
    assert_roundtrip::<GetHeaderArgs>("getHeaderForHeight-simple-args.json");
}

#[test]
fn get_header_for_height_simple_result_roundtrip() {
    assert_roundtrip::<GetHeaderResult>("getHeaderForHeight-simple-result.json");
}

#[test]
fn get_network_simple_result_roundtrip() {
    assert_roundtrip::<GetNetworkResult>("getNetwork-simple-result.json");
}

#[test]
fn get_version_simple_result_roundtrip() {
    assert_roundtrip::<GetVersionResult>("getVersion-simple-result.json");
}

// ---------------------------------------------------------------------------
// getPublicKey
// ---------------------------------------------------------------------------

#[test]
fn get_public_key_simple_args_roundtrip() {
    assert_roundtrip::<GetPublicKeyArgs>("getPublicKey-simple-args.json");
}

#[test]
fn get_public_key_simple_result_roundtrip() {
    assert_roundtrip::<GetPublicKeyResult>("getPublicKey-simple-result.json");
}

// ---------------------------------------------------------------------------
// encrypt / decrypt
// ---------------------------------------------------------------------------

#[test]
fn encrypt_simple_args_roundtrip() {
    assert_roundtrip::<EncryptArgs>("encrypt-simple-args.json");
}

#[test]
fn encrypt_simple_result_roundtrip() {
    assert_roundtrip::<EncryptResult>("encrypt-simple-result.json");
}

#[test]
fn decrypt_simple_args_roundtrip() {
    assert_roundtrip::<DecryptArgs>("decrypt-simple-args.json");
}

#[test]
fn decrypt_simple_result_roundtrip() {
    assert_roundtrip::<DecryptResult>("decrypt-simple-result.json");
}

// ---------------------------------------------------------------------------
// createSignature / verifySignature
// ---------------------------------------------------------------------------

#[test]
fn create_signature_simple_args_roundtrip() {
    assert_roundtrip::<CreateSignatureArgs>("createSignature-simple-args.json");
}

#[test]
fn create_signature_simple_result_roundtrip() {
    assert_roundtrip::<CreateSignatureResult>("createSignature-simple-result.json");
}

#[test]
fn verify_signature_simple_args_roundtrip() {
    assert_roundtrip::<VerifySignatureArgs>("verifySignature-simple-args.json");
}

#[test]
fn verify_signature_simple_result_roundtrip() {
    assert_roundtrip::<VerifySignatureResult>("verifySignature-simple-result.json");
}

// ---------------------------------------------------------------------------
// createHmac / verifyHmac
// ---------------------------------------------------------------------------

#[test]
fn create_hmac_simple_args_roundtrip() {
    assert_roundtrip::<CreateHmacArgs>("createHmac-simple-args.json");
}

#[test]
fn create_hmac_simple_result_roundtrip() {
    assert_roundtrip::<CreateHmacResult>("createHmac-simple-result.json");
}

#[test]
fn verify_hmac_simple_args_roundtrip() {
    assert_roundtrip::<VerifyHmacArgs>("verifyHmac-simple-args.json");
}

#[test]
fn verify_hmac_simple_result_roundtrip() {
    assert_roundtrip::<VerifyHmacResult>("verifyHmac-simple-result.json");
}

// ---------------------------------------------------------------------------
// revealCounterpartyKeyLinkage / revealSpecificKeyLinkage
// ---------------------------------------------------------------------------

#[test]
fn reveal_counterparty_key_linkage_simple_args_roundtrip() {
    assert_roundtrip::<RevealCounterpartyKeyLinkageArgs>(
        "revealCounterpartyKeyLinkage-simple-args.json",
    );
}

#[test]
fn reveal_counterparty_key_linkage_simple_result_roundtrip() {
    assert_roundtrip::<RevealCounterpartyKeyLinkageResult>(
        "revealCounterpartyKeyLinkage-simple-result.json",
    );
}

#[test]
fn reveal_specific_key_linkage_simple_args_roundtrip() {
    assert_roundtrip::<RevealSpecificKeyLinkageArgs>("revealSpecificKeyLinkage-simple-args.json");
}

#[test]
fn reveal_specific_key_linkage_simple_result_roundtrip() {
    assert_roundtrip::<RevealSpecificKeyLinkageResult>(
        "revealSpecificKeyLinkage-simple-result.json",
    );
}
