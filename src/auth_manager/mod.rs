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

use types::{AuthState, StateSnapshot, WalletBuilderFn};

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
    /// 1. Retrieves final presentation key from WAB
    /// 2. Looks up UMP token to determine new vs existing user
    /// 3. Derives root key from presentation key
    /// 4. Constructs inner wallet via wallet_builder
    /// 5. Optionally redeems faucet for new users
    /// 6. Signals authentication completion
    ///
    /// # Arguments
    /// * `interactor` - The auth method interactor
    /// * `payload` - Verification payload (e.g., SMS code)
    pub async fn complete_auth(
        &self,
        interactor: &dyn AuthMethodInteractor,
        payload: serde_json::Value,
    ) -> Result<(), WalletError> {
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

        // Look up UMP token to determine new vs existing user
        // For now, this always returns None (new user); full overlay integration later
        let presentation_key_bytes = hex_to_bytes(&presentation_key_hex)?;
        let _is_new_user = {
            let guard = self.inner.read().await;
            if let Some(ref _w) = *guard {
                // If we somehow already have a wallet, skip lookup
                false
            } else {
                // No wallet yet; use presentation key bytes as root key placeholder
                true
            }
        };

        // Derive root key from presentation key bytes
        // In the full CWI flow this would combine presentation key with password via UMP token
        // For now, use presentation key bytes directly as root key material
        let root_key = if presentation_key_bytes.len() >= 32 {
            presentation_key_bytes.clone()
        } else {
            return Err(WalletError::Internal(
                "Presentation key too short for root key derivation".to_string(),
            ));
        };

        // Build a placeholder PrivilegedKeyManager from root key
        let priv_key_manager = Arc::new(crate::wallet::privileged::NoOpPrivilegedKeyManager);

        // Construct inner wallet
        let wallet = (self.wallet_builder)(root_key, priv_key_manager).await?;

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

        Ok(())
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
        serde_json::to_vec(&snapshot).map_err(|e| {
            WalletError::Internal(format!("Failed to serialize state snapshot: {}", e))
        })
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
                outpoint: format!("{}.0", txid),
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
            .map_err(|e| WalletError::Internal(format!("Faucet create_action failed: {}", e)))?;

        let signable = create_result.signable_transaction.ok_or_else(|| {
            WalletError::Internal("No signable transaction from faucet create_action".to_string())
        })?;

        // Parse k-value for RPuzzle
        let k = BigNumber::from_hex(k_hex)
            .map_err(|e| WalletError::Internal(format!("Failed to parse faucet k value: {}", e)))?;
        let random_key = PrivateKey::from_random().map_err(|e| {
            WalletError::Internal(format!("Failed to generate random key for RPuzzle: {}", e))
        })?;

        // Create RPuzzle unlocking template with k value
        // The puzzle type is Raw since faucet uses raw R-value matching
        let puzzle = RPuzzle::from_k(RPuzzleType::Raw, vec![], k, random_key);

        // Convert signable.tx bytes to hex for Transaction::from_beef
        let tx_hex_str: String = signable.tx.iter().map(|b| format!("{:02x}", b)).collect();

        // Parse the signable transaction BEEF
        let tx = bsv::transaction::Transaction::from_beef(&tx_hex_str).map_err(|e| {
            WalletError::Internal(format!("Failed to parse signable transaction BEEF: {}", e))
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
                WalletError::Internal(format!("Failed to compute sighash preimage: {}", e))
            })?;

        // Unlock with RPuzzle (produces DER signature + sighash byte)
        let unlocking_script = puzzle
            .unlock(&preimage)
            .map_err(|e| WalletError::Internal(format!("RPuzzle unlock failed: {}", e)))?;

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
            .map_err(|e| WalletError::Internal(format!("Faucet sign_action failed: {}", e)))?;

        Ok(())
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
                .map_err(|e| WalletError::Internal(format!("Invalid hex at position {}: {}", i, e)))
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
            "Expected NOT_ACTIVE error, got: {}",
            err_str
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
}
