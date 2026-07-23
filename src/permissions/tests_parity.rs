//! Parity regression tests for the permissions subsystem.
//!
//! Each test targets a verified gap against the TypeScript
//! `WalletPermissionsManager` reference:
//!
//! * monthly spend tracking — `create_action` must stamp admin originator/month
//!   labels and `query_spent_since` must sum them, so the DSAP cap accumulates.
//! * privileged token matching — a privileged request must not be satisfied by a
//!   non-privileged token.
//! * PushDrop tokens — an emitted permission token must be a real PushDrop
//!   script (spendable, `OP_CHECKSIG`-guarded), not a length-prefixed blob.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use bsv::primitives::private_key::PrivateKey;
use bsv::script::locking_script::LockingScript;
use bsv::script::templates::push_drop::decode as decode_push_drop;
use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, AcquireCertificateArgs, Action, ActionStatus,
    AuthenticatedResult, Certificate, CreateActionArgs, CreateActionResult, CreateHmacArgs,
    CreateHmacResult, CreateSignatureArgs, CreateSignatureResult, DecryptArgs, DecryptResult,
    DiscoverByAttributesArgs, DiscoverByIdentityKeyArgs, DiscoverCertificatesResult, EncryptArgs,
    EncryptResult, GetHeaderArgs, GetHeaderResult, GetHeightResult, GetNetworkResult,
    GetPublicKeyArgs, GetPublicKeyResult, GetVersionResult, InternalizeActionArgs,
    InternalizeActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, Output, ProveCertificateArgs,
    ProveCertificateResult, RelinquishCertificateArgs, RelinquishCertificateResult,
    RelinquishOutputArgs, RelinquishOutputResult, RevealCounterpartyKeyLinkageArgs,
    RevealCounterpartyKeyLinkageResult, RevealSpecificKeyLinkageArgs,
    RevealSpecificKeyLinkageResult, SignActionArgs, SignActionResult, VerifyHmacArgs,
    VerifyHmacResult, VerifySignatureArgs, VerifySignatureResult, WalletInterface,
};

use super::config::PermissionsManagerConfig;
use super::token_crud;
use super::types::{PermissionRequest, PermissionType};
use super::WalletPermissionsManager;

// ---------------------------------------------------------------------------
// Stateful mock wallet
// ---------------------------------------------------------------------------

/// A planned set of transaction outputs as `(locking_script, satoshis)` pairs,
/// used to stage the signable transaction the mock wallet returns.
type PlannedOutputs = Vec<(Vec<u8>, u64)>;

#[derive(Clone)]
struct StoredOutput {
    basket: String,
    outpoint: String,
    script: Vec<u8>,
    satoshis: u64,
    tags: Vec<String>,
}

#[derive(Clone)]
struct StoredAction {
    labels: Vec<String>,
    /// Net satoshis for the action (negative for a spend), mirroring the wallet's
    /// `Action.satoshis` convention.
    satoshis: i64,
}

/// A minimal in-memory wallet that actually stores outputs and actions, so the
/// permission flows (token lookup, spend accumulation, PushDrop locking) can be
/// exercised end-to-end. `encrypt`/`decrypt` are the identity transform, which
/// is sufficient for round-tripping token fields in tests.
struct StatefulWallet {
    outputs: Mutex<Vec<StoredOutput>>,
    actions: Mutex<Vec<StoredAction>>,
    key: PrivateKey,
    counter: Mutex<u64>,
    /// When set, `create_action` returns a signable transaction whose Atomic
    /// BEEF encodes exactly these `(locking_script, satoshis)` outputs, standing
    /// in for whatever storage produced. Used to exercise the output-substitution
    /// defense (GHSA-36f9-7rg5-cpf8).
    signable_outputs: Mutex<Option<PlannedOutputs>>,
    /// References passed to `abort_action`, so a test can assert the manager
    /// aborted a tampered action.
    aborted_refs: Mutex<Vec<Vec<u8>>>,
}

impl StatefulWallet {
    fn new() -> Self {
        Self {
            outputs: Mutex::new(Vec::new()),
            actions: Mutex::new(Vec::new()),
            key: PrivateKey::from_random().unwrap(),
            counter: Mutex::new(0),
            signable_outputs: Mutex::new(None),
            aborted_refs: Mutex::new(Vec::new()),
        }
    }

    /// Arrange for the next `create_action` to return a signable transaction
    /// containing `outs` instead of the default txid-only result.
    fn set_signable_outputs(&self, outs: PlannedOutputs) {
        *self.signable_outputs.lock().unwrap() = Some(outs);
    }
}

/// Build Atomic BEEF bytes for a transaction carrying `outs` as its outputs, plus
/// its txid. Mirrors how the signer packages a signable transaction so the
/// permissions-layer parser (`Beef::from_binary` + `into_transaction`) round-trips
/// it exactly as it would a real one.
fn atomic_beef_with_outputs(outs: &[(Vec<u8>, u64)]) -> (Vec<u8>, Vec<u8>) {
    use bsv::transaction::beef::{Beef, BEEF_V1};
    use bsv::transaction::transaction::Transaction;
    use bsv::transaction::transaction_output::TransactionOutput;

    let mut tx = Transaction::new();
    for (script, sats) in outs {
        tx.outputs.push(TransactionOutput {
            satoshis: Some(*sats),
            locking_script: LockingScript::from_binary(script),
            change: false,
        });
    }
    let txid = tx.id().unwrap();
    let mut raw = Vec::new();
    tx.to_binary(&mut raw).unwrap();

    let mut beef = Beef::new(BEEF_V1);
    beef.merge_raw_tx(&raw, None).unwrap();
    let atomic = beef.to_binary_atomic(&txid).unwrap();
    // Reference is opaque to the manager; use the txid bytes as a stable stand-in.
    (atomic, txid.into_bytes())
}

#[async_trait]
impl WalletInterface for StatefulWallet {
    async fn is_authenticated(
        &self,
        _originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        Ok(AuthenticatedResult {
            authenticated: true,
        })
    }

    async fn get_public_key(
        &self,
        _args: GetPublicKeyArgs,
        _originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, bsv::wallet::error::WalletError> {
        Ok(GetPublicKeyResult {
            public_key: self.key.to_public_key(),
        })
    }

    async fn create_signature(
        &self,
        _args: CreateSignatureArgs,
        _originator: Option<&str>,
    ) -> Result<CreateSignatureResult, bsv::wallet::error::WalletError> {
        // Content is irrelevant to PushDrop decoding, which only extracts data
        // pushes; a fixed-length dummy DER-shaped blob suffices.
        Ok(CreateSignatureResult {
            signature: vec![0x30u8; 71],
        })
    }

    async fn encrypt(
        &self,
        args: EncryptArgs,
        _originator: Option<&str>,
    ) -> Result<EncryptResult, bsv::wallet::error::WalletError> {
        Ok(EncryptResult {
            ciphertext: args.plaintext,
        })
    }

    async fn decrypt(
        &self,
        args: DecryptArgs,
        _originator: Option<&str>,
    ) -> Result<DecryptResult, bsv::wallet::error::WalletError> {
        Ok(DecryptResult {
            plaintext: args.ciphertext,
        })
    }

    async fn create_action(
        &self,
        args: CreateActionArgs,
        _originator: Option<&str>,
    ) -> Result<CreateActionResult, bsv::wallet::error::WalletError> {
        let txid = {
            let mut c = self.counter.lock().unwrap();
            *c += 1;
            format!("{:064x}", *c)
        };

        let mut net: i64 = 0;
        {
            let mut outs = self.outputs.lock().unwrap();
            for (i, out) in args.outputs.iter().enumerate() {
                net += out.satoshis as i64;
                if let Some(script) = &out.locking_script {
                    outs.push(StoredOutput {
                        basket: out.basket.clone().unwrap_or_default(),
                        outpoint: format!("{txid}.{i}"),
                        script: script.clone(),
                        satoshis: out.satoshis,
                        tags: out.tags.clone(),
                    });
                }
            }
        }

        // Record the action as a spend: net satoshis is negative for money out,
        // matching the wallet's `Action.satoshis` convention.
        self.actions.lock().unwrap().push(StoredAction {
            labels: args.labels.clone(),
            satoshis: -net,
        });

        // If a signable-tx plan is set, return a signable transaction whose
        // Atomic BEEF encodes the planned outputs (standing in for storage's
        // response), rather than the default txid-only result.
        if let Some(planned) = self.signable_outputs.lock().unwrap().clone() {
            let (tx, reference) = atomic_beef_with_outputs(&planned);
            return Ok(CreateActionResult {
                txid: None,
                tx: None,
                no_send_change: vec![],
                send_with_results: vec![],
                signable_transaction: Some(bsv::wallet::interfaces::SignableTransaction {
                    tx,
                    reference,
                }),
            });
        }

        Ok(CreateActionResult {
            txid: Some(txid),
            tx: None,
            no_send_change: vec![],
            send_with_results: vec![],
            signable_transaction: None,
        })
    }

    async fn list_actions(
        &self,
        args: ListActionsArgs,
        _originator: Option<&str>,
    ) -> Result<ListActionsResult, bsv::wallet::error::WalletError> {
        let actions = self.actions.lock().unwrap();
        let matched: Vec<Action> = actions
            .iter()
            .filter(|a| args.labels.iter().all(|l| a.labels.contains(l)))
            .map(|a| Action {
                txid: String::new(),
                satoshis: a.satoshis,
                status: ActionStatus::Completed,
                is_outgoing: a.satoshis < 0,
                description: String::new(),
                labels: a.labels.clone(),
                version: 0,
                lock_time: 0,
                inputs: vec![],
                outputs: vec![],
            })
            .collect();
        Ok(ListActionsResult {
            total_actions: matched.len() as u32,
            actions: matched,
        })
    }

    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        _originator: Option<&str>,
    ) -> Result<ListOutputsResult, bsv::wallet::error::WalletError> {
        // Filter by basket only (ignore tags), so the decoded-field checks in the
        // lookup — not tag filtering — are what these tests exercise.
        let outs = self.outputs.lock().unwrap();
        let matched: Vec<Output> = outs
            .iter()
            .filter(|o| o.basket == args.basket)
            .map(|o| Output {
                satoshis: o.satoshis,
                locking_script: Some(o.script.clone()),
                spendable: true,
                custom_instructions: None,
                tags: o.tags.clone(),
                outpoint: o.outpoint.clone(),
                labels: vec![],
            })
            .collect();
        Ok(ListOutputsResult {
            total_outputs: matched.len() as u32,
            beef: None,
            outputs: matched,
        })
    }

    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        _originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, bsv::wallet::error::WalletError> {
        let mut outs = self.outputs.lock().unwrap();
        let before = outs.len();
        outs.retain(|o| !(o.basket == args.basket && o.outpoint == args.output));
        Ok(RelinquishOutputResult {
            relinquished: outs.len() < before,
        })
    }

    // --- Unused methods: not needed by these tests ---

    async fn wait_for_authentication(
        &self,
        _originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn get_height(
        &self,
        _originator: Option<&str>,
    ) -> Result<GetHeightResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn get_header_for_height(
        &self,
        _args: GetHeaderArgs,
        _originator: Option<&str>,
    ) -> Result<GetHeaderResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn get_network(
        &self,
        _originator: Option<&str>,
    ) -> Result<GetNetworkResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn get_version(
        &self,
        _originator: Option<&str>,
    ) -> Result<GetVersionResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn sign_action(
        &self,
        _args: SignActionArgs,
        _originator: Option<&str>,
    ) -> Result<SignActionResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn abort_action(
        &self,
        args: AbortActionArgs,
        _originator: Option<&str>,
    ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
        self.aborted_refs.lock().unwrap().push(args.reference);
        Ok(AbortActionResult { aborted: true })
    }
    async fn internalize_action(
        &self,
        _args: InternalizeActionArgs,
        _originator: Option<&str>,
    ) -> Result<InternalizeActionResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn reveal_counterparty_key_linkage(
        &self,
        _args: RevealCounterpartyKeyLinkageArgs,
        _originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn reveal_specific_key_linkage(
        &self,
        _args: RevealSpecificKeyLinkageArgs,
        _originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn create_hmac(
        &self,
        _args: CreateHmacArgs,
        _originator: Option<&str>,
    ) -> Result<CreateHmacResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn verify_hmac(
        &self,
        _args: VerifyHmacArgs,
        _originator: Option<&str>,
    ) -> Result<VerifyHmacResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn verify_signature(
        &self,
        _args: VerifySignatureArgs,
        _originator: Option<&str>,
    ) -> Result<VerifySignatureResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn acquire_certificate(
        &self,
        _args: AcquireCertificateArgs,
        _originator: Option<&str>,
    ) -> Result<Certificate, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn list_certificates(
        &self,
        _args: ListCertificatesArgs,
        _originator: Option<&str>,
    ) -> Result<ListCertificatesResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn prove_certificate(
        &self,
        _args: ProveCertificateArgs,
        _originator: Option<&str>,
    ) -> Result<ProveCertificateResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn relinquish_certificate(
        &self,
        _args: RelinquishCertificateArgs,
        _originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn discover_by_identity_key(
        &self,
        _args: DiscoverByIdentityKeyArgs,
        _originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
    async fn discover_by_attributes(
        &self,
        _args: DiscoverByAttributesArgs,
        _originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ADMIN: &str = "admin.example.com";

fn spend_output(satoshis: u64) -> bsv::wallet::interfaces::CreateActionOutput {
    bsv::wallet::interfaces::CreateActionOutput {
        locking_script: None,
        satoshis,
        output_description: "spend".to_string(),
        basket: None,
        custom_instructions: None,
        tags: vec![],
    }
}

fn spend_args(satoshis: u64) -> CreateActionArgs {
    CreateActionArgs {
        description: "spend".to_string(),
        input_beef: None,
        inputs: vec![],
        outputs: vec![spend_output(satoshis)],
        lock_time: None,
        version: None,
        labels: vec![],
        options: None,
        reference: None,
    }
}

fn dsap_request(originator: &str, amount: u64) -> PermissionRequest {
    PermissionRequest {
        permission_type: PermissionType::SpendingAuthorization,
        originator: originator.to_string(),
        privileged: None,
        protocol: None,
        security_level: None,
        counterparty: None,
        basket_name: None,
        cert_type: None,
        cert_fields: None,
        verifier: None,
        amount: Some(amount),
        description: None,
        labels: None,
        is_new_user: false,
    }
}

fn protocol_request(originator: &str, protocol: &str, privileged: bool) -> PermissionRequest {
    PermissionRequest {
        permission_type: PermissionType::ProtocolPermission,
        originator: originator.to_string(),
        privileged: Some(privileged),
        protocol: Some(protocol.to_string()),
        security_level: Some(1),
        counterparty: Some(String::new()),
        basket_name: None,
        cert_type: None,
        cert_fields: None,
        verifier: None,
        amount: None,
        description: None,
        labels: None,
        is_new_user: false,
    }
}

fn config_no_metadata() -> PermissionsManagerConfig {
    // Skip description metadata encryption so the tests exercise only the
    // permission/spend logic (token-field encryption is separate and stays on).
    PermissionsManagerConfig {
        encrypt_wallet_metadata: false,
        ..PermissionsManagerConfig::default()
    }
}

// ---------------------------------------------------------------------------
// #14 — monthly spend tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_monthly_spend_accumulates_and_rejects_over_limit() {
    let wallet = Arc::new(StatefulWallet::new());
    // Authorize 1000 sats for app.com.
    token_crud::create_permission_token(
        wallet.as_ref(),
        &dsap_request("app.com", 1000),
        None,
        ADMIN,
    )
    .await
    .unwrap();

    let mgr = WalletPermissionsManager::new(wallet, ADMIN.to_string(), config_no_metadata());

    // Distinct amounts avoid the per-amount recent-grant cache short-circuit, so
    // every spend re-queries the accumulated monthly total.
    mgr.create_action(spend_args(300), Some("app.com"))
        .await
        .expect("first spend (300, total 300) is within the 1000 cap");
    mgr.create_action(spend_args(400), Some("app.com"))
        .await
        .expect("second spend (400, total 700) is within the 1000 cap");

    // Third spend pushes the running total to 1200 > 1000 and must be rejected.
    // Before the fix, query_spent_since always returned 0, so this wrongly passed.
    let over = mgr.create_action(spend_args(500), Some("app.com")).await;
    assert!(
        over.is_err(),
        "third spend (500, total 1200) must exceed the 1000 monthly cap"
    );
}

// ---------------------------------------------------------------------------
// #15 — privileged flag honored in token lookup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_privileged_request_not_satisfied_by_nonprivileged_token() {
    let wallet = Arc::new(StatefulWallet::new());
    // Grant a NON-privileged protocol permission for app.com.
    token_crud::create_permission_token(
        wallet.as_ref(),
        &protocol_request("app.com", "test proto", false),
        None,
        ADMIN,
    )
    .await
    .unwrap();

    let mgr = WalletPermissionsManager::new(wallet, ADMIN.to_string(), config_no_metadata());

    // Control: a NON-privileged request is satisfied by the non-privileged token.
    super::ensure::ensure_protocol_permission(
        &mgr,
        "app.com",
        "test proto",
        1,
        "",
        false,
        "signing",
    )
    .await
    .expect("non-privileged request should be satisfied by the non-privileged token");

    // A PRIVILEGED request must NOT be satisfied by the non-privileged token.
    // Before the fix, the lookup ignored the privileged flag and wrongly granted.
    let privileged = super::ensure::ensure_protocol_permission(
        &mgr,
        "app.com",
        "test proto",
        1,
        "",
        true,
        "signing",
    )
    .await;
    assert!(
        privileged.is_err(),
        "privileged request must not be satisfied by a non-privileged token"
    );
}

// ---------------------------------------------------------------------------
// #16 — permission tokens are real PushDrop scripts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_permission_token_is_valid_pushdrop_roundtrip() {
    let wallet = Arc::new(StatefulWallet::new());
    token_crud::create_permission_token(
        wallet.as_ref(),
        &protocol_request("app.com", "test proto", false),
        None,
        ADMIN,
    )
    .await
    .unwrap();

    let tokens = token_crud::list_protocol_permissions(wallet.as_ref(), "app.com", ADMIN)
        .await
        .unwrap();
    assert_eq!(tokens.len(), 1, "exactly one protocol token expected");

    let token = &tokens[0];
    assert_eq!(token.protocol.as_deref(), Some("test proto"));
    assert_eq!(token.privileged, Some(false));
    assert_eq!(token.originator, "app.com");

    // The stored locking script must decode as a real PushDrop template. Before
    // the fix it was a length-prefixed blob, which is not a valid script.
    let script = LockingScript::from_binary(&token.output_script);
    let decoded = decode_push_drop(&script)
        .expect("permission token locking script must be a valid PushDrop");
    // First data field is the (identity-encrypted) originator domain.
    assert_eq!(decoded.fields[0], b"app.com".to_vec());
}

// ---------------------------------------------------------------------------
// #17 — output-substitution defense (GHSA-36f9-7rg5-cpf8)
// ---------------------------------------------------------------------------

/// A canonical P2PKH locking script `OP_DUP OP_HASH160 <20-byte hash> OP_EQUALVERIFY
/// OP_CHECKSIG`, distinguished by `tag` in the pubkey-hash. Canonical form so it
/// round-trips byte-for-byte through Script parse/serialize (as a real caller
/// script and the storage-echoed script both would).
fn p2pkh_script(tag: u8) -> Vec<u8> {
    let mut s = vec![0x76, 0xa9, 0x14];
    s.extend_from_slice(&[tag; 20]);
    s.extend_from_slice(&[0x88, 0xac]);
    s
}

/// A caller-requested output carrying an explicit locking script (unlike
/// `spend_output`, which leaves it unset).
fn output_with_script(
    script: Vec<u8>,
    satoshis: u64,
) -> bsv::wallet::interfaces::CreateActionOutput {
    bsv::wallet::interfaces::CreateActionOutput {
        locking_script: Some(script),
        satoshis,
        output_description: "pay".to_string(),
        basket: None,
        custom_instructions: None,
        tags: vec![],
    }
}

fn args_with_outputs(
    outputs: Vec<bsv::wallet::interfaces::CreateActionOutput>,
) -> CreateActionArgs {
    CreateActionArgs {
        description: "pay recipients".to_string(),
        input_beef: None,
        inputs: vec![],
        outputs,
        lock_time: None,
        version: None,
        labels: vec![],
        options: None,
        reference: None,
    }
}

/// A substituted recipient script in the signable transaction must be detected:
/// the manager verifies the returned signable tx contains every caller-requested
/// output, aborts the action, and errors. Before this fix the inner result passed
/// through undetected (wave-1 baseline), so the substituted output would be signed.
///
/// Runs as ADMIN so spending-authorization is bypassed and the test isolates the
/// output-presence check itself.
#[tokio::test]
async fn test_create_action_detects_substituted_output_and_aborts() {
    let wallet = Arc::new(StatefulWallet::new());

    let recipient = p2pkh_script(0x11);
    // Storage returns a signable tx where the recipient script is swapped for an
    // attacker's, keeping the amount identical — plus a legitimate change output.
    let attacker = p2pkh_script(0x99);
    wallet.set_signable_outputs(vec![(attacker, 100), (p2pkh_script(0xcc), 40)]);

    let mgr =
        WalletPermissionsManager::new(wallet.clone(), ADMIN.to_string(), config_no_metadata());

    let args = args_with_outputs(vec![output_with_script(recipient, 100)]);
    let result = mgr.create_action(args, Some(ADMIN)).await;

    assert!(
        result.is_err(),
        "a substituted recipient script must be detected and rejected"
    );
    // The pending, tampered action must have been aborted.
    assert_eq!(
        wallet.aborted_refs.lock().unwrap().len(),
        1,
        "the tampered action must be aborted exactly once"
    );
}

/// A dropped caller-requested output (present in the request, absent from the
/// signable tx) must likewise be detected and aborted.
#[tokio::test]
async fn test_create_action_detects_dropped_output_and_aborts() {
    let wallet = Arc::new(StatefulWallet::new());

    let a = p2pkh_script(0x11);
    let b = p2pkh_script(0x22);
    // Storage drops the second requested output, returning only the first + change.
    wallet.set_signable_outputs(vec![(a.clone(), 100), (p2pkh_script(0xcc), 40)]);

    let mgr =
        WalletPermissionsManager::new(wallet.clone(), ADMIN.to_string(), config_no_metadata());

    let args = args_with_outputs(vec![output_with_script(a, 100), output_with_script(b, 200)]);
    let result = mgr.create_action(args, Some(ADMIN)).await;

    assert!(
        result.is_err(),
        "a dropped requested output must be detected"
    );
    assert_eq!(
        wallet.aborted_refs.lock().unwrap().len(),
        1,
        "the tampered action must be aborted exactly once"
    );
}

/// Control: when every caller-requested output is present in the signable tx
/// (alongside change/commission outputs, in randomized order), create_action
/// succeeds and returns the signable transaction without aborting.
#[tokio::test]
async fn test_create_action_passes_when_all_requested_outputs_present() {
    let wallet = Arc::new(StatefulWallet::new());

    let a = p2pkh_script(0x11);
    let b = p2pkh_script(0x22);
    let change = p2pkh_script(0xcc);
    // All requested outputs present; extra change output and reordered to mimic
    // the default output randomization.
    wallet.set_signable_outputs(vec![(change, 40), (b.clone(), 200), (a.clone(), 100)]);

    let mgr =
        WalletPermissionsManager::new(wallet.clone(), ADMIN.to_string(), config_no_metadata());

    let args = args_with_outputs(vec![output_with_script(a, 100), output_with_script(b, 200)]);
    let result = mgr
        .create_action(args, Some(ADMIN))
        .await
        .expect("all requested outputs present -> create_action succeeds");

    assert!(
        result.signable_transaction.is_some(),
        "the signable transaction should be returned to the caller"
    );
    assert!(
        wallet.aborted_refs.lock().unwrap().is_empty(),
        "a valid action must not be aborted"
    );
}
