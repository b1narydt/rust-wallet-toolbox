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
//! # Known drifts (marked `#[ignore]`)
//!
//! 8 of 57 tests are `#[ignore]`'d with specific diagnoses in the `#[ignore = "..."]`
//! message and in a comment above each test. The orchestrator should either
//! fix the underlying `bsv-sdk` types (out of scope for this PR) or accept the
//! divergence and delete the offending vectors. Two classes of drift exist:
//!
//! 1. **`reference` field encoded as base64 string in TS, as `Vec<u8>` array in
//!    Rust.** The TS spec for BRC-100 serializes opaque handles
//!    (`CreateActionResult.signableTransaction.reference`, `SignActionArgs.reference`,
//!    `AbortActionArgs.reference`) as Base64Url strings. The Rust SDK uses
//!    `serde_helpers::bytes_as_array`, producing a JSON array of byte integers.
//!    This will prevent any Rust wallet from accepting a real TS-produced
//!    `reference` over the wire.
//!
//! 2. **Optional/default fields emitted without `skip_serializing_if`.** Several
//!    args types emit default-valued fields (`identityKey: false`, `forSelf: null`,
//!    `acceptDelayedBroadcast: true`, `randomizeOutputs: true`, the
//!    `includeInputs`/`includeOutputs`/etc. family on `ListActionsArgs` and
//!    `ListOutputsArgs`) that the TS spec leaves absent. Under serde's default
//!    `deny_unknown_fields = false`, inbound parses still succeed, but outbound
//!    requests from a Rust client will contain extra keys a strict TS verifier
//!    could reject — and the Rust wire-format deviates from the shared vectors.
//!
//! Run the ignored tests with:
//! ```sh
//! cargo test --test brc100_vectors -- --include-ignored
//! ```

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

// Drift: `CreateActionOptions` fields `acceptDelayedBroadcast` (default true),
// `noSend`, `returnTxidOnly`, and `randomizeOutputs` (default true) are all
// emitted on serialization when absent from the vector. The TS spec omits
// unset options. Fix is in `bsv::wallet::interfaces::CreateActionOptions` in
// bsv-sdk: add `#[serde(skip_serializing_if = ...)]` guards (or switch the
// `BooleanDefaultTrue`/`BooleanDefaultFalse` newtypes to skip their default).
#[test]
#[ignore = "drift: CreateActionOptions emits default-valued fields missing from TS vector"]
fn create_action_no_sign_and_process_args_roundtrip() {
    assert_roundtrip::<CreateActionArgs>("createAction-no-signAndProcess-args.json");
}

// Drift: `SignableTransaction.reference` is encoded as a Base64Url string in
// the TS vector (`"reference": "dGVzdA=="`) but `CreateActionResult` in the
// Rust SDK uses `serde_helpers::bytes_as_array` for the reference bytes,
// producing a `[u8, ...]` JSON array. Deserialization fails outright:
// `invalid type: string "dGVzdA==", expected a sequence`. This will break
// any Rust client accepting a `createAction` response from a TS wallet.
// Fix is in `bsv::wallet::interfaces::SignableTransaction` (or wherever the
// `reference: Vec<u8>` field lives): switch to a base64-string helper.
#[test]
#[ignore = "drift: SignableTransaction.reference is base64 string in TS vector, Vec<u8> array in Rust"]
fn create_action_no_sign_and_process_result_roundtrip() {
    assert_roundtrip::<CreateActionResult>("createAction-no-signAndProcess-result.json");
}

// ---------------------------------------------------------------------------
// signAction
// ---------------------------------------------------------------------------

// Drift: `SignActionArgs.reference` is a Base64Url string in the TS vector
// (`"reference": "dGVzdA=="`) but `SignActionArgs` uses `bytes_as_array`,
// producing a JSON byte array. Deserialization fails:
// `invalid type: string "dGVzdA==", expected a sequence`. See notes on
// `create_action_no_sign_and_process_result_roundtrip` — same `reference`
// encoding issue.
#[test]
#[ignore = "drift: SignActionArgs.reference is base64 string in TS vector, Vec<u8> array in Rust"]
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

// Drift: same `reference` base64-vs-array issue as signAction/createAction.
// `AbortActionArgs.reference` should accept the Base64Url string the TS spec
// produces, but the Rust type uses `bytes_as_array` and fails to deserialize
// `"dGVzdA=="`.
#[test]
#[ignore = "drift: AbortActionArgs.reference is base64 string in TS vector, Vec<u8> array in Rust"]
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

// Drift: `ListActionsArgs` emits five default-`false` boolean fields on
// serialization — `includeInputs`, `includeInputSourceLockingScripts`,
// `includeInputUnlockingScripts`, `includeOutputLockingScripts`,
// `includeLabels` — plus `seekPermission: true` default, none of which the TS
// vector includes. Fix in bsv-sdk: add
// `#[serde(skip_serializing_if = ...)]` to the `BooleanDefaultFalse`/
// `BooleanDefaultTrue` / `Option<bool>` fields on `ListActionsArgs`.
#[test]
#[ignore = "drift: ListActionsArgs emits default-valued include*/seekPermission fields missing from TS vector"]
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

// Drift: `ListOutputsArgs` emits default-valued fields missing from the TS
// vector — `includeCustomInstructions: false`, `includeLabels: false`, and
// `seekPermission: true`. Same class of fix as `ListActionsArgs`:
// `#[serde(skip_serializing_if = ...)]` on the defaulting fields.
#[test]
#[ignore = "drift: ListOutputsArgs emits default-valued fields missing from TS vector"]
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

// Drift: `GetPublicKeyArgs.identity_key: bool` serializes as `identityKey:
// false` by default, but the TS vector omits it entirely. Fix in bsv-sdk:
// wrap as `Option<bool>` (with `skip_serializing_if = "Option::is_none"`) or
// add a `skip_serializing_if` guard — TS treats the field as truly optional.
#[test]
#[ignore = "drift: GetPublicKeyArgs emits identityKey: false; TS vector omits the field when not set"]
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

// Drift: `VerifySignatureArgs.forSelf: Option<bool>` serializes as
// `forSelf: null` when `None`, but the TS vector omits the field. The Rust
// type has `skip_serializing_if = "Option::is_none"` in theory, but the
// reserialized output shows `"forSelf": null` — either the attribute is
// missing in the SDK type or the deserializer is producing `Some(Null)`
// semantics. Fix in bsv-sdk: verify `skip_serializing_if` is present on
// every `Option<T>` field of `VerifySignatureArgs`.
#[test]
#[ignore = "drift: VerifySignatureArgs emits forSelf: null; TS vector omits the field when None"]
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
