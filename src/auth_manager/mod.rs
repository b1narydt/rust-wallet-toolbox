//! Authentication manager for WAB-based wallet authentication.
//!
//! Provides `WalletAuthenticationManager` which wraps an inner wallet with
//! WAB-based authentication flow. Before authentication completes, all
//! WalletInterface methods (except `is_authenticated` and `wait_for_authentication`)
//! return `WERR_NOT_ACTIVE`. After successful auth, calls delegate to the inner wallet.
//!
//! Composes CWI base class logic (UMP tokens, PBKDF2 key derivation, password +
//! presentation key management) and R-Puzzle faucet funding.

/// CWI-style key derivation functions (PBKDF2, XOR, identity key derivation).
pub mod cwi_logic;
/// Authentication state types, snapshot, and profile.
pub mod types;
/// UMP token struct and overlay lookup/creation.
pub mod ump_token;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

use bsv::primitives::big_number::BigNumber;
use bsv::primitives::private_key::PrivateKey;
use bsv::script::templates::r_puzzle::{RPuzzle, RPuzzleType};
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
    VerifyHmacArgs, VerifyHmacResult, VerifySignatureArgs, VerifySignatureResult, WalletInterface,
};

use crate::wab_client::interactors::AuthMethodInteractor;
use crate::wab_client::types::{FaucetResponse, StartAuthResponse};
use crate::wab_client::WABClient;
use crate::WalletError;

use types::{AuthState, CompleteAuthOutcome, SecondFactor, StateSnapshot, WalletBuilderFn};
use ump_token::{AuthFactors, UMPToken};

/// Macro to delegate a WalletInterface method to the inner wallet, or return
/// `WERR_NOT_ACTIVE` if no inner wallet is available (pre-authentication).
macro_rules! delegate_or_not_active {
    ($self:ident, $method:ident, $args:expr, $originator:expr) => {{
        let guard = $self.inner.read().await;
        match guard.as_ref() {
            Some(w) => w.$method($args, $originator).await,
            None => Err(bsv::wallet::error::WalletError::Internal(
                "WERR_NOT_ACTIVE: Wallet is not authenticated".into(),
            )),
        }
    }};
    // Variant without args (for get_height, get_network, get_version)
    ($self:ident, $method:ident, $originator:expr) => {{
        let guard = $self.inner.read().await;
        match guard.as_ref() {
            Some(w) => w.$method($originator).await,
            None => Err(bsv::wallet::error::WalletError::Internal(
                "WERR_NOT_ACTIVE: Wallet is not authenticated".into(),
            )),
        }
    }};
}

/// WalletAuthenticationManager wraps a wallet with WAB-based authentication.
///
/// Before authentication completes, all WalletInterface methods except
/// `is_authenticated` and `wait_for_authentication` return `WERR_NOT_ACTIVE`.
/// After authentication, all calls are delegated to the inner wallet.
///
/// The authentication flow:
/// 1. Application calls `start_auth()` with an auth method interactor and payload
/// 2. WAB server initiates verification (e.g., sends SMS code)
/// 3. Application calls `complete_auth()` with verification payload
/// 4. Manager determines new vs existing user via UMP token lookup
/// 5. Root key derived from presentation key + password logic
/// 6. Inner wallet constructed via `wallet_builder`
/// 7. For new users, optional faucet funding via R-Puzzle redemption
/// 8. `wait_for_authentication` resolvers are notified
pub struct WalletAuthenticationManager {
    /// The inner wallet; `None` before authentication, `Some` after.
    inner: RwLock<Option<Arc<dyn WalletInterface + Send + Sync>>>,
    /// Sender to signal authentication completion.
    auth_tx: tokio::sync::watch::Sender<bool>,
    /// Receiver to wait for authentication completion.
    auth_rx: tokio::sync::watch::Receiver<bool>,
    /// WAB HTTP client for authentication API calls.
    wab_client: WABClient,
    /// Async closure to construct the inner wallet from root key material.
    wallet_builder: WalletBuilderFn,
    /// Admin originator domain for privileged operations.
    admin_originator: String,
    /// Current authentication state.
    state: Mutex<AuthState>,
    /// Generated or restored presentation key (hex).
    presentation_key: Mutex<Option<String>>,
}

impl WalletAuthenticationManager {
    /// Create a new `WalletAuthenticationManager`.
    ///
    /// # Arguments
    /// * `wab_client` - WAB HTTP client for auth API calls
    /// * `wallet_builder` - Async closure to construct inner wallet from root key
    /// * `admin_originator` - Admin domain for privileged operations
    /// * `state_snapshot` - Optional serialized `StateSnapshot` to restore state
    pub fn new(
        wab_client: WABClient,
        wallet_builder: WalletBuilderFn,
        admin_originator: String,
        state_snapshot: Option<Vec<u8>>,
    ) -> Result<Self, WalletError> {
        let (auth_tx, auth_rx) = tokio::sync::watch::channel(false);

        let (initial_state, initial_key) = if let Some(snapshot_bytes) = state_snapshot {
            let snapshot: StateSnapshot = serde_json::from_slice(&snapshot_bytes)?;
            let key = snapshot.presentation_key.clone();
            (snapshot.auth_state, key)
        } else {
            (AuthState::Unauthenticated, None)
        };

        Ok(Self {
            inner: RwLock::new(None),
            auth_tx,
            auth_rx,
            wab_client,
            wallet_builder,
            admin_originator,
            state: Mutex::new(initial_state),
            presentation_key: Mutex::new(initial_key),
        })
    }

    // ========================================================================
    // Auth flow methods (not part of WalletInterface)
    // ========================================================================

    /// Start the WAB authentication flow.
    ///
    /// Generates a temporary presentation key if none is set, then calls
    /// `start_auth_method` on the WAB client with the chosen interactor's method type.
    ///
    /// # Arguments
    /// * `interactor` - The auth method interactor (e.g., Twilio, Persona)
    /// * `payload` - Method-specific payload (e.g., phone number for Twilio)
    pub async fn start_auth(
        &self,
        interactor: &dyn AuthMethodInteractor,
        payload: serde_json::Value,
    ) -> Result<StartAuthResponse, WalletError> {
        // Generate presentation key if not already set
        {
            let mut pk = self.presentation_key.lock().await;
            if pk.is_none() {
                *pk = Some(WABClient::generate_random_presentation_key()?);
            }
        }

        let pk = self.presentation_key.lock().await;
        let presentation_key = pk
            .as_ref()
            .ok_or_else(|| WalletError::Internal("Presentation key missing".to_string()))?;

        // Update state to Authenticating
        {
            let mut state = self.state.lock().await;
            *state = AuthState::Authenticating;
        }

        let method_type = interactor.method_type();
        self.wab_client
            .start_auth_method(presentation_key, method_type, payload)
            .await
    }

    /// Complete the WAB authentication flow.
    ///
    /// On success:
    /// 1. Retrieves final presentation key from WAB (the WAB never sees the
    ///    password or recovery key — it only ever holds the presentation-key
    ///    factor, so it can never reconstruct a wallet key on its own).
    /// 2. Resolves the `rootPrimaryKey` via the real UMP 2-of-3 threshold
    ///    scheme: for a new user (no `existing_ump_token`), builds a fresh
    ///    `UMPToken` from presentation + password + a freshly generated
    ///    recovery key; for a returning user, decrypts the existing token
    ///    using presentation + password (or presentation + recovery).
    /// 3. Constructs inner wallet via `wallet_builder` using the real root key.
    /// 4. For new users, publishes the freshly built `UMPToken` on-chain.
    /// 5. Signals authentication completion.
    ///
    /// Discovering `existing_ump_token` (looking up the on-chain token by
    /// `presentationHash`) is the caller's responsibility — it requires a
    /// public overlay lookup that is independent of any specific wallet's
    /// local storage (see [`ump_token::lookup_ump_token`] and the module
    /// docs there). Pass `None` for first-time signup.
    ///
    /// # Arguments
    /// * `interactor` - The auth method interactor
    /// * `payload` - Verification payload (e.g., SMS code)
    /// * `second_factor` - The password (new signup or returning login) or
    ///   recovery key the caller supplies, alongside the WAB-issued
    ///   presentation key
    /// * `existing_ump_token` - The on-chain `UMPToken` for this user, if
    ///   the caller already looked one up; `None` for new-user signup
    pub async fn complete_auth(
        &self,
        interactor: &dyn AuthMethodInteractor,
        payload: serde_json::Value,
        second_factor: SecondFactor,
        existing_ump_token: Option<UMPToken>,
    ) -> Result<CompleteAuthOutcome, WalletError> {
        let temp_key = {
            let mut pk = self.presentation_key.lock().await;
            pk.take().ok_or_else(|| {
                WalletError::Internal("No presentation key; call start_auth first".to_string())
            })?
        };

        let method_type = interactor.method_type();
        let result = self
            .wab_client
            .complete_auth_method(&temp_key, method_type, payload)
            .await?;

        if !result.success {
            let msg = result
                .message
                .unwrap_or_else(|| "Failed to complete WAB auth".to_string());
            let mut state = self.state.lock().await;
            *state = AuthState::Failed(msg.clone());
            return Err(WalletError::Internal(msg));
        }

        let presentation_key_hex = result.presentation_key.ok_or_else(|| {
            WalletError::Internal("No presentation key returned from WAB".to_string())
        })?;

        // Store the final presentation key
        {
            let mut pk = self.presentation_key.lock().await;
            *pk = Some(presentation_key_hex.clone());
        }

        let presentation_key_bytes = hex_to_bytes(&presentation_key_hex)?;

        // Resolve the real rootPrimaryKey via the UMP 2-of-3 threshold
        // scheme (build for new users, unlock via 2 factors for returning
        // users) -- this replaces the old placeholder that used the
        // presentation key bytes directly as the root key.
        let resolved = resolve_root_key(
            &presentation_key_bytes,
            &second_factor,
            existing_ump_token.as_ref(),
        )?;

        // Build a placeholder PrivilegedKeyManager from root key
        let priv_key_manager = Arc::new(crate::wallet::privileged::NoOpPrivilegedKeyManager);

        // Construct inner wallet using the real root key
        let wallet = (self.wallet_builder)(resolved.root_key, priv_key_manager).await?;

        // For new users, publish the freshly built UMP token on-chain now
        // that we have a wallet capable of create_action.
        if let Some(ref token) = resolved.token_to_publish {
            ump_token::create_ump_token(wallet.as_ref(), token, &self.admin_originator).await?;
        }

        // Store inner wallet
        {
            let mut inner = self.inner.write().await;
            *inner = Some(wallet);
        }

        // Update state
        {
            let mut state = self.state.lock().await;
            *state = AuthState::Authenticated;
        }

        // Signal authentication completion
        let _ = self.auth_tx.send(true);

        Ok(CompleteAuthOutcome {
            is_new_user: resolved.is_new_user,
            generated_recovery_key: resolved.generated_recovery_key,
        })
    }

    /// Serialize current state to a JSON byte vector for persistence.
    ///
    /// The returned bytes can be passed to `new()` as `state_snapshot` to
    /// restore manager state across process restarts.
    pub async fn get_state_snapshot(&self) -> Result<Vec<u8>, WalletError> {
        let state = self.state.lock().await.clone();
        let pk = self.presentation_key.lock().await.clone();
        let snapshot = StateSnapshot {
            presentation_key: pk,
            auth_state: state,
            profile: None,
            is_new_user: None,
        };
        serde_json::to_vec(&snapshot)
            .map_err(|e| WalletError::Internal(format!("Failed to serialize state snapshot: {e}")))
    }

    // ========================================================================
    // R-Puzzle faucet redemption
    // ========================================================================

    /// Redeem a faucet UTXO using R-Puzzle.
    ///
    /// Ports the TS WalletAuthenticationManager faucet logic:
    /// 1. Creates an action with the faucet UTXO as input (unlockingScriptLength 108)
    /// 2. Signs with RPuzzle::from_k() using the k-value from the faucet response
    /// 3. Completes signing via wallet.sign_action()
    ///
    /// # Arguments
    /// * `wallet` - The inner wallet to use for create_action/sign_action
    /// * `faucet_response` - Response from WABClient::request_faucet containing tx, txid, and k
    #[allow(dead_code)]
    async fn redeem_faucet(
        &self,
        wallet: &(dyn WalletInterface + Send + Sync),
        faucet_response: &FaucetResponse,
    ) -> Result<(), WalletError> {
        let k_hex = faucet_response
            .k
            .as_ref()
            .ok_or_else(|| WalletError::Internal("Faucet response missing k value".to_string()))?;
        let txid = faucet_response
            .txid
            .as_ref()
            .ok_or_else(|| WalletError::Internal("Faucet response missing txid".to_string()))?;
        let tx_hex = faucet_response
            .tx
            .as_ref()
            .ok_or_else(|| WalletError::Internal("Faucet response missing tx data".to_string()))?;

        // Parse the faucet transaction BEEF
        let tx_bytes = hex_to_bytes(tx_hex)?;

        // Create action with faucet UTXO as input
        let create_args = CreateActionArgs {
            input_beef: Some(tx_bytes),
            inputs: vec![bsv::wallet::interfaces::CreateActionInput {
                outpoint: format!("{txid}.0"),
                unlocking_script_length: Some(108),
                input_description: "Fund from faucet".to_string(),
                unlocking_script: None,
                sequence_number: None,
            }],
            description: "Fund wallet".to_string(),
            outputs: vec![],
            labels: vec![],
            lock_time: None,
            version: None,
            options: Some(bsv::wallet::interfaces::CreateActionOptions {
                accept_delayed_broadcast: bsv::wallet::types::BooleanDefaultTrue(Some(false)),
                ..Default::default()
            }),
            reference: None,
        };

        let create_result = wallet
            .create_action(create_args, Some(&self.admin_originator))
            .await
            .map_err(|e| WalletError::Internal(format!("Faucet create_action failed: {e}")))?;

        let signable = create_result.signable_transaction.ok_or_else(|| {
            WalletError::Internal("No signable transaction from faucet create_action".to_string())
        })?;

        // Parse k-value for RPuzzle
        let k = BigNumber::from_hex(k_hex)
            .map_err(|e| WalletError::Internal(format!("Failed to parse faucet k value: {e}")))?;
        let random_key = PrivateKey::from_random().map_err(|e| {
            WalletError::Internal(format!("Failed to generate random key for RPuzzle: {e}"))
        })?;

        // Create RPuzzle unlocking template with k value
        // The puzzle type is Raw since faucet uses raw R-value matching
        let puzzle = RPuzzle::from_k(RPuzzleType::Raw, vec![], k, random_key);

        // Convert signable.tx bytes to hex for Transaction::from_beef
        let tx_hex_str: String = signable.tx.iter().map(|b| format!("{b:02x}")).collect();

        // Parse the signable transaction BEEF
        let tx = bsv::transaction::Transaction::from_beef(&tx_hex_str).map_err(|e| {
            WalletError::Internal(format!("Failed to parse signable transaction BEEF: {e}"))
        })?;

        // Get the source output's locking script and satoshis from the transaction's
        // source_transaction on input 0 (set by the wallet during create_action)
        let (source_sats, source_script) = tx
            .inputs
            .first()
            .and_then(|inp| {
                inp.source_transaction.as_ref().map(|stx| {
                    let vout = inp.source_output_index as usize;
                    let output = &stx.outputs[vout];
                    (output.satoshis, output.locking_script.clone())
                })
            })
            .ok_or_else(|| {
                WalletError::Internal(
                    "Faucet signable tx missing source transaction on input 0".to_string(),
                )
            })?;

        // Generate the sighash preimage for input 0
        let preimage = tx
            .sighash_preimage(
                0,
                bsv::primitives::transaction_signature::SIGHASH_ALL
                    | bsv::primitives::transaction_signature::SIGHASH_FORKID,
                source_sats.unwrap_or(0),
                &source_script,
            )
            .map_err(|e| {
                WalletError::Internal(format!("Failed to compute sighash preimage: {e}"))
            })?;

        // Unlock with RPuzzle (produces DER signature + sighash byte)
        let unlocking_script = puzzle
            .unlock(&preimage)
            .map_err(|e| WalletError::Internal(format!("RPuzzle unlock failed: {e}")))?;

        // Sign the action with the RPuzzle unlocking script (binary bytes)
        let mut spends = std::collections::HashMap::new();
        spends.insert(
            0u32,
            bsv::wallet::interfaces::SignActionSpend {
                unlocking_script: unlocking_script.to_binary(),
                sequence_number: None,
            },
        );

        let _sign_result = wallet
            .sign_action(
                SignActionArgs {
                    reference: signable.reference,
                    spends,
                    options: None,
                },
                Some(&self.admin_originator),
            )
            .await
            .map_err(|e| WalletError::Internal(format!("Faucet sign_action failed: {e}")))?;

        Ok(())
    }
}

/// Outcome of resolving the real UMP `rootPrimaryKey` from the presentation
/// key plus a caller-supplied second factor, against an optional existing
/// on-chain `UMPToken`.
struct ResolvedAuth {
    /// The wallet's real root key (never the presentation key bytes).
    root_key: Vec<u8>,
    /// Whether this resolution created a brand-new `UMPToken`.
    is_new_user: bool,
    /// The freshly generated recovery key, only for new users.
    generated_recovery_key: Option<Vec<u8>>,
    /// The freshly built `UMPToken` to publish on-chain, only for new users.
    token_to_publish: Option<UMPToken>,
}

/// Core UMP 2-of-3 decision logic for `complete_auth`, factored out as a
/// synchronous pure function (no WAB/network/wallet dependency) so it can
/// be unit tested directly.
///
/// - `existing_ump_token = None` + `SecondFactor::NewUserPassword` builds a
///   fresh `UMPToken` (presentation + chosen password + a freshly generated
///   recovery key) and returns the generated recovery key for the caller to
///   persist/display exactly once.
/// - `existing_ump_token = Some(token)` + `SecondFactor::Password` or
///   `SecondFactor::Recovery` decrypts the existing token's `rootPrimaryKey`
///   via the matching 2-of-3 combination.
/// - Any other pairing (e.g. a token already exists but the caller passed
///   `NewUserPassword`, or no token exists but the caller passed `Recovery`)
///   is a caller error.
fn resolve_root_key(
    presentation_key: &[u8],
    second_factor: &SecondFactor,
    existing_ump_token: Option<&UMPToken>,
) -> Result<ResolvedAuth, WalletError> {
    match (existing_ump_token, second_factor) {
        (None, SecondFactor::NewUserPassword(password)) => {
            let recovery_key = bsv::primitives::random::random_bytes(32);
            let (token, root_key) =
                ump_token::build_ump_token(password.as_bytes(), presentation_key, &recovery_key)?;
            Ok(ResolvedAuth {
                root_key,
                is_new_user: true,
                generated_recovery_key: Some(recovery_key),
                token_to_publish: Some(token),
            })
        }
        (None, SecondFactor::Password(_)) | (None, SecondFactor::Recovery(_)) => {
            Err(WalletError::Internal(
                "No UMP token found for this presentation key; first-time signup requires \
                 SecondFactor::NewUserPassword"
                    .to_string(),
            ))
        }
        (Some(_), SecondFactor::NewUserPassword(_)) => Err(WalletError::Internal(
            "A UMP token already exists for this presentation key; use SecondFactor::Password \
             or SecondFactor::Recovery to log in"
                .to_string(),
        )),
        (Some(token), SecondFactor::Password(password)) => {
            let root_key = ump_token::unlock_root_primary_key(
                token,
                &AuthFactors::PresentationPassword {
                    presentation_key: presentation_key.to_vec(),
                    password: password.as_bytes().to_vec(),
                },
            )?;
            Ok(ResolvedAuth {
                root_key,
                is_new_user: false,
                generated_recovery_key: None,
                token_to_publish: None,
            })
        }
        (Some(token), SecondFactor::Recovery(recovery_key)) => {
            let root_key = ump_token::unlock_root_primary_key(
                token,
                &AuthFactors::PresentationRecovery {
                    presentation_key: presentation_key.to_vec(),
                    recovery_key: recovery_key.clone(),
                },
            )?;
            Ok(ResolvedAuth {
                root_key,
                is_new_user: false,
                generated_recovery_key: None,
                token_to_publish: None,
            })
        }
    }
}

/// Convert a hex string to bytes.
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, WalletError> {
    if !hex.len().is_multiple_of(2) {
        return Err(WalletError::Internal(format!(
            "Invalid hex string length: {}",
            hex.len()
        )));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| WalletError::Internal(format!("Invalid hex at position {i}: {e}")))
        })
        .collect()
}

// ============================================================================
// WalletInterface implementation
// ============================================================================

#[async_trait]
impl WalletInterface for WalletAuthenticationManager {
    async fn is_authenticated(
        &self,
        _originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        let guard = self.inner.read().await;
        Ok(AuthenticatedResult {
            authenticated: guard.is_some(),
        })
    }

    async fn wait_for_authentication(
        &self,
        _originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        let mut rx = self.auth_rx.clone();
        // If already authenticated, return immediately
        if *rx.borrow() {
            return Ok(AuthenticatedResult {
                authenticated: true,
            });
        }
        // Wait for auth signal
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return Ok(AuthenticatedResult {
                    authenticated: true,
                });
            }
        }
        // Channel closed without auth -- should not happen in normal flow
        Ok(AuthenticatedResult {
            authenticated: false,
        })
    }

    async fn get_height(
        &self,
        originator: Option<&str>,
    ) -> Result<GetHeightResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, get_height, originator)
    }

    async fn get_header_for_height(
        &self,
        args: GetHeaderArgs,
        originator: Option<&str>,
    ) -> Result<GetHeaderResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, get_header_for_height, args, originator)
    }

    async fn get_network(
        &self,
        originator: Option<&str>,
    ) -> Result<GetNetworkResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, get_network, originator)
    }

    async fn get_version(
        &self,
        originator: Option<&str>,
    ) -> Result<GetVersionResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, get_version, originator)
    }

    async fn create_action(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
    ) -> Result<CreateActionResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, create_action, args, originator)
    }

    async fn sign_action(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
    ) -> Result<SignActionResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, sign_action, args, originator)
    }

    async fn abort_action(
        &self,
        args: AbortActionArgs,
        originator: Option<&str>,
    ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, abort_action, args, originator)
    }

    async fn list_actions(
        &self,
        args: ListActionsArgs,
        originator: Option<&str>,
    ) -> Result<ListActionsResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, list_actions, args, originator)
    }

    async fn internalize_action(
        &self,
        args: InternalizeActionArgs,
        originator: Option<&str>,
    ) -> Result<InternalizeActionResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, internalize_action, args, originator)
    }

    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        originator: Option<&str>,
    ) -> Result<ListOutputsResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, list_outputs, args, originator)
    }

    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, relinquish_output, args, originator)
    }

    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
        originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, get_public_key, args, originator)
    }

    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, reveal_counterparty_key_linkage, args, originator)
    }

    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, reveal_specific_key_linkage, args, originator)
    }

    async fn encrypt(
        &self,
        args: EncryptArgs,
        originator: Option<&str>,
    ) -> Result<EncryptResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, encrypt, args, originator)
    }

    async fn decrypt(
        &self,
        args: DecryptArgs,
        originator: Option<&str>,
    ) -> Result<DecryptResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, decrypt, args, originator)
    }

    async fn create_hmac(
        &self,
        args: CreateHmacArgs,
        originator: Option<&str>,
    ) -> Result<CreateHmacResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, create_hmac, args, originator)
    }

    async fn verify_hmac(
        &self,
        args: VerifyHmacArgs,
        originator: Option<&str>,
    ) -> Result<VerifyHmacResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, verify_hmac, args, originator)
    }

    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
        originator: Option<&str>,
    ) -> Result<CreateSignatureResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, create_signature, args, originator)
    }

    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
        originator: Option<&str>,
    ) -> Result<VerifySignatureResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, verify_signature, args, originator)
    }

    async fn acquire_certificate(
        &self,
        args: AcquireCertificateArgs,
        originator: Option<&str>,
    ) -> Result<Certificate, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, acquire_certificate, args, originator)
    }

    async fn list_certificates(
        &self,
        args: ListCertificatesArgs,
        originator: Option<&str>,
    ) -> Result<ListCertificatesResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, list_certificates, args, originator)
    }

    async fn prove_certificate(
        &self,
        args: ProveCertificateArgs,
        originator: Option<&str>,
    ) -> Result<ProveCertificateResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, prove_certificate, args, originator)
    }

    async fn relinquish_certificate(
        &self,
        args: RelinquishCertificateArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, relinquish_certificate, args, originator)
    }

    async fn discover_by_identity_key(
        &self,
        args: DiscoverByIdentityKeyArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, discover_by_identity_key, args, originator)
    }

    async fn discover_by_attributes(
        &self,
        args: DiscoverByAttributesArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        delegate_or_not_active!(self, discover_by_attributes, args, originator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple mock wallet for auth manager testing.
    /// `is_authenticated` returns true; all other methods return NotImplemented.
    struct MockWallet;

    #[async_trait]
    impl WalletInterface for MockWallet {
        async fn is_authenticated(
            &self,
            _originator: Option<&str>,
        ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
            Ok(AuthenticatedResult {
                authenticated: true,
            })
        }

        async fn wait_for_authentication(
            &self,
            _originator: Option<&str>,
        ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
            Ok(AuthenticatedResult {
                authenticated: true,
            })
        }

        async fn get_height(
            &self,
            _o: Option<&str>,
        ) -> Result<GetHeightResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn get_header_for_height(
            &self,
            _a: GetHeaderArgs,
            _o: Option<&str>,
        ) -> Result<GetHeaderResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn get_network(
            &self,
            _o: Option<&str>,
        ) -> Result<GetNetworkResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn get_version(
            &self,
            _o: Option<&str>,
        ) -> Result<GetVersionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn create_action(
            &self,
            _a: CreateActionArgs,
            _o: Option<&str>,
        ) -> Result<CreateActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn sign_action(
            &self,
            _a: SignActionArgs,
            _o: Option<&str>,
        ) -> Result<SignActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn abort_action(
            &self,
            _a: AbortActionArgs,
            _o: Option<&str>,
        ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn list_actions(
            &self,
            _a: ListActionsArgs,
            _o: Option<&str>,
        ) -> Result<ListActionsResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn internalize_action(
            &self,
            _a: InternalizeActionArgs,
            _o: Option<&str>,
        ) -> Result<InternalizeActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn list_outputs(
            &self,
            _a: ListOutputsArgs,
            _o: Option<&str>,
        ) -> Result<ListOutputsResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn relinquish_output(
            &self,
            _a: RelinquishOutputArgs,
            _o: Option<&str>,
        ) -> Result<RelinquishOutputResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn get_public_key(
            &self,
            _a: GetPublicKeyArgs,
            _o: Option<&str>,
        ) -> Result<GetPublicKeyResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn reveal_counterparty_key_linkage(
            &self,
            _a: RevealCounterpartyKeyLinkageArgs,
            _o: Option<&str>,
        ) -> Result<RevealCounterpartyKeyLinkageResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn reveal_specific_key_linkage(
            &self,
            _a: RevealSpecificKeyLinkageArgs,
            _o: Option<&str>,
        ) -> Result<RevealSpecificKeyLinkageResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn encrypt(
            &self,
            _a: EncryptArgs,
            _o: Option<&str>,
        ) -> Result<EncryptResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn decrypt(
            &self,
            _a: DecryptArgs,
            _o: Option<&str>,
        ) -> Result<DecryptResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn create_hmac(
            &self,
            _a: CreateHmacArgs,
            _o: Option<&str>,
        ) -> Result<CreateHmacResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn verify_hmac(
            &self,
            _a: VerifyHmacArgs,
            _o: Option<&str>,
        ) -> Result<VerifyHmacResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn create_signature(
            &self,
            _a: CreateSignatureArgs,
            _o: Option<&str>,
        ) -> Result<CreateSignatureResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn verify_signature(
            &self,
            _a: VerifySignatureArgs,
            _o: Option<&str>,
        ) -> Result<VerifySignatureResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn acquire_certificate(
            &self,
            _a: AcquireCertificateArgs,
            _o: Option<&str>,
        ) -> Result<Certificate, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn list_certificates(
            &self,
            _a: ListCertificatesArgs,
            _o: Option<&str>,
        ) -> Result<ListCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn prove_certificate(
            &self,
            _a: ProveCertificateArgs,
            _o: Option<&str>,
        ) -> Result<ProveCertificateResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn relinquish_certificate(
            &self,
            _a: RelinquishCertificateArgs,
            _o: Option<&str>,
        ) -> Result<RelinquishCertificateResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn discover_by_identity_key(
            &self,
            _a: DiscoverByIdentityKeyArgs,
            _o: Option<&str>,
        ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
        async fn discover_by_attributes(
            &self,
            _a: DiscoverByAttributesArgs,
            _o: Option<&str>,
        ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "mock".into(),
            ))
        }
    }

    fn make_builder() -> WalletBuilderFn {
        Box::new(|_root_key, _pkm| {
            Box::pin(async move {
                let mock: Arc<dyn WalletInterface + Send + Sync> = Arc::new(MockWallet);
                Ok(mock)
            })
        })
    }

    #[tokio::test]
    async fn test_auth_manager_not_active_before_auth() {
        let wab = WABClient::new("http://localhost:3000");
        let mgr = WalletAuthenticationManager::new(
            wab,
            make_builder(),
            "admin.example.com".to_string(),
            None,
        )
        .unwrap();

        // create_action should return NotActive before authentication
        let result = mgr
            .create_action(
                CreateActionArgs {
                    description: "test action".to_string(),
                    inputs: vec![],
                    outputs: vec![],
                    labels: vec![],
                    lock_time: None,
                    version: None,
                    options: None,
                    input_beef: None,
                    reference: None,
                },
                Some("test.com"),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("not authenticated") || err_str.contains("NOT_ACTIVE"),
            "Expected NOT_ACTIVE error, got: {err_str}"
        );
    }

    #[tokio::test]
    async fn test_is_authenticated_before_and_after() {
        let wab = WABClient::new("http://localhost:3000");
        let mgr = WalletAuthenticationManager::new(
            wab,
            make_builder(),
            "admin.example.com".to_string(),
            None,
        )
        .unwrap();

        // Before auth
        let auth = mgr.is_authenticated(None).await.unwrap();
        assert!(!auth.authenticated, "Should not be authenticated initially");

        // Manually set inner wallet to simulate auth completion
        {
            let mock: Arc<dyn WalletInterface + Send + Sync> = Arc::new(MockWallet);
            let mut inner = mgr.inner.write().await;
            *inner = Some(mock);
        }

        // After auth
        let auth = mgr.is_authenticated(None).await.unwrap();
        assert!(
            auth.authenticated,
            "Should be authenticated after setting inner"
        );
    }

    // ------------------------------------------------------------------
    // resolve_root_key: the exact UMP 2-of-3 decision logic complete_auth
    // wires up, tested directly (no WAB/network/wallet needed).
    // ------------------------------------------------------------------

    const TEST_PASSWORD: &str = "correct horse battery staple";

    fn random_presentation_key() -> Vec<u8> {
        bsv::primitives::random::random_bytes(32)
    }

    #[test]
    fn test_new_user_builds_ump_token_and_returns_recovery_key() {
        let presentation_key = random_presentation_key();

        let resolved = resolve_root_key(
            &presentation_key,
            &SecondFactor::NewUserPassword(TEST_PASSWORD.to_string()),
            None,
        )
        .expect("new user resolution should succeed");

        assert!(resolved.is_new_user);
        assert!(resolved.token_to_publish.is_some());
        let recovery_key = resolved
            .generated_recovery_key
            .expect("new user must get a generated recovery key");
        assert_eq!(recovery_key.len(), 32);
        assert_eq!(resolved.root_key.len(), 32);
        // Root key must not be the presentation key bytes (the old bug).
        assert_ne!(
            resolved.root_key, presentation_key,
            "root key must never equal the raw presentation key bytes"
        );
    }

    #[test]
    fn test_returning_user_password_and_recovery_and_password_recovery_agree() {
        let presentation_key = random_presentation_key();

        // Step 1: sign up, capturing the built token + generated recovery key.
        let signup = resolve_root_key(
            &presentation_key,
            &SecondFactor::NewUserPassword(TEST_PASSWORD.to_string()),
            None,
        )
        .unwrap();
        let token = signup.token_to_publish.clone().unwrap();
        let recovery_key = signup.generated_recovery_key.clone().unwrap();
        let root_key = signup.root_key.clone();

        // Mode 1: presentation + password.
        let via_password = resolve_root_key(
            &presentation_key,
            &SecondFactor::Password(TEST_PASSWORD.to_string()),
            Some(&token),
        )
        .expect("presentation+password should unlock");
        assert!(!via_password.is_new_user);
        assert_eq!(via_password.root_key, root_key);

        // Mode 2: presentation + recovery.
        let via_recovery = resolve_root_key(
            &presentation_key,
            &SecondFactor::Recovery(recovery_key.clone()),
            Some(&token),
        )
        .expect("presentation+recovery should unlock");
        assert!(!via_recovery.is_new_user);
        assert_eq!(via_recovery.root_key, root_key);

        // Mode 3: password + recovery (the third 2-of-3 combination),
        // exercised directly against the UMP token / cwi_logic layer since
        // `complete_auth`'s SecondFactor is always paired with a WAB-issued
        // presentation key.
        let via_password_recovery = ump_token::unlock_root_primary_key(
            &token,
            &AuthFactors::PasswordRecovery {
                password: TEST_PASSWORD.as_bytes().to_vec(),
                recovery_key: recovery_key.clone(),
            },
        )
        .expect("password+recovery should unlock");
        assert_eq!(via_password_recovery, root_key);
    }

    #[test]
    fn test_returning_user_wrong_password_fails() {
        let presentation_key = random_presentation_key();
        let signup = resolve_root_key(
            &presentation_key,
            &SecondFactor::NewUserPassword(TEST_PASSWORD.to_string()),
            None,
        )
        .unwrap();
        let token = signup.token_to_publish.unwrap();

        let result = resolve_root_key(
            &presentation_key,
            &SecondFactor::Password("wrong-password".to_string()),
            Some(&token),
        );

        assert!(
            result.is_err(),
            "wrong password must not unlock the root key"
        );
    }

    #[test]
    fn test_new_user_second_factor_recovery_without_token_is_rejected() {
        let presentation_key = random_presentation_key();

        let result = resolve_root_key(
            &presentation_key,
            &SecondFactor::Recovery(vec![0u8; 32]),
            None,
        );

        assert!(
            result.is_err(),
            "a single factor (recovery only, no token) must never resolve a root key"
        );
    }

    #[test]
    fn test_signup_again_with_existing_token_is_rejected() {
        let presentation_key = random_presentation_key();
        let signup = resolve_root_key(
            &presentation_key,
            &SecondFactor::NewUserPassword(TEST_PASSWORD.to_string()),
            None,
        )
        .unwrap();
        let token = signup.token_to_publish.unwrap();

        let result = resolve_root_key(
            &presentation_key,
            &SecondFactor::NewUserPassword("another-password".to_string()),
            Some(&token),
        );

        assert!(
            result.is_err(),
            "signing up again over an existing token must be rejected"
        );
    }
}
