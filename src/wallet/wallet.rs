//! Wallet struct implementing the full WalletInterface trait.
//!
//! This is the core deliverable of Phase 5: a complete WalletInterface implementation
//! that wires together the signer, storage, and ProtoWallet subsystems. Every method
//! follows the validate-delegate-postprocess pattern from the TS reference.

use std::sync::Arc;

use async_trait::async_trait;

use bsv::primitives::ecdsa::ecdsa_verify;
use bsv::primitives::hash::{sha256, sha256_hmac};
use bsv::primitives::public_key::PublicKey;
use bsv::primitives::signature::Signature;
use bsv::primitives::symmetric_key::SymmetricKey;
use bsv::services::overlay_tools::LookupResolver;
use bsv::transaction::beef_party::BeefParty;
use bsv::wallet::error::WalletError as SdkWalletError;
use bsv::wallet::interfaces::{
    AbortActionArgs, AbortActionResult, AcquireCertificateArgs, AcquisitionProtocol,
    AuthenticatedResult, Certificate, CreateActionArgs, CreateActionOptions, CreateActionResult,
    CreateHmacArgs, CreateHmacResult, CreateSignatureArgs, CreateSignatureResult, DecryptArgs,
    DecryptResult, DiscoverByAttributesArgs, DiscoverByIdentityKeyArgs, DiscoverCertificatesResult,
    EncryptArgs, EncryptResult, GetHeaderArgs, GetHeaderResult, GetHeightResult, GetNetworkResult,
    GetPublicKeyArgs, GetPublicKeyResult, GetVersionResult, InternalizeActionArgs,
    InternalizeActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, Network, ProveCertificateArgs,
    ProveCertificateResult, RelinquishCertificateArgs, RelinquishCertificateResult,
    RelinquishOutputArgs, RelinquishOutputResult, RevealCounterpartyKeyLinkageArgs,
    RevealCounterpartyKeyLinkageResult, RevealSpecificKeyLinkageArgs,
    RevealSpecificKeyLinkageResult, SignActionArgs, SignActionResult, TrustSelf, VerifyHmacArgs,
    VerifyHmacResult, VerifySignatureArgs, VerifySignatureResult, WalletInterface,
};
use bsv::wallet::proto_wallet::ProtoWallet;
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};
use bsv::wallet::KeyDeriverApi;

use crate::wallet::discovery::OverlayCache;

use crate::error::WalletError;
use crate::services::traits::WalletServices;
use crate::signer::default_signer::DefaultWalletSigner;
use crate::signer::signing_context::{ContextualWallet, SigningContext};
use crate::signer::signing_provider::SigningProvider;
use crate::signer::traits::WalletSigner;
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;
use crate::wallet::privileged::PrivilegedKeyManager;
use crate::wallet::settings::WalletSettingsManager;
use crate::wallet::types::{
    AdminStatsResult, AuthId, KeyPair, StorageIdentity, UtxoInfo, WalletArgs, WalletBalance,
    SPEC_OP_FAILED_ACTIONS, SPEC_OP_INVALID_CHANGE, SPEC_OP_NO_SEND_ACTIONS,
    SPEC_OP_SET_WALLET_CHANGE_PARAMS, SPEC_OP_WALLET_BALANCE,
};
use crate::wallet::validation::validate_originator;

// ---------------------------------------------------------------------------
// Wallet struct
// ---------------------------------------------------------------------------

/// The core Wallet implementing all 28 WalletInterface methods.
///
/// Delegates crypto operations to ProtoWallet (or PrivilegedKeyManager),
/// action operations to DefaultWalletSigner, and query operations to
/// WalletStorageManager.
///
/// # Example
///
/// ```no_run
/// use bsv::primitives::private_key::PrivateKey;
/// use bsv_wallet_toolbox::wallet::setup::WalletBuilder;
/// use bsv_wallet_toolbox::types::Chain;
///
/// # async fn example() -> bsv_wallet_toolbox::WalletResult<()> {
/// let root_key = PrivateKey::from_hex("aa").unwrap();
/// let setup = WalletBuilder::new()
///     .chain(Chain::Test)
///     .root_key(root_key)
///     .with_sqlite_memory()
///     .build()
///     .await?;
///
/// // Use the wallet for operations
/// let wallet = setup.wallet;
/// # Ok(())
/// # }
/// ```
pub struct Wallet {
    /// BSV network chain this wallet operates on.
    pub chain: Chain,
    /// Key deriver for BRC-42/BRC-43 child key derivation.
    pub key_deriver: Arc<dyn KeyDeriverApi>,
    /// Storage manager providing persistence operations.
    pub storage: Arc<WalletStorageManager>,
    /// Optional network services (broadcasting, chain lookups, etc.).
    pub services: Option<Arc<dyn WalletServices>>,
    /// Optional background monitor for transaction lifecycle.
    pub monitor: Option<Arc<crate::monitor::Monitor>>,
    /// Optional privileged key manager for sensitive crypto operations.
    pub privileged_key_manager: Option<Arc<dyn PrivilegedKeyManager>>,
    /// Settings manager with cached TTL for wallet configuration.
    pub settings_manager: WalletSettingsManager,
    /// Public identity key derived from the root private key.
    pub identity_key: PublicKey,
    /// Protocol wallet providing default WalletInterface implementations.
    pub proto: ProtoWallet,

    // BeefParty state
    beef: tokio::sync::Mutex<BeefParty>,
    /// Whether to include all source transactions in BEEF output.
    pub include_all_source_transactions: bool,
    /// Whether to automatically add known txids from storage.
    pub auto_known_txids: bool,
    /// Whether to return only the txid without full BEEF data.
    pub return_txid_only: bool,
    /// Self-trust configuration for signed operations.
    pub trust_self: Option<TrustSelf>,
    // Stored for future BEEF-party operations; currently only referenced at
    // construction time via `BeefParty::new(...)`. Kept to preserve the
    // identity used to construct `beef` so later operations can rebind it.
    #[allow(dead_code)]
    user_party: String,

    // Overlay discovery cache
    overlay_cache: OverlayCache,
    /// Optional overlay lookup resolver for identity certificate discovery.
    pub lookup_resolver: Option<Arc<LookupResolver>>,

    /// Test hook: pre-determined random values for deterministic testing.
    pub random_vals: Option<Vec<f64>>,

    /// The wallet's one and only signer.
    ///
    /// It owns the single `pending_sign_actions` map that `createAction` writes
    /// and `signAction` reads. A delegated custody backend goes *inside* it (see
    /// [`WalletArgs::signing_provider`]) — never alongside it.
    signer: DefaultWalletSigner,
}

// ---------------------------------------------------------------------------
// Constructor and helpers
// ---------------------------------------------------------------------------

impl Wallet {
    /// Create a new Wallet from WalletArgs.
    ///
    /// Validates that the key deriver's identity key matches the storage auth ID.
    pub fn new(args: WalletArgs) -> Result<Self, WalletError> {
        // Validate identity key match
        let identity_key = args.key_deriver.identity_key();
        let identity_key_hex = identity_key.to_der_hex();
        let storage_auth_id = args.storage.auth_id().to_string();
        if identity_key_hex != storage_auth_id {
            return Err(WalletError::InvalidParameter {
                parameter: "key_deriver".to_string(),
                must_be: format!(
                    "consistent with storage auth_id. key_deriver identity_key={identity_key_hex} but storage auth_id={storage_auth_id}"
                ),
            });
        }

        // Wrap the deriver itself, never its root key: a deriver whose identity
        // is decoupled from a locally-held root (a threshold vault) must answer
        // identity queries with its true identity and surface derivation errors,
        // not silently derive from a throwaway root.
        let proto = ProtoWallet::from_key_deriver(args.key_deriver.clone());

        // Derive user_party
        let root_pub_hex = args.key_deriver.root_key().to_public_key().to_der_hex();
        let user_party = format!("user {root_pub_hex}");

        // Initialize BeefParty
        let beef = BeefParty::new([user_party.clone()]);

        // Wire services into signer
        let services_for_signer = args.services.clone().ok_or_else(|| {
            WalletError::InvalidOperation(
                "WalletServices required for Wallet construction".to_string(),
            )
        })?;

        // Signer shares the same Arc<WalletStorageManager> as the wallet.
        // The signing provider is injected INTO this signer, so the pending
        // sign actions it records stay in one map.
        let signer = DefaultWalletSigner::new(
            args.storage.clone(),
            services_for_signer,
            args.key_deriver.clone(),
            args.chain.clone(),
            identity_key.clone(),
            args.signing_provider,
        );

        // Settings manager: use provided or create default
        let settings_manager = args.settings_manager.unwrap_or_else(|| {
            // Use the initial active provider for the settings cache.
            // The wallet always requires at least one provider.
            let active_provider = args
                .storage
                .active()
                .cloned()
                .expect("WalletStorageManager must have at least one storage provider");
            WalletSettingsManager::new(active_provider)
        });

        // Captured before `args.chain` moves into the struct below.
        let resolver_network = match args.chain {
            Chain::Main => bsv::services::overlay_tools::Network::Mainnet,
            Chain::Test => bsv::services::overlay_tools::Network::Testnet,
        };

        Ok(Wallet {
            chain: args.chain,
            key_deriver: args.key_deriver,
            storage: args.storage,
            services: args.services,
            monitor: args.monitor,
            privileged_key_manager: args.privileged_key_manager,
            settings_manager,
            identity_key,
            proto,
            beef: tokio::sync::Mutex::new(beef),
            include_all_source_transactions: true,
            auto_known_txids: false,
            return_txid_only: false,
            trust_self: Some(TrustSelf::Known),
            user_party,
            overlay_cache: OverlayCache::new(),
            // TS builds one from the chain when the caller supplies none
            // (Wallet.ts: `args.lookupResolver || new LookupResolver({...})`),
            // so discovery works on a default wallet. Leaving this None made
            // discoverByIdentityKey/discoverByAttributes fail with "No lookup
            // resolver configured" for every wallet built through
            // WalletBuilder, which had no way to set it.
            lookup_resolver: args
                .lookup_resolver
                .or_else(|| Some(Arc::new(LookupResolver::for_network(resolver_network)))),
            random_vals: None,
            signer,
        })
    }

    /// Returns the AuthId for this wallet.
    fn auth_id(&self) -> AuthId {
        AuthId {
            identity_key: self.identity_key.to_der_hex(),
            user_id: None,
            is_active: None,
        }
    }

    /// Validates the originator parameter.
    fn validate_originator(&self, originator: Option<&str>) -> Result<(), WalletError> {
        validate_originator(originator)
    }

    /// Returns a reference to the wallet services, or an error if not configured.
    fn get_services(&self) -> Result<&Arc<dyn WalletServices>, WalletError> {
        self.services.as_ref().ok_or_else(|| {
            WalletError::InvalidOperation("Wallet services not configured".to_string())
        })
    }

    /// The delegated custody backend, if one was injected.
    ///
    /// `None` means this wallet derives and signs locally from its own root key.
    pub fn signing_provider(&self) -> Option<&Arc<dyn SigningProvider>> {
        self.signer.signing_provider()
    }

    /// Returns the client change key pair (root private + public key).
    ///
    /// Errors when a signing provider is injected: the root key behind a
    /// delegated deriver is not the key that locks this wallet's change, so
    /// handing it out would invite the caller to build outputs nobody can spend.
    pub fn get_client_change_key_pair(&self) -> Result<KeyPair, WalletError> {
        if self.signing_provider().is_some() {
            return Err(WalletError::InvalidOperation(
                "get_client_change_key_pair is unavailable when a signing provider is injected: \
                 change is locked by the provider, not by key_deriver's root key"
                    .to_string(),
            ));
        }
        let root = self.key_deriver.root_key();
        Ok(KeyPair {
            private_key: root.to_hex(),
            public_key: root.to_public_key().to_der_hex(),
        })
    }

    /// Returns the storage identity for this wallet.
    ///
    /// Returns the active store's storage_identity_key if available (after
    /// make_available), otherwise falls back to the wallet's identity key.
    pub async fn get_storage_identity(&self) -> StorageIdentity {
        let key = self
            .storage
            .get_storage_identity_key()
            .await
            .unwrap_or_else(|_| self.identity_key.to_der_hex());
        StorageIdentity {
            storage_identity_key: key,
            storage_name: "default".to_string(),
        }
    }

    /// Returns the storage party string.
    pub async fn storage_party(&self) -> String {
        let si = self.get_storage_identity().await;
        format!("storage {}", si.storage_identity_key)
    }

    /// Returns the identity key as a hex string.
    pub async fn get_identity_key(&self) -> Result<String, SdkWalletError> {
        let result = self
            .get_public_key(
                GetPublicKeyArgs {
                    identity_key: true,
                    protocol_id: None,
                    key_id: None,
                    counterparty: None,
                    privileged: false,
                    privileged_reason: None,
                    for_self: None,
                    seek_permission: None,
                },
                None,
            )
            .await?;
        Ok(result.public_key.to_der_hex())
    }

    /// Destroy the wallet: destroys storage and privileged key manager if present.
    pub async fn destroy(&self) -> Result<(), WalletError> {
        self.storage.destroy().await?;
        if let Some(ref pkm) = self.privileged_key_manager {
            pkm.destroy_key().await.map_err(|e| {
                WalletError::Internal(format!("Failed to destroy privileged key: {e}"))
            })?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Convenience methods (Phase 7)
    // -----------------------------------------------------------------------

    /// Returns the total spendable balance in satoshis.
    ///
    /// Routes through the specOp WalletBalance basket to sum all spendable
    /// outputs. If `args` is provided with a non-specOp basket, the specOp
    /// constant is pushed to tags to combine basket filtering with balance.
    pub async fn balance(&self, args: Option<ListOutputsArgs>) -> Result<u64, WalletError> {
        let args = match args {
            Some(mut a) => {
                if a.basket != SPEC_OP_WALLET_BALANCE {
                    a.tags.push(SPEC_OP_WALLET_BALANCE.to_string());
                }
                a
            }
            None => ListOutputsArgs {
                basket: SPEC_OP_WALLET_BALANCE.to_string(),
                tags: vec![],
                tag_query_mode: None,
                include: None,
                include_custom_instructions: Default::default(),
                include_tags: Default::default(),
                include_labels: Default::default(),
                limit: None,
                offset: None,
                seek_permission: Default::default(),
            },
        };

        let r = self
            .list_outputs(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;
        Ok(r.total_outputs as u64)
    }

    /// Returns the total spendable balance plus individual UTXO details.
    ///
    /// Similar to `balance()` but also collects per-output information.
    pub async fn balance_and_utxos(
        &self,
        args: Option<ListOutputsArgs>,
    ) -> Result<WalletBalance, WalletError> {
        let args = match args {
            Some(mut a) => {
                if a.basket != SPEC_OP_WALLET_BALANCE {
                    a.tags.push(SPEC_OP_WALLET_BALANCE.to_string());
                }
                a
            }
            None => ListOutputsArgs {
                basket: SPEC_OP_WALLET_BALANCE.to_string(),
                tags: vec![],
                tag_query_mode: None,
                include: None,
                include_custom_instructions: Default::default(),
                include_tags: Default::default(),
                include_labels: Default::default(),
                limit: None,
                offset: None,
                seek_permission: Default::default(),
            },
        };

        let r = self
            .list_outputs(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;

        let utxos: Vec<UtxoInfo> = r
            .outputs
            .iter()
            .map(|o| UtxoInfo {
                satoshis: o.satoshis,
                outpoint: o.outpoint.clone(),
            })
            .collect();

        Ok(WalletBalance {
            total: r.total_outputs as u64,
            utxos,
        })
    }

    /// Transfer all wallet funds to another wallet using BRC-29 payment.
    ///
    /// Creates a sweep transaction sending MAX_POSSIBLE_SATOSHIS to the
    /// receiving wallet via createAction + internalizeAction.
    ///
    /// Unavailable when a signing provider is injected: the BRC-29 output is
    /// locked here with `key_deriver.root_key()` as the ECDH locker, and under
    /// delegation that key is not the one the receiver will derive against.
    /// The [`SigningProvider`] trait has no "lock to a counterparty" member to
    /// route this through, so it fails closed rather than sending funds the
    /// receiver cannot internalize.
    pub async fn sweep_to(&self, to_wallet: &Wallet) -> Result<(), WalletError> {
        use crate::storage::methods::generate_change::MAX_POSSIBLE_SATOSHIS;
        use crate::utility::script_template_brc29::ScriptTemplateBRC29;

        if self.signing_provider().is_some() {
            return Err(WalletError::InvalidOperation(
                "sweep_to is unavailable when a signing provider is injected: it locks the \
                 outgoing BRC-29 output with the local root key, which the provider does not own"
                    .to_string(),
            ));
        }

        // Generate random derivation prefix and suffix
        let derivation_prefix = random_base64(8);
        let derivation_suffix = random_base64(8);

        let template =
            ScriptTemplateBRC29::new(derivation_prefix.clone(), derivation_suffix.clone());

        // Lock with sender's private key to receiver's public key
        let sender_priv = self.key_deriver.root_key().clone();
        let receiver_pub = to_wallet.identity_key.clone();
        let lock_script = template.lock(&sender_priv, &receiver_pub)?;

        let custom_instructions = serde_json::json!({
            "derivationPrefix": derivation_prefix,
            "derivationSuffix": derivation_suffix,
            "type": "BRC29"
        })
        .to_string();

        let car = self
            .create_action(
                CreateActionArgs {
                    description: "sweep".to_string(),
                    input_beef: None,
                    inputs: None,
                    outputs: Some(vec![bsv::wallet::interfaces::CreateActionOutput {
                        locking_script: Some(lock_script),
                        satoshis: MAX_POSSIBLE_SATOSHIS,
                        output_description: "sweep".to_string(),
                        basket: None,
                        custom_instructions: Some(custom_instructions),
                        tags: Some(vec!["relinquish".to_string()]),
                    }]),
                    lock_time: None,
                    version: None,
                    labels: Some(vec!["sweep".to_string()]),
                    options: Some(CreateActionOptions {
                        randomize_outputs: bsv::wallet::types::BooleanDefaultTrue(Some(false)),
                        accept_delayed_broadcast: bsv::wallet::types::BooleanDefaultTrue(Some(
                            false,
                        )),
                        ..Default::default()
                    }),
                    reference: None,
                },
                None,
            )
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;

        let tx = car.tx.ok_or_else(|| {
            WalletError::Internal("sweep createAction returned no tx".to_string())
        })?;

        // Internalize on receiving wallet
        to_wallet
            .internalize_action(
                InternalizeActionArgs {
                    tx,
                    description: "sweep".to_string(),
                    labels: Some(vec!["sweep".to_string()]),
                    seek_permission: bsv::wallet::types::BooleanDefaultTrue(Some(false)),
                    outputs: vec![bsv::wallet::interfaces::InternalizeOutput::WalletPayment {
                        output_index: 0,
                        payment: bsv::wallet::interfaces::Payment {
                            // `Payment` carries the RAW derivation bytes; storage
                            // base64-encodes them when persisting. The lock above
                            // used the base64 STRING as the BRC-29 key-id, so we
                            // must hand back the bytes that base64-encode to that
                            // same string — a strict base64 decode, never
                            // `into_bytes` (which would double-encode into
                            // base64-of-base64 and make the output unspendable).
                            derivation_prefix: payment_derivation_bytes(&derivation_prefix)?,
                            derivation_suffix: payment_derivation_bytes(&derivation_suffix)?,
                            sender_identity_key: self.identity_key.clone(),
                        },
                    }],
                },
                None,
            )
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Check UTXOs against the network and identify invalid/unspendable outputs.
    ///
    /// If `release` is true, locked invalid outputs are released.
    /// If `all` is true, all outputs are checked (not just change).
    pub async fn review_spendable_outputs(
        &self,
        release: bool,
        all: bool,
    ) -> Result<ListOutputsResult, WalletError> {
        let mut tags = Vec::new();
        if release {
            tags.push("release".to_string());
        }
        if all {
            tags.push("all".to_string());
        }

        let args = ListOutputsArgs {
            basket: SPEC_OP_INVALID_CHANGE.to_string(),
            tags,
            tag_query_mode: None,
            include: None,
            include_custom_instructions: Default::default(),
            include_tags: Default::default(),
            include_labels: Default::default(),
            limit: None,
            offset: None,
            seek_permission: Default::default(),
        };

        self.list_outputs(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))
    }

    /// Configure the number and size of change outputs for the default basket.
    ///
    /// Parameters are passed via specOp tags to list_outputs.
    pub async fn set_wallet_change_params(
        &self,
        count: u32,
        satoshis: u64,
    ) -> Result<(), WalletError> {
        let args = ListOutputsArgs {
            basket: SPEC_OP_SET_WALLET_CHANGE_PARAMS.to_string(),
            tags: vec![count.to_string(), satoshis.to_string()],
            tag_query_mode: None,
            include: None,
            include_custom_instructions: Default::default(),
            include_tags: Default::default(),
            include_labels: Default::default(),
            limit: None,
            offset: None,
            seek_permission: Default::default(),
        };

        let _ = self
            .list_outputs(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))?;
        Ok(())
    }

    /// List all no-send (un-broadcast) actions.
    ///
    /// If `abort` is true, each matched action is aborted after querying.
    pub async fn list_no_send_actions(
        &self,
        abort: bool,
    ) -> Result<ListActionsResult, WalletError> {
        let mut labels = vec![SPEC_OP_NO_SEND_ACTIONS.to_string()];
        if abort {
            labels.push("abort".to_string());
        }

        let args = ListActionsArgs {
            labels,
            label_query_mode: None,
            include_labels: Default::default(),
            include_inputs: Default::default(),
            include_input_source_locking_scripts: Default::default(),
            include_input_unlocking_scripts: Default::default(),
            include_outputs: Default::default(),
            include_output_locking_scripts: Default::default(),
            limit: None,
            offset: None,
            seek_permission: Default::default(),
        };

        self.list_actions(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))
    }

    /// List all failed actions.
    ///
    /// If `unfail` is true, each matched action is reset to unprocessed for retry.
    pub async fn list_failed_actions(
        &self,
        unfail: bool,
    ) -> Result<ListActionsResult, WalletError> {
        let mut labels = vec![SPEC_OP_FAILED_ACTIONS.to_string()];
        if unfail {
            labels.push("unfail".to_string());
        }

        let args = ListActionsArgs {
            labels,
            label_query_mode: None,
            include_labels: Default::default(),
            include_inputs: Default::default(),
            include_input_source_locking_scripts: Default::default(),
            include_input_unlocking_scripts: Default::default(),
            include_outputs: Default::default(),
            include_output_locking_scripts: Default::default(),
            limit: None,
            offset: None,
            seek_permission: Default::default(),
        };

        self.list_actions(args, None)
            .await
            .map_err(|e| WalletError::Internal(e.to_string()))
    }

    /// Returns aggregate deployment statistics from storage.
    ///
    /// Delegates to the storage manager's admin_stats method.
    pub async fn admin_stats(&self) -> Result<AdminStatsResult, WalletError> {
        let identity_key_hex = self.identity_key.to_der_hex();
        self.storage.admin_stats(&identity_key_hex).await
    }
}

/// Generate a random base64-encoded string from `n` random bytes.
fn random_base64(n: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

/// Convert a base64 derivation key-id string (the form used to build the BRC-29
/// lock) into the raw bytes carried by [`bsv::wallet::interfaces::Payment`].
///
/// `storage_internalize_action` re-encodes `Payment`'s derivation bytes to base64
/// when persisting, so the stored spend key-id equals `base64(bytes)`. To keep the
/// spend key-id equal to the base64 string used at lock time, the bytes handed to
/// `Payment` must be exactly those that base64-encode back to that string — i.e. a
/// strict base64 *decode*. Using `String::into_bytes` (the UTF-8 of the base64
/// text) would make storage produce base64-of-base64 and desync the key-ids,
/// rendering the swept output unspendable. This must NOT fall back to lossy UTF-8
/// decoding.
fn payment_derivation_bytes(key_id_b64: &str) -> Result<Vec<u8>, WalletError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(key_id_b64)
        .map_err(|e| WalletError::Internal(format!("invalid base64 derivation key-id: {e}")))
}

// ---------------------------------------------------------------------------
// Delegated-crypto composition helpers
//
// When a signing provider is injected, the nine BRC-100 crypto methods use its
// three key operations (derive_public_key, derive_symmetric_key,
// create_signature) and compose everything else — AES-GCM, HMAC, signature
// verification — locally, so a remote backend never sees plaintext. Semantics
// must match ProtoWallet exactly; these helpers mirror its argument handling.
// ---------------------------------------------------------------------------

/// Substitute the per-operation default when the caller left the counterparty
/// Uninitialized — the same dispatch as `ProtoWallet::default_counterparty`
/// (Self_ for everything except createSignature, which defaults to Anyone).
fn effective_counterparty(
    counterparty: &Counterparty,
    default_type: CounterpartyType,
) -> Counterparty {
    if counterparty.counterparty_type == CounterpartyType::Uninitialized {
        Counterparty {
            counterparty_type: default_type,
            public_key: None,
        }
    } else {
        counterparty.clone()
    }
}

/// The 32-byte ECDSA digest for createSignature/verifySignature: the caller's
/// direct hash when provided, otherwise SHA-256 of the data. Mirrors
/// ProtoWallet's argument handling, including error text.
fn signature_digest(
    data: Option<&[u8]>,
    direct_hash: Option<&[u8]>,
    direct_hash_param: &str,
) -> Result<[u8; 32], SdkWalletError> {
    if let Some(h) = direct_hash {
        if h.len() != 32 {
            return Err(SdkWalletError::InvalidParameter(format!(
                "{direct_hash_param} must be exactly 32 bytes"
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(h);
        Ok(arr)
    } else if let Some(d) = data {
        Ok(sha256(d))
    } else {
        Err(SdkWalletError::InvalidParameter(format!(
            "either data or {direct_hash_param} must be provided"
        )))
    }
}

/// Derive the shared symmetric key through the provider and rebuild the SDK
/// `SymmetricKey` from its 32 bytes, so AES (full 32 bytes) and HMAC (minimal
/// big-endian via `to_hmac_key_bytes`) keying stay byte-identical to the
/// local ProtoWallet path.
async fn provider_symmetric_key(
    provider: &dyn SigningProvider,
    protocol: &Protocol,
    key_id: &str,
    counterparty: &Counterparty,
) -> Result<SymmetricKey, SdkWalletError> {
    let key_bytes = provider
        .derive_symmetric_key(protocol, key_id, counterparty)
        .await
        .map_err(to_sdk_error)?;
    SymmetricKey::from_bytes(&key_bytes)
        .map_err(|e| SdkWalletError::Internal(format!("invalid provider symmetric key: {e}")))
}

/// Constant-time byte comparison for HMAC verification.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The NotImplemented error for key-linkage revelation under delegation: the
/// linkage secrets exist only inside the custody backend, and neither reveal
/// is expressible through the provider's three key operations.
fn linkage_unsupported_under_delegation(method: &str) -> SdkWalletError {
    SdkWalletError::NotImplemented(format!(
        "{method} is not supported when a signing provider is injected"
    ))
}

// ---------------------------------------------------------------------------
// Error conversion: crate::error::WalletError -> bsv::wallet::error::WalletError
// ---------------------------------------------------------------------------

fn to_sdk_error(e: WalletError) -> SdkWalletError {
    match e {
        WalletError::InvalidParameter { parameter, must_be } => {
            SdkWalletError::InvalidParameter(format!("The {parameter} parameter must be {must_be}"))
        }
        WalletError::NotImplemented(msg) => SdkWalletError::NotImplemented(msg),
        WalletError::InvalidOperation(msg) => SdkWalletError::Internal(msg),
        _ => SdkWalletError::Internal(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// WalletInterface implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl WalletInterface for Wallet {
    // -----------------------------------------------------------------------
    // CRYPTO methods (9) -- delegate to ProtoWallet or PrivilegedKeyManager
    // -----------------------------------------------------------------------

    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
        originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_get_public_key_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.get_public_key(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            if args.identity_key {
                return Ok(GetPublicKeyResult {
                    public_key: provider.identity_public_key().clone(),
                });
            }
            let protocol = args.protocol_id.unwrap_or(Protocol {
                security_level: 0,
                protocol: String::new(),
            });
            let key_id = args.key_id.unwrap_or_default();
            if protocol.protocol.is_empty() || key_id.is_empty() {
                return Err(SdkWalletError::InvalidParameter(
                    "protocolID and keyID are required if identityKey is false".to_string(),
                ));
            }
            let counterparty = effective_counterparty(
                &args.counterparty.unwrap_or_default(),
                CounterpartyType::Self_,
            );
            let public_key = provider
                .derive_public_key(
                    &protocol,
                    &key_id,
                    &counterparty,
                    args.for_self.unwrap_or(false),
                )
                .await
                .map_err(to_sdk_error)?;
            return Ok(GetPublicKeyResult { public_key });
        }
        self.proto.get_public_key(args, originator).await
    }

    async fn encrypt(
        &self,
        args: EncryptArgs,
        originator: Option<&str>,
    ) -> Result<EncryptResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_encrypt_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.encrypt(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Self_);
            let key = provider_symmetric_key(
                provider.as_ref(),
                &args.protocol_id,
                &args.key_id,
                &counterparty,
            )
            .await?;
            let ciphertext = key
                .encrypt(&args.plaintext)
                .map_err(|e| SdkWalletError::Internal(format!("AES-GCM encryption failed: {e}")))?;
            return Ok(EncryptResult { ciphertext });
        }
        self.proto.encrypt(args, originator).await
    }

    async fn decrypt(
        &self,
        args: DecryptArgs,
        originator: Option<&str>,
    ) -> Result<DecryptResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_decrypt_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.decrypt(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Self_);
            let key = provider_symmetric_key(
                provider.as_ref(),
                &args.protocol_id,
                &args.key_id,
                &counterparty,
            )
            .await?;
            let plaintext = key.decrypt(&args.ciphertext)?;
            return Ok(DecryptResult { plaintext });
        }
        self.proto.decrypt(args, originator).await
    }

    async fn create_hmac(
        &self,
        args: CreateHmacArgs,
        originator: Option<&str>,
    ) -> Result<CreateHmacResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_create_hmac_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.create_hmac(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Self_);
            let key = provider_symmetric_key(
                provider.as_ref(),
                &args.protocol_id,
                &args.key_id,
                &counterparty,
            )
            .await?;
            // Minimal big-endian keying: a leading-zero key is a 31-byte HMAC
            // key, matching TS `key.toArray()`. Full 32 bytes would silently
            // break interop for ~1 key in 256.
            let hmac = sha256_hmac(&key.to_hmac_key_bytes(), &args.data).to_vec();
            return Ok(CreateHmacResult { hmac });
        }
        self.proto.create_hmac(args, originator).await
    }

    async fn verify_hmac(
        &self,
        args: VerifyHmacArgs,
        originator: Option<&str>,
    ) -> Result<VerifyHmacResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_verify_hmac_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.verify_hmac(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Self_);
            let key = provider_symmetric_key(
                provider.as_ref(),
                &args.protocol_id,
                &args.key_id,
                &counterparty,
            )
            .await?;
            let expected = sha256_hmac(&key.to_hmac_key_bytes(), &args.data);
            if !constant_time_eq(&expected, &args.hmac) {
                return Err(SdkWalletError::InvalidHmac);
            }
            return Ok(VerifyHmacResult { valid: true });
        }
        self.proto.verify_hmac(args, originator).await
    }

    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
        originator: Option<&str>,
    ) -> Result<CreateSignatureResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_create_signature_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.create_signature(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            // createSignature is the one op whose counterparty defaults to
            // Anyone, matching TS ProtoWallet.
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Anyone);
            let digest = signature_digest(
                args.data.as_deref(),
                args.hash_to_directly_sign.as_deref(),
                "hash_to_directly_sign",
            )?;
            // BRC-100 surface: the wallet signs for itself. A host that
            // authenticates a caller reaches the provider seam through the
            // action path (`ContextualWallet`), not this method.
            let signature = provider
                .create_signature(
                    &args.protocol_id,
                    &args.key_id,
                    &counterparty,
                    &digest,
                    &SigningContext::itself(),
                )
                .await
                .map_err(to_sdk_error)?;
            return Ok(CreateSignatureResult { signature });
        }
        self.proto.create_signature(args, originator).await
    }

    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
        originator: Option<&str>,
    ) -> Result<VerifySignatureResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_verify_signature_args(&args)?;

        if args.privileged {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.verify_signature(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if let Some(provider) = self.signing_provider() {
            // Public-key derivation goes through the provider; verification is
            // local — no secret material is involved in verifying.
            let counterparty = effective_counterparty(&args.counterparty, CounterpartyType::Self_);
            let digest = signature_digest(
                args.data.as_deref(),
                args.hash_to_directly_verify.as_deref(),
                "hash_to_directly_verify",
            )?;
            let derived_pub = provider
                .derive_public_key(
                    &args.protocol_id,
                    &args.key_id,
                    &counterparty,
                    args.for_self.unwrap_or(false),
                )
                .await
                .map_err(to_sdk_error)?;
            let sig = Signature::from_der(&args.signature)?;
            if !ecdsa_verify(&digest, &sig, derived_pub.point())? {
                return Err(SdkWalletError::InvalidSignature);
            }
            return Ok(VerifySignatureResult { valid: true });
        }
        self.proto.verify_signature(args, originator).await
    }

    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_reveal_counterparty_key_linkage_args(&args)?;

        if args.privileged.unwrap_or(false) {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.reveal_counterparty_key_linkage(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if self.signing_provider().is_some() {
            return Err(linkage_unsupported_under_delegation(
                "revealCounterpartyKeyLinkage",
            ));
        }
        self.proto
            .reveal_counterparty_key_linkage(args, originator)
            .await
    }

    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_reveal_specific_key_linkage_args(&args)?;

        if args.privileged.unwrap_or(false) {
            if let Some(ref pkm) = self.privileged_key_manager {
                return pkm.reveal_specific_key_linkage(args).await;
            }
            return Err(SdkWalletError::Internal(
                "No privileged key manager configured".to_string(),
            ));
        }
        if self.signing_provider().is_some() {
            return Err(linkage_unsupported_under_delegation(
                "revealSpecificKeyLinkage",
            ));
        }
        self.proto
            .reveal_specific_key_linkage(args, originator)
            .await
    }

    // -----------------------------------------------------------------------
    // INFO methods (6)
    // -----------------------------------------------------------------------

    async fn is_authenticated(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        Ok(AuthenticatedResult {
            authenticated: true,
        })
    }

    async fn wait_for_authentication(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        Ok(AuthenticatedResult {
            authenticated: true,
        })
    }

    async fn get_height(
        &self,
        originator: Option<&str>,
    ) -> Result<GetHeightResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        let services = self.get_services().map_err(to_sdk_error)?;
        let height = services
            .get_height()
            .await
            .map_err(|e| SdkWalletError::Internal(e.to_string()))?;
        Ok(GetHeightResult { height })
    }

    async fn get_header_for_height(
        &self,
        args: GetHeaderArgs,
        originator: Option<&str>,
    ) -> Result<GetHeaderResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_get_header_args(&args)?;
        let services = self.get_services().map_err(to_sdk_error)?;
        let header = services
            .get_header_for_height(args.height)
            .await
            .map_err(|e| SdkWalletError::Internal(e.to_string()))?;
        Ok(GetHeaderResult { header })
    }

    async fn get_network(
        &self,
        originator: Option<&str>,
    ) -> Result<GetNetworkResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        let network = match self.chain {
            Chain::Main => Network::Mainnet,
            Chain::Test => Network::Testnet,
        };
        Ok(GetNetworkResult { network })
    }

    async fn get_version(
        &self,
        originator: Option<&str>,
    ) -> Result<GetVersionResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        Ok(GetVersionResult {
            version: "wallet-brc100-1.0.0".to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // ACTION methods (4) -- delegate to signer
    // -----------------------------------------------------------------------

    async fn create_action(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
    ) -> Result<CreateActionResult, SdkWalletError> {
        // The BRC-100 surface has no caller context: the wallet acts for
        // itself. Transports that authenticate a caller use `create_action_in`.
        self.create_action_in(args, originator, &SigningContext::itself())
            .await
    }

    async fn sign_action(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
    ) -> Result<SignActionResult, SdkWalletError> {
        // The BRC-100 surface has no caller context: the wallet acts for
        // itself. Transports that authenticate a caller use `sign_action_in`.
        self.sign_action_in(args, originator, &SigningContext::itself())
            .await
    }

    async fn internalize_action(
        &self,
        args: InternalizeActionArgs,
        originator: Option<&str>,
    ) -> Result<InternalizeActionResult, SdkWalletError> {
        let _spend_guard = self
            .storage
            .acquire_spend_lock()
            .await
            .map_err(to_sdk_error)?;
        tracing::debug!(description = %args.description, "internalizeAction starting");
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_internalize_action_args(&args)?;

        // Check specOp throw review actions label
        for label in args.labels.as_deref().unwrap_or(&[]) {
            if crate::wallet::types::is_spec_op_throw_label(label) {
                return Err(SdkWalletError::Internal(
                    "WERR_REVIEW_ACTIONS: internalizeAction specOp throw review actions"
                        .to_string(),
                ));
            }
        }

        // Pass SDK InternalizeOutput enum directly through to signer
        let valid_args = crate::signer::types::ValidInternalizeActionArgs {
            tx: args.tx,
            description: args.description,
            labels: args.labels.unwrap_or_default(),
            outputs: args.outputs,
        };

        let signer_result = self
            .signer
            .internalize_action(valid_args)
            .await
            .map_err(to_sdk_error)?;

        let result = InternalizeActionResult {
            accepted: signer_result.accepted,
        };

        crate::wallet::error_helpers::throw_if_unsuccessful_internalize_action(&result)?;

        tracing::info!(accepted = result.accepted, "internalizeAction completed");
        Ok(result)
    }

    async fn abort_action(
        &self,
        args: AbortActionArgs,
        originator: Option<&str>,
    ) -> Result<AbortActionResult, SdkWalletError> {
        let _spend_guard = self
            .storage
            .acquire_spend_lock()
            .await
            .map_err(to_sdk_error)?;
        tracing::debug!(reference = %String::from_utf8_lossy(&args.reference), "abortAction starting");
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_abort_action_args(&args)?;

        let auth = self.auth_id();
        let result = self
            .storage
            .abort_action(&auth, &args)
            .await
            .map_err(to_sdk_error)?;
        tracing::info!("abortAction completed");
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // QUERY methods (3) -- delegate to storage
    // -----------------------------------------------------------------------

    async fn list_actions(
        &self,
        args: ListActionsArgs,
        originator: Option<&str>,
    ) -> Result<ListActionsResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_list_actions_args(&args)?;

        let auth = self.auth_id();
        let mut result = self
            .storage
            .list_actions(&auth, &args)
            .await
            .map_err(to_sdk_error)?;

        // Strip customInstructions from outputs (security policy)
        for action in &mut result.actions {
            for output in action.outputs.as_deref_mut().unwrap_or(&mut []) {
                output.custom_instructions = None;
            }
        }

        Ok(result)
    }

    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        originator: Option<&str>,
    ) -> Result<ListOutputsResult, SdkWalletError> {
        use bsv::transaction::beef::{Beef, BEEF_V2};
        use bsv::wallet::interfaces::OutputInclude;
        use std::collections::HashSet;

        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_list_outputs_args(&args)?;

        let auth = self.auth_id();
        let mut result = self
            .storage
            .list_outputs(&auth, &args)
            .await
            .map_err(to_sdk_error)?;

        // BEEF assembly at the wallet layer for EntireTransactions requests.
        // The storage layer returns beef = None (stub); we build it here where
        // we have access to StorageProvider (required by get_valid_beef_for_txid).
        if matches!(args.include, Some(OutputInclude::EntireTransactions)) && result.beef.is_none()
        {
            // Collect unique txids from outpoints (format: "txid.vout")
            let mut unique_txids: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for output in &result.outputs {
                if let Some(dot_pos) = output.outpoint.find('.') {
                    let txid = output.outpoint[..dot_pos].to_string();
                    if seen.insert(txid.clone()) {
                        unique_txids.push(txid);
                    }
                }
            }

            if !unique_txids.is_empty() {
                // The caller asked for ENTIRE transactions, so assembly must not
                // elide the wallet's own txs as txid-only: the verify step below
                // resolves txid-only entries against the in-memory BeefParty,
                // which a fresh process holds EMPTY — trustSelf-elided entries
                // then fail as "unable to merge txid into beef" even though
                // storage holds the raw tx and its proof. trustSelf is an
                // optimization for counterparties that share this wallet's
                // storage; EntireTransactions is a request not to apply it.
                let beef_trust_self = crate::storage::beef::TrustSelf::No;
                let known_txids: HashSet<String> = HashSet::new();
                let storage_provider = self.storage.get_active().await.map_err(to_sdk_error)?;

                // Build a single merged BEEF from all txids through the SDK's
                // merge semantics (TS listOutputs merges per-txid BEEFs with
                // Beef.mergeBeef): merge_beef dedups bumps by (height, root)
                // and re-derives bump indices — the previous hand-rolled
                // bump-offset concat kept duplicate bumps, and its linear
                // txid dedup could keep a tx pointing at the wrong copy.
                let mut merged_beef = Beef::new(BEEF_V2);
                for txid in &unique_txids {
                    let beef_bytes_opt = crate::storage::beef::get_valid_beef_for_txid(
                        storage_provider.as_ref(),
                        txid,
                        beef_trust_self,
                        &known_txids,
                    )
                    .await
                    .map_err(to_sdk_error)?;

                    if let Some(beef_bytes) = beef_bytes_opt {
                        merged_beef
                            .merge_beef_from_binary(&beef_bytes)
                            .map_err(|e| {
                                to_sdk_error(crate::error::WalletError::Internal(format!(
                                    "Failed to merge BEEF for {txid}: {e}"
                                )))
                            })?;
                    }
                }

                if !merged_beef.txs.is_empty() {
                    // Ancestors before children on the wire (TS Beef.toBinary
                    // sorts implicitly; the Rust SDK's does not).
                    merged_beef.sort_txs();
                    let mut buf = Vec::new();
                    merged_beef.to_binary(&mut buf).map_err(|e| {
                        to_sdk_error(crate::error::WalletError::Internal(format!(
                            "Failed to serialize merged BEEF: {e}"
                        )))
                    })?;
                    result.beef = Some(buf);
                }
            }
        }

        // Merge BEEF into BeefParty and verify returned txid-only
        if let Some(ref mut beef_bytes) = result.beef {
            let beef_lock = self.beef.lock().await;
            let verified = crate::wallet::beef_helpers::verify_returned_txid_only_beef(
                beef_bytes,
                &beef_lock,
                self.return_txid_only,
            )?;
            *beef_bytes = verified;
        }

        Ok(result)
    }

    async fn list_certificates(
        &self,
        args: ListCertificatesArgs,
        originator: Option<&str>,
    ) -> Result<ListCertificatesResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_list_certificates_args(&args)?;

        let auth = self.auth_id();
        self.storage
            .list_certificates(&auth, &args)
            .await
            .map_err(to_sdk_error)
    }

    // -----------------------------------------------------------------------
    // RELINQUISH methods (2) -- delegate to storage
    // -----------------------------------------------------------------------

    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, SdkWalletError> {
        let _spend_guard = self
            .storage
            .acquire_spend_lock()
            .await
            .map_err(to_sdk_error)?;
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_relinquish_output_args(&args)?;

        let auth = self.auth_id();
        self.storage
            .relinquish_output(&auth, &args)
            .await
            .map(|_| bsv::wallet::interfaces::RelinquishOutputResult { relinquished: true })
            .map_err(to_sdk_error)
    }

    async fn relinquish_certificate(
        &self,
        args: RelinquishCertificateArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_relinquish_certificate_args(&args)?;

        let auth = self.auth_id();
        self.storage
            .relinquish_certificate(&auth, &args)
            .await
            .map(|_| bsv::wallet::interfaces::RelinquishCertificateResult { relinquished: true })
            .map_err(to_sdk_error)
    }

    // -----------------------------------------------------------------------
    // CERTIFICATE methods (4) -- Plans 04/05
    // -----------------------------------------------------------------------

    async fn acquire_certificate(
        &self,
        args: AcquireCertificateArgs,
        originator: Option<&str>,
    ) -> Result<Certificate, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_acquire_certificate_args(&args)?;

        let auth = self.auth_id();

        let result = match args.acquisition_protocol {
            AcquisitionProtocol::Direct => crate::wallet::certificates::acquire_direct_certificate(
                &self.storage,
                self,
                &auth,
                &args,
            )
            .await
            .map_err(to_sdk_error)?,
            AcquisitionProtocol::Issuance => {
                crate::wallet::certificates::acquire_issuance_certificate(
                    &self.storage,
                    self,
                    &auth,
                    &args,
                )
                .await
                .map_err(to_sdk_error)?
            }
        };

        // Convert AcquireCertificateResult to SDK Certificate
        let subject_pk = PublicKey::from_string(&result.subject)
            .map_err(|e| SdkWalletError::Internal(format!("Invalid subject key: {e}")))?;
        let certifier_pk = PublicKey::from_string(&result.certifier)
            .map_err(|e| SdkWalletError::Internal(format!("Invalid certifier key: {e}")))?;

        Ok(Certificate {
            cert_type: args.cert_type,
            serial_number: args
                .serial_number
                .unwrap_or(bsv::wallet::interfaces::SerialNumber([0u8; 32])),
            subject: subject_pk,
            certifier: certifier_pk,
            revocation_outpoint: Some(result.revocation_outpoint),
            fields: Some(result.fields),
            signature: args.signature,
        })
    }

    async fn prove_certificate(
        &self,
        args: ProveCertificateArgs,
        originator: Option<&str>,
    ) -> Result<ProveCertificateResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_prove_certificate_args(&args)?;

        let auth = self.auth_id();
        crate::wallet::certificates::prove_certificate(&self.storage, self, &auth, &args)
            .await
            .map_err(to_sdk_error)
    }

    async fn discover_by_identity_key(
        &self,
        args: DiscoverByIdentityKeyArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_discover_by_identity_key_args(&args)?;

        let resolver = self
            .lookup_resolver
            .as_ref()
            .ok_or_else(|| SdkWalletError::Internal("No lookup resolver configured".to_string()))?;

        crate::wallet::discovery::discover_by_identity_key(
            &self.settings_manager,
            resolver,
            &self.overlay_cache,
            &args,
        )
        .await
        .map_err(to_sdk_error)
    }

    async fn discover_by_attributes(
        &self,
        args: DiscoverByAttributesArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, SdkWalletError> {
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_discover_by_attributes_args(&args)?;

        let resolver = self
            .lookup_resolver
            .as_ref()
            .ok_or_else(|| SdkWalletError::Internal("No lookup resolver configured".to_string()))?;

        crate::wallet::discovery::discover_by_attributes(
            &self.settings_manager,
            resolver,
            &self.overlay_cache,
            &args,
        )
        .await
        .map_err(to_sdk_error)
    }
}

// ---------------------------------------------------------------------------
// ContextualWallet implementation
// ---------------------------------------------------------------------------

/// The context-taking action variants. These hold the full createAction /
/// signAction pipelines; `WalletInterface::create_action` / `sign_action`
/// delegate here with [`SigningContext::itself`].
#[async_trait]
impl ContextualWallet for Wallet {
    async fn create_action_in(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<CreateActionResult, SdkWalletError> {
        // The spend lock is taken inside the signer pipeline, scoped to the
        // storage allocation and process steps only — holding it here for
        // the whole pipeline would serialize every spend-path caller behind
        // signing ceremonies and network broadcast.
        tracing::debug!(description = %args.description, "createAction starting");
        self.validate_originator(originator).map_err(to_sdk_error)?;
        bsv::wallet::validation::validate_create_action_args(&args)?;

        // Merge wallet defaults into options
        let mut options = args.options.clone().unwrap_or_default();

        // Merge trust_self from wallet defaults if not set
        if options.trust_self.is_none() {
            options.trust_self = self.trust_self.clone();
        }

        // Merge auto_known_txids: add wallet's BeefParty known txids
        if self.auto_known_txids {
            let mut beef_lock = self.beef.lock().await;
            let known = crate::wallet::beef_helpers::get_known_txids(
                &mut beef_lock,
                options.known_txids.as_deref(),
            );
            options.known_txids = (!known.is_empty()).then_some(known);
        }

        // Build validated args for signer
        let sign_and_process = *options.sign_and_process;
        let no_send = *options.no_send;
        let accept_delayed = *options.accept_delayed_broadcast;

        let valid_args = crate::signer::types::ValidCreateActionArgs {
            description: args.description,
            inputs: args
                .inputs
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|input| {
                    let parts: Vec<&str> = input.outpoint.rsplitn(2, '.').collect();
                    let (vout_str, txid_str) = if parts.len() == 2 {
                        (parts[0], parts[1])
                    } else {
                        ("0", input.outpoint.as_str())
                    };
                    crate::signer::types::ValidCreateActionInput {
                        outpoint: crate::signer::types::OutpointInfo {
                            txid: txid_str.to_string(),
                            vout: vout_str.parse().unwrap_or(0),
                        },
                        input_description: input.input_description.clone(),
                        unlocking_script: input.unlocking_script.clone(),
                        unlocking_script_length: input.unlocking_script_length.unwrap_or(0)
                            as usize,
                        sequence_number: input.sequence_number.unwrap_or(0xffffffff),
                    }
                })
                .collect(),
            outputs: args.outputs.unwrap_or_default(),
            lock_time: args.lock_time.unwrap_or(0),
            version: args.version.unwrap_or(1),
            labels: args.labels.unwrap_or_default(),
            options: options.clone(),
            input_beef: args.input_beef,
            is_new_tx: true,
            is_sign_action: !sign_and_process,
            is_no_send: no_send,
            is_delayed: accept_delayed,
            is_send_with: !options.send_with.as_deref().unwrap_or(&[]).is_empty(),
        };

        let signer_result = self
            .signer
            .create_action(valid_args, ctx)
            .await
            .map_err(to_sdk_error)?;

        // Convert signer result to SDK result
        let mut result = CreateActionResult {
            txid: signer_result.txid,
            tx: signer_result.tx,
            no_send_change: (!signer_result.no_send_change.is_empty())
                .then_some(signer_result.no_send_change),
            send_with_results: (!signer_result.send_with_results.is_empty())
                .then_some(signer_result.send_with_results),
            signable_transaction: match signer_result.signable_transaction {
                Some(st) => {
                    // The SDK's `SignableTransaction.reference` is RAW bytes
                    // that `bytes_as_base64` re-encodes on the wire, and the
                    // toolbox's reference is already base64 TEXT
                    // (`random_bytes_base64(12)`, stored verbatim on the
                    // transaction row). Handing the text's UTF-8 bytes to the
                    // SDK double-encoded the wire value (base64 of base64) —
                    // a TS client saw a different string than a TS toolbox
                    // would return, and `abortAction`'s re-encoding lookup
                    // could never match the stored row. Decode to the raw
                    // bytes so the wire carries exactly the stored reference.
                    use base64::Engine as _;
                    let reference = base64::engine::general_purpose::STANDARD
                        .decode(&st.reference)
                        .map_err(|e| {
                            SdkWalletError::Internal(format!(
                                "createAction produced a non-base64 reference: {e}"
                            ))
                        })?;
                    Some(bsv::wallet::interfaces::SignableTransaction {
                        reference,
                        tx: st.tx,
                    })
                }
                None => None,
            },
        };

        // Merge result beef into BeefParty
        if let Some(ref tx_bytes) = result.tx {
            let mut beef_lock = self.beef.lock().await;
            if let Ok(beef) =
                bsv::transaction::beef::Beef::from_binary(&mut std::io::Cursor::new(tx_bytes))
            {
                if let Err(e) = beef_lock.beef.merge_beef(&beef) {
                    tracing::warn!("BeefParty merge failed: {e}");
                }
            }
        }

        // Verify returned txid-only if applicable
        if let Some(ref mut tx_bytes) = result.tx {
            let beef_lock = self.beef.lock().await;
            let verified = crate::wallet::beef_helpers::verify_returned_txid_only_atomic_beef(
                tx_bytes,
                &beef_lock,
                self.return_txid_only,
                None,
            )?;
            *tx_bytes = verified;
        }

        // Check for unsuccessful results
        crate::wallet::error_helpers::throw_if_any_unsuccessful_create_actions(&result)?;

        tracing::info!(txid = ?result.txid, "createAction completed");
        Ok(result)
    }

    async fn sign_action_in(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<SignActionResult, SdkWalletError> {
        // Spend lock handling lives in the signer pipeline (see
        // create_action_in) — never held across signing or broadcast.
        tracing::debug!("signAction starting");
        self.validate_originator(originator).map_err(to_sdk_error)?;
        // Reference-parity validation (TS `validateSignActionArgs`): `spends`
        // MAY be empty — signAction legitimately completes an action whose
        // every input is wallet-signed (the caller supplied no inputs of its
        // own). The vendored SDK's `validate_sign_action_args` rejects an
        // empty map ("spends: must be at least one spend"), a rule the TS
        // reference has never had, so the checks it does share are applied
        // here instead of calling it.
        if args.reference.is_empty() {
            return Err(SdkWalletError::InvalidParameter(
                "reference: must be non-empty".to_string(),
            ));
        }
        for spend in args.spends.values() {
            if spend.unlocking_script.is_empty() {
                return Err(SdkWalletError::InvalidParameter(
                    "unlocking_script: must be non-empty".to_string(),
                ));
            }
        }

        // `args.reference` is the RAW bytes the wire's Base64String decoded to
        // (SDK `bytes_as_base64`), while createAction stored the base64 TEXT as
        // the transaction's reference. Re-encode for the storage lookup — the
        // same bridge `storage::methods::abort_action` crosses; a lossy UTF-8
        // read here turned the raw bytes into garbage and made a wire-level
        // signAction unable to find the action its own createAction had built.
        let reference = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&args.reference)
        };
        let raw_options = &args.options;
        let options = raw_options.clone().unwrap_or_default();

        // Derive Option<bool> flags from raw options, preserving None when the
        // caller didn't specify a value.  This enables mergePriorOptions in
        // sign_action.rs: None means "inherit from createAction", Some(v) means
        // "caller explicitly set this".
        let (is_no_send, is_delayed, is_send_with) = match raw_options {
            Some(opts) => (
                opts.no_send.0,
                // `isDelayed` IS `acceptDelayedBroadcast` (TS
                // `validateSignActionArgs`). It was negated here until the
                // funded conformance recording caught it: an explicit
                // `acceptDelayedBroadcast=true` broadcast immediately and
                // `=false` suppressed the broadcast — exactly inverted. The
                // createAction path above always mapped it straight.
                opts.accept_delayed_broadcast.0,
                if opts.send_with.as_deref().unwrap_or(&[]).is_empty() {
                    None
                } else {
                    Some(true)
                },
            ),
            None => (None, None, None),
        };

        let valid_args = crate::signer::types::ValidSignActionArgs {
            reference: reference.clone(),
            spends: args.spends,
            options,
            is_new_tx: true,
            is_no_send,
            is_delayed,
            is_send_with,
        };

        let signer_result = self
            .signer
            .sign_action(valid_args, ctx)
            .await
            .map_err(to_sdk_error)?;

        let mut result = SignActionResult {
            txid: signer_result.txid,
            tx: signer_result.tx,
            send_with_results: (!signer_result.send_with_results.is_empty())
                .then_some(signer_result.send_with_results),
        };

        // Verify returned txid-only if applicable
        if let Some(ref mut tx_bytes) = result.tx {
            let beef_lock = self.beef.lock().await;
            let verified = crate::wallet::beef_helpers::verify_returned_txid_only_atomic_beef(
                tx_bytes,
                &beef_lock,
                self.return_txid_only,
                None,
            )?;
            *tx_bytes = verified;
        }

        // Check for unsuccessful results
        crate::wallet::error_helpers::throw_if_any_unsuccessful_sign_actions(&result)?;

        tracing::info!(txid = ?result.txid, "signAction completed");
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// WalletArc -- Clone-able newtype for Wallet
// ---------------------------------------------------------------------------
//
// AuthMiddlewareFactory requires W: Clone, but Wallet cannot implement Clone
// (WalletStorageManager contains Mutex). We use a newtype around Arc<Wallet>
// that is Clone and implements WalletInterface by delegating to the inner Wallet.
// This satisfies orphan rules (WalletArc is a local type).

/// Clone-able wrapper around `Arc<Wallet>` implementing `WalletInterface`.
///
/// Needed because `Wallet` cannot implement `Clone` (contains `Mutex`),
/// but `AuthMiddlewareFactory` requires `W: WalletInterface + Clone`.
#[derive(Clone)]
pub struct WalletArc(pub Arc<Wallet>);

impl WalletArc {
    /// Create a new WalletArc from an existing `Arc<Wallet>`.
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self(wallet)
    }
}

#[async_trait]
impl WalletInterface for WalletArc {
    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
        originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, SdkWalletError> {
        self.0.as_ref().get_public_key(args, originator).await
    }

    async fn encrypt(
        &self,
        args: EncryptArgs,
        originator: Option<&str>,
    ) -> Result<EncryptResult, SdkWalletError> {
        self.0.as_ref().encrypt(args, originator).await
    }

    async fn decrypt(
        &self,
        args: DecryptArgs,
        originator: Option<&str>,
    ) -> Result<DecryptResult, SdkWalletError> {
        self.0.as_ref().decrypt(args, originator).await
    }

    async fn create_hmac(
        &self,
        args: CreateHmacArgs,
        originator: Option<&str>,
    ) -> Result<CreateHmacResult, SdkWalletError> {
        self.0.as_ref().create_hmac(args, originator).await
    }

    async fn verify_hmac(
        &self,
        args: VerifyHmacArgs,
        originator: Option<&str>,
    ) -> Result<VerifyHmacResult, SdkWalletError> {
        self.0.as_ref().verify_hmac(args, originator).await
    }

    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
        originator: Option<&str>,
    ) -> Result<CreateSignatureResult, SdkWalletError> {
        self.0.as_ref().create_signature(args, originator).await
    }

    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
        originator: Option<&str>,
    ) -> Result<VerifySignatureResult, SdkWalletError> {
        self.0.as_ref().verify_signature(args, originator).await
    }

    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, SdkWalletError> {
        self.0
            .as_ref()
            .reveal_counterparty_key_linkage(args, originator)
            .await
    }

    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, SdkWalletError> {
        self.0
            .as_ref()
            .reveal_specific_key_linkage(args, originator)
            .await
    }

    async fn is_authenticated(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, SdkWalletError> {
        self.0.as_ref().is_authenticated(originator).await
    }

    async fn wait_for_authentication(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, SdkWalletError> {
        self.0.as_ref().wait_for_authentication(originator).await
    }

    async fn get_height(
        &self,
        originator: Option<&str>,
    ) -> Result<GetHeightResult, SdkWalletError> {
        self.0.as_ref().get_height(originator).await
    }

    async fn get_header_for_height(
        &self,
        args: GetHeaderArgs,
        originator: Option<&str>,
    ) -> Result<GetHeaderResult, SdkWalletError> {
        self.0
            .as_ref()
            .get_header_for_height(args, originator)
            .await
    }

    async fn get_network(
        &self,
        originator: Option<&str>,
    ) -> Result<GetNetworkResult, SdkWalletError> {
        self.0.as_ref().get_network(originator).await
    }

    async fn get_version(
        &self,
        originator: Option<&str>,
    ) -> Result<GetVersionResult, SdkWalletError> {
        self.0.as_ref().get_version(originator).await
    }

    async fn create_action(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
    ) -> Result<CreateActionResult, SdkWalletError> {
        self.0.as_ref().create_action(args, originator).await
    }

    async fn sign_action(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
    ) -> Result<SignActionResult, SdkWalletError> {
        self.0.as_ref().sign_action(args, originator).await
    }

    async fn internalize_action(
        &self,
        args: InternalizeActionArgs,
        originator: Option<&str>,
    ) -> Result<InternalizeActionResult, SdkWalletError> {
        self.0.as_ref().internalize_action(args, originator).await
    }

    async fn abort_action(
        &self,
        args: AbortActionArgs,
        originator: Option<&str>,
    ) -> Result<AbortActionResult, SdkWalletError> {
        self.0.as_ref().abort_action(args, originator).await
    }

    async fn list_actions(
        &self,
        args: ListActionsArgs,
        originator: Option<&str>,
    ) -> Result<ListActionsResult, SdkWalletError> {
        self.0.as_ref().list_actions(args, originator).await
    }

    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        originator: Option<&str>,
    ) -> Result<ListOutputsResult, SdkWalletError> {
        self.0.as_ref().list_outputs(args, originator).await
    }

    async fn list_certificates(
        &self,
        args: ListCertificatesArgs,
        originator: Option<&str>,
    ) -> Result<ListCertificatesResult, SdkWalletError> {
        self.0.as_ref().list_certificates(args, originator).await
    }

    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, SdkWalletError> {
        self.0.as_ref().relinquish_output(args, originator).await
    }

    async fn relinquish_certificate(
        &self,
        args: RelinquishCertificateArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, SdkWalletError> {
        self.0
            .as_ref()
            .relinquish_certificate(args, originator)
            .await
    }

    async fn acquire_certificate(
        &self,
        args: AcquireCertificateArgs,
        originator: Option<&str>,
    ) -> Result<Certificate, SdkWalletError> {
        self.0.as_ref().acquire_certificate(args, originator).await
    }

    async fn prove_certificate(
        &self,
        args: ProveCertificateArgs,
        originator: Option<&str>,
    ) -> Result<ProveCertificateResult, SdkWalletError> {
        self.0.as_ref().prove_certificate(args, originator).await
    }

    async fn discover_by_identity_key(
        &self,
        args: DiscoverByIdentityKeyArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, SdkWalletError> {
        self.0
            .as_ref()
            .discover_by_identity_key(args, originator)
            .await
    }

    async fn discover_by_attributes(
        &self,
        args: DiscoverByAttributesArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, SdkWalletError> {
        self.0
            .as_ref()
            .discover_by_attributes(args, originator)
            .await
    }
}

// WalletArc is the shape auth middleware holds (`W: WalletInterface + Clone`),
// i.e. exactly the transport that authenticates callers — so it must be able
// to hand the caller through too.
#[async_trait]
impl ContextualWallet for WalletArc {
    async fn create_action_in(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<CreateActionResult, SdkWalletError> {
        self.0
            .as_ref()
            .create_action_in(args, originator, ctx)
            .await
    }

    async fn sign_action_in(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<SignActionResult, SdkWalletError> {
        self.0.as_ref().sign_action_in(args, originator, ctx).await
    }
}

#[cfg(test)]
mod sweep_derivation_tests {
    use super::{payment_derivation_bytes, random_base64};
    use base64::Engine as _;

    /// #4 regression: sweep_to locks a BRC-29 output using the base64 derivation
    /// STRING as the key-id, while `Payment` carries raw derivation bytes that
    /// storage re-encodes to base64 when persisting. The stored spend key-id must
    /// equal the base64 string used to lock, or the output is unspendable.
    #[test]
    fn lock_and_spend_key_ids_match() {
        let key_id = random_base64(8);

        // The bytes handed to Payment, then re-encoded by storage_internalize_action.
        let payment_bytes = payment_derivation_bytes(&key_id).unwrap();
        let stored = base64::engine::general_purpose::STANDARD.encode(&payment_bytes);
        assert_eq!(
            stored, key_id,
            "spend key-id (base64 of Payment bytes) must equal the lock key-id"
        );

        // Guard against the original bug: passing the base64 string's own UTF-8
        // bytes makes storage emit base64-of-base64, desyncing lock and spend.
        let buggy_stored =
            base64::engine::general_purpose::STANDARD.encode(key_id.clone().into_bytes());
        assert_ne!(
            buggy_stored, key_id,
            "into_bytes double-encodes; that path must not be used"
        );
    }
}
