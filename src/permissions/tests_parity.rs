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
}

impl StatefulWallet {
    fn new() -> Self {
        Self {
            outputs: Mutex::new(Vec::new()),
            actions: Mutex::new(Vec::new()),
            key: PrivateKey::from_random().unwrap(),
            counter: Mutex::new(0),
        }
    }
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
        _args: AbortActionArgs,
        _originator: Option<&str>,
    ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
        Err(bsv::wallet::error::WalletError::NotImplemented("".into()))
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
