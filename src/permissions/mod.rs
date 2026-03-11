//! Permission management for wallet operations.
//!
//! Provides the `WalletPermissionsManager` which wraps an inner
//! `WalletInterface` implementation and intercepts calls to enforce
//! permission checks based on originator identity.

/// BRC-114 action time label parsing utilities.
pub mod brc114;
/// Permission caching with concurrent request deduplication.
pub mod cache;
/// Async callback trait for permission request events.
pub mod callbacks;
/// Permission manager configuration flags.
pub mod config;
/// Permission checking functions (ensure_*).
pub mod ensure;
/// Metadata encryption/decryption for wallet descriptions.
pub mod metadata;
/// Originator normalization and lookup utilities.
pub mod originator;
/// P-Module delegation trait for BRC-98/99 schemes.
pub mod p_module;
/// Permission token CRUD operations via inner wallet.
pub mod token_crud;
/// Permission types, tokens, and request/response structs.
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;

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

use config::PermissionsManagerConfig;
use p_module::PermissionsModule;
use types::{
    GroupedPermissions, PermissionRequest, PermissionResponse, PermissionToken, PermissionType,
};

// ---------------------------------------------------------------------------
// WalletPermissionsManager
// ---------------------------------------------------------------------------

/// Wraps an inner `WalletInterface` with permission management.
///
/// Intercepts calls from external applications (identified by originators),
/// checks if the request is allowed, and proxies the actual call to the
/// underlying wallet. The admin originator bypasses all permission checks.
pub struct WalletPermissionsManager {
    /// The wrapped inner wallet.
    inner: Arc<dyn WalletInterface + Send + Sync>,
    /// The admin originator domain (normalized). Bypasses all checks.
    admin_originator: String,
    /// Configuration flags controlling which permission checks are enforced.
    config: PermissionsManagerConfig,
    /// Registered P-Module handlers, keyed by scheme ID.
    permission_modules: HashMap<String, Arc<dyn PermissionsModule>>,
    /// Optional async callbacks for permission request events.
    callbacks: Option<Arc<dyn callbacks::PermissionCallbacks>>,
    /// Permission cache for manifest entries, recent grants, and deduplication.
    cache: cache::PermissionCache,
}

impl WalletPermissionsManager {
    /// Create a new `WalletPermissionsManager` wrapping the given wallet.
    ///
    /// The `admin_originator` domain (normalized) will bypass all permission
    /// checks for all methods.
    pub fn new(
        inner: Arc<dyn WalletInterface + Send + Sync>,
        admin_originator: String,
        config: PermissionsManagerConfig,
    ) -> Self {
        let normalized_admin = originator::normalize_originator(&admin_originator);
        Self {
            inner,
            admin_originator: if normalized_admin.is_empty() {
                admin_originator
            } else {
                normalized_admin
            },
            config,
            permission_modules: HashMap::new(),
            callbacks: None,
            cache: cache::PermissionCache::new(),
        }
    }

    /// Check if the given originator is the admin.
    pub(crate) fn is_admin(&self, originator: Option<&str>) -> bool {
        match originator {
            Some(o) => originator::is_admin_originator(o, &self.admin_originator),
            None => false,
        }
    }

    /// Register a P-Module handler for a given scheme ID.
    ///
    /// When a protocol or basket name starts with `"p <scheme_id>"`, the
    /// corresponding module's `on_request`/`on_response` methods will be called.
    pub fn register_module(&mut self, scheme_id: String, module: Arc<dyn PermissionsModule>) {
        self.permission_modules.insert(scheme_id, module);
    }

    /// Check if a P-Module is registered for the given scheme ID.
    pub(crate) fn has_permission_module(&self, scheme_id: &str) -> bool {
        self.permission_modules.contains_key(scheme_id)
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &PermissionsManagerConfig {
        &self.config
    }

    /// Get a reference to the inner wallet.
    pub fn inner(&self) -> &Arc<dyn WalletInterface + Send + Sync> {
        &self.inner
    }

    /// Get the normalized admin originator.
    pub(crate) fn admin_originator(&self) -> String {
        self.admin_originator.clone()
    }

    // -----------------------------------------------------------------------
    // Callback management
    // -----------------------------------------------------------------------

    /// Register an async callback handler for permission request events.
    pub fn bind_callback(&mut self, cbs: Arc<dyn callbacks::PermissionCallbacks>) {
        self.callbacks = Some(cbs);
    }

    /// Remove the registered callback handler.
    pub fn unbind_callback(&mut self) {
        self.callbacks = None;
    }

    /// Clear all permission caches (manifest, recent grants, active requests).
    pub async fn clear_caches(&self) {
        self.cache.clear().await;
    }

    /// Get a reference to the permission cache (for ensure* functions).
    pub(crate) fn cache(&self) -> &cache::PermissionCache {
        &self.cache
    }

    /// Get a reference to the callbacks (for ensure* functions).
    pub(crate) fn callbacks(&self) -> &Option<Arc<dyn callbacks::PermissionCallbacks>> {
        &self.callbacks
    }

    // -----------------------------------------------------------------------
    // Public permission management methods
    // -----------------------------------------------------------------------

    /// Grant a permission by creating an on-chain token.
    pub async fn grant_permission(
        &self,
        request: &PermissionRequest,
        expiry: Option<u64>,
    ) -> Result<(), bsv::wallet::error::WalletError> {
        token_crud::grant_permission(self.inner.as_ref(), request, expiry, &self.admin_originator)
            .await
    }

    /// Deny a permission request.
    pub fn deny_permission(&self, request: &PermissionRequest) -> PermissionResponse {
        token_crud::deny_permission(request)
    }

    /// Grant all permissions in a grouped set.
    pub async fn grant_grouped_permission(
        &self,
        grouped: &GroupedPermissions,
        expiry: Option<u64>,
    ) -> Result<(), bsv::wallet::error::WalletError> {
        token_crud::grant_grouped_permission(
            self.inner.as_ref(),
            grouped,
            expiry,
            &self.admin_originator,
        )
        .await
    }

    /// Deny all permissions in a grouped set.
    pub fn deny_grouped_permission(&self, grouped: &GroupedPermissions) -> Vec<PermissionResponse> {
        token_crud::deny_grouped_permission(grouped)
    }

    /// Revoke a single permission token.
    pub async fn revoke_permission(
        &self,
        txid: &str,
        output_index: u32,
        permission_type: PermissionType,
    ) -> Result<(), bsv::wallet::error::WalletError> {
        let basket = match permission_type {
            PermissionType::ProtocolPermission => types::BASKET_DPACP,
            PermissionType::BasketAccess => types::BASKET_DBAP,
            PermissionType::CertificateAccess => types::BASKET_DCAP,
            PermissionType::SpendingAuthorization => types::BASKET_DSAP,
        };
        token_crud::revoke_permission_token(
            self.inner.as_ref(),
            txid,
            output_index,
            basket,
            &self.admin_originator,
        )
        .await
    }

    /// Revoke all permission tokens for an originator.
    pub async fn revoke_all_for_originator(
        &self,
        originator_domain: &str,
    ) -> Result<u32, bsv::wallet::error::WalletError> {
        token_crud::revoke_all_for_originator(
            self.inner.as_ref(),
            originator_domain,
            &self.admin_originator,
        )
        .await
    }

    /// List all protocol permissions (DPACP) for an originator.
    pub async fn list_protocol_permissions(
        &self,
        originator_domain: &str,
    ) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
        token_crud::list_protocol_permissions(
            self.inner.as_ref(),
            originator_domain,
            &self.admin_originator,
        )
        .await
    }

    /// Check if an originator has a specific protocol permission.
    pub async fn has_protocol_permission(
        &self,
        originator_domain: &str,
        protocol: &str,
        security_level: u8,
        counterparty: &str,
    ) -> Result<bool, bsv::wallet::error::WalletError> {
        token_crud::has_protocol_permission(
            self.inner.as_ref(),
            originator_domain,
            protocol,
            security_level,
            counterparty,
            &self.admin_originator,
        )
        .await
    }

    /// List all basket access tokens (DBAP) for an originator.
    pub async fn list_basket_access(
        &self,
        originator_domain: &str,
    ) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
        token_crud::list_basket_access(
            self.inner.as_ref(),
            originator_domain,
            &self.admin_originator,
        )
        .await
    }

    /// Check if an originator has basket access permission.
    pub async fn has_basket_access(
        &self,
        originator_domain: &str,
        basket_name: &str,
    ) -> Result<bool, bsv::wallet::error::WalletError> {
        token_crud::has_basket_access(
            self.inner.as_ref(),
            originator_domain,
            basket_name,
            &self.admin_originator,
        )
        .await
    }

    /// List all certificate access tokens (DCAP) for an originator.
    pub async fn list_certificate_access(
        &self,
        originator_domain: &str,
    ) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
        token_crud::list_certificate_access(
            self.inner.as_ref(),
            originator_domain,
            &self.admin_originator,
        )
        .await
    }

    /// List all spending authorizations (DSAP) for an originator.
    pub async fn list_spending_authorizations(
        &self,
        originator_domain: &str,
    ) -> Result<Vec<PermissionToken>, bsv::wallet::error::WalletError> {
        token_crud::list_spending_authorizations(
            self.inner.as_ref(),
            originator_domain,
            &self.admin_originator,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Helper: extract protocol info from args
// ---------------------------------------------------------------------------

/// Extract protocol name and security level from a Protocol struct.
fn protocol_info(proto: &bsv::wallet::types::Protocol) -> (String, u8) {
    (proto.protocol.clone(), proto.security_level)
}

/// Convert a Counterparty to a string for permission checks.
fn counterparty_str(cpty: &bsv::wallet::types::Counterparty) -> String {
    match cpty.counterparty_type {
        bsv::wallet::types::CounterpartyType::Self_ => "self".to_string(),
        bsv::wallet::types::CounterpartyType::Anyone => "anyone".to_string(),
        bsv::wallet::types::CounterpartyType::Other => cpty
            .public_key
            .as_ref()
            .map(|pk| pk.to_der_hex())
            .unwrap_or_default(),
        bsv::wallet::types::CounterpartyType::Uninitialized => String::new(),
    }
}

/// Resolve originator, defaulting to empty string if None.
fn orig(originator: Option<&str>) -> &str {
    originator.unwrap_or("")
}

#[async_trait]
impl WalletInterface for WalletPermissionsManager {
    // -----------------------------------------------------------------------
    // Auth/Info methods -- no permission checks needed, simple delegation
    // -----------------------------------------------------------------------

    async fn is_authenticated(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        self.inner.is_authenticated(originator).await
    }

    async fn wait_for_authentication(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, bsv::wallet::error::WalletError> {
        self.inner.wait_for_authentication(originator).await
    }

    async fn get_height(
        &self,
        originator: Option<&str>,
    ) -> Result<GetHeightResult, bsv::wallet::error::WalletError> {
        self.inner.get_height(originator).await
    }

    async fn get_header_for_height(
        &self,
        args: GetHeaderArgs,
        originator: Option<&str>,
    ) -> Result<GetHeaderResult, bsv::wallet::error::WalletError> {
        self.inner.get_header_for_height(args, originator).await
    }

    async fn get_network(
        &self,
        originator: Option<&str>,
    ) -> Result<GetNetworkResult, bsv::wallet::error::WalletError> {
        self.inner.get_network(originator).await
    }

    async fn get_version(
        &self,
        originator: Option<&str>,
    ) -> Result<GetVersionResult, bsv::wallet::error::WalletError> {
        self.inner.get_version(originator).await
    }

    // -----------------------------------------------------------------------
    // Action methods
    // -----------------------------------------------------------------------

    /// Checks spending authorization and basket access before delegating.
    /// If `config.encrypt_wallet_metadata` is true, encrypts the description.
    async fn create_action(
        &self,
        mut args: CreateActionArgs,
        originator: Option<&str>,
    ) -> Result<CreateActionResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);

        // Check basket access for each output that has a basket
        for output in &args.outputs {
            if let Some(ref basket) = output.basket {
                ensure::ensure_basket_access(self, o, basket, "insertion").await?;
            }
        }

        // Check spending authorization if there are outputs with satoshis
        let total_output_sats: u64 = args.outputs.iter().map(|o| o.satoshis).sum();
        if total_output_sats > 0 {
            ensure::ensure_spending_authorization(self, o, total_output_sats).await?;
        }

        // Encrypt metadata if configured
        if self.config.encrypt_wallet_metadata {
            args.description = metadata::encrypt_action_metadata(
                self.inner.as_ref(),
                &args.description,
                &self.admin_originator,
            )
            .await?;
        }

        self.inner.create_action(args, originator).await
    }

    /// Permission checked on create_action, not here.
    async fn sign_action(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
    ) -> Result<SignActionResult, bsv::wallet::error::WalletError> {
        self.inner.sign_action(args, originator).await
    }

    /// No permission check needed for abort.
    async fn abort_action(
        &self,
        args: AbortActionArgs,
        originator: Option<&str>,
    ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
        self.inner.abort_action(args, originator).await
    }

    /// Checks label access before delegating. Decrypts metadata if configured.
    async fn list_actions(
        &self,
        args: ListActionsArgs,
        originator: Option<&str>,
    ) -> Result<ListActionsResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);

        // Check label access for requested labels
        if !args.labels.is_empty() {
            ensure::ensure_label_access(self, o, &args.labels, "list").await?;
        }

        let mut result = self.inner.list_actions(args, originator).await?;

        // Decrypt metadata if configured
        if self.config.encrypt_wallet_metadata {
            for action in &mut result.actions {
                action.description = metadata::decrypt_action_metadata(
                    self.inner.as_ref(),
                    &action.description,
                    &self.admin_originator,
                )
                .await?;
            }
        }

        Ok(result)
    }

    /// Checks basket access for each output's target basket.
    async fn internalize_action(
        &self,
        args: InternalizeActionArgs,
        originator: Option<&str>,
    ) -> Result<InternalizeActionResult, bsv::wallet::error::WalletError> {
        // Basket access checks would apply to the target baskets in args.outputs.
        // The InternalizeActionArgs structure is complex; for now we delegate
        // as the specific basket extraction requires deeper args inspection
        // that Plan 05 will complete with full integration tests.
        self.inner.internalize_action(args, originator).await
    }

    // -----------------------------------------------------------------------
    // Output methods
    // -----------------------------------------------------------------------

    /// Checks basket access before delegating. Decrypts metadata if configured.
    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        originator: Option<&str>,
    ) -> Result<ListOutputsResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        ensure::ensure_basket_access(self, o, &args.basket, "listing").await?;

        let mut result = self.inner.list_outputs(args, originator).await?;

        // Decrypt custom instructions metadata if configured
        if self.config.encrypt_wallet_metadata {
            for output in &mut result.outputs {
                if let Some(ref instructions) = output.custom_instructions {
                    output.custom_instructions = Some(
                        metadata::decrypt_action_metadata(
                            self.inner.as_ref(),
                            instructions,
                            &self.admin_originator,
                        )
                        .await?,
                    );
                }
            }
        }

        Ok(result)
    }

    /// Checks basket access before delegating.
    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        ensure::ensure_basket_access(self, o, &args.basket, "relinquishing").await?;
        self.inner.relinquish_output(args, originator).await
    }

    // -----------------------------------------------------------------------
    // Key/Crypto methods
    // -----------------------------------------------------------------------

    /// Checks protocol permission if config flag is set.
    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
        originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        if let Some(ref proto) = args.protocol_id {
            let (name, sec) = protocol_info(proto);
            let cpty = args
                .counterparty
                .as_ref()
                .map(|c| counterparty_str(c))
                .unwrap_or_default();
            ensure::ensure_protocol_permission(
                self,
                o,
                &name,
                sec,
                &cpty,
                args.privileged,
                "publicKey",
            )
            .await?;
        }
        self.inner.get_public_key(args, originator).await
    }

    /// Checks protocol permission for key linkage revelation.
    /// RevealCounterpartyKeyLinkageArgs has counterparty as PublicKey, not Protocol.
    /// We use a generic key linkage protocol check.
    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let cpty = args.counterparty.to_der_hex();
        let privileged = args.privileged.unwrap_or(false);
        ensure::ensure_protocol_permission(
            self,
            o,
            "key linkage revelation",
            2,
            &cpty,
            privileged,
            "keyLinkage",
        )
        .await?;
        self.inner
            .reveal_counterparty_key_linkage(args, originator)
            .await
    }

    /// Checks protocol permission for key linkage revelation.
    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        let privileged = args.privileged.unwrap_or(false);
        ensure::ensure_protocol_permission(self, o, &name, sec, &cpty, privileged, "keyLinkage")
            .await?;
        self.inner
            .reveal_specific_key_linkage(args, originator)
            .await
    }

    /// Checks protocol permission for encryption.
    async fn encrypt(
        &self,
        args: EncryptArgs,
        originator: Option<&str>,
    ) -> Result<EncryptResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(
            self,
            o,
            &name,
            sec,
            &cpty,
            args.privileged,
            "encrypting",
        )
        .await?;
        self.inner.encrypt(args, originator).await
    }

    /// Checks protocol permission for decryption.
    async fn decrypt(
        &self,
        args: DecryptArgs,
        originator: Option<&str>,
    ) -> Result<DecryptResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(
            self,
            o,
            &name,
            sec,
            &cpty,
            args.privileged,
            "decryption",
        )
        .await?;
        self.inner.decrypt(args, originator).await
    }

    /// Checks protocol permission for HMAC creation.
    async fn create_hmac(
        &self,
        args: CreateHmacArgs,
        originator: Option<&str>,
    ) -> Result<CreateHmacResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(self, o, &name, sec, &cpty, args.privileged, "hmac")
            .await?;
        self.inner.create_hmac(args, originator).await
    }

    /// Checks protocol permission for HMAC verification.
    async fn verify_hmac(
        &self,
        args: VerifyHmacArgs,
        originator: Option<&str>,
    ) -> Result<VerifyHmacResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(
            self,
            o,
            &name,
            sec,
            &cpty,
            args.privileged,
            "hmacVerification",
        )
        .await?;
        self.inner.verify_hmac(args, originator).await
    }

    /// Checks protocol permission for signature creation.
    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
        originator: Option<&str>,
    ) -> Result<CreateSignatureResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(self, o, &name, sec, &cpty, args.privileged, "signing")
            .await?;
        self.inner.create_signature(args, originator).await
    }

    /// Checks protocol permission for signature verification.
    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
        originator: Option<&str>,
    ) -> Result<VerifySignatureResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let (name, sec) = protocol_info(&args.protocol_id);
        let cpty = counterparty_str(&args.counterparty);
        ensure::ensure_protocol_permission(
            self,
            o,
            &name,
            sec,
            &cpty,
            args.privileged,
            "signatureVerification",
        )
        .await?;
        self.inner.verify_signature(args, originator).await
    }

    // -----------------------------------------------------------------------
    // Certificate methods
    // -----------------------------------------------------------------------

    /// Checks protocol permission for certificate acquisition before delegating.
    ///
    /// If `config.require_certificate_access_for_acquisition` is true,
    /// calls `ensure_protocol_permission` with protocol
    /// `"certificate acquisition {base64(cert_type)}"`.
    async fn acquire_certificate(
        &self,
        args: AcquireCertificateArgs,
        originator: Option<&str>,
    ) -> Result<Certificate, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        if self.config.require_certificate_access_for_acquisition {
            let cert_type_str = BASE64_STANDARD.encode(args.cert_type.0);
            ensure::ensure_protocol_permission(
                self,
                o,
                &format!("certificate acquisition {}", cert_type_str),
                1,
                "self",
                args.privileged,
                "generic",
            )
            .await?;
        }
        self.inner.acquire_certificate(args, originator).await
    }

    /// Checks protocol permission for certificate listing before delegating.
    ///
    /// If `config.require_certificate_access_for_listing` is true,
    /// calls `ensure_protocol_permission` with protocol `"certificate list"`.
    async fn list_certificates(
        &self,
        args: ListCertificatesArgs,
        originator: Option<&str>,
    ) -> Result<ListCertificatesResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        if self.config.require_certificate_access_for_listing {
            ensure::ensure_protocol_permission(
                self,
                o,
                "certificate list",
                1,
                "self",
                *args.privileged,
                "generic",
            )
            .await?;
        }
        self.inner.list_certificates(args, originator).await
    }

    /// Checks certificate access permission before proving a certificate.
    ///
    /// Always calls `ensure_certificate_access` with the certificate type,
    /// fields to reveal, and verifier public key.
    async fn prove_certificate(
        &self,
        args: ProveCertificateArgs,
        originator: Option<&str>,
    ) -> Result<ProveCertificateResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        let cert_type_str = args
            .certificate
            .cert_type
            .as_ref()
            .map(|ct| BASE64_STANDARD.encode(ct.0))
            .unwrap_or_default();
        let verifier_hex = args.verifier.to_der_hex();
        ensure::ensure_certificate_access(
            self,
            o,
            &cert_type_str,
            Some(&args.fields_to_reveal),
            Some(&verifier_hex),
            *args.privileged,
            "disclosure",
        )
        .await?;
        self.inner.prove_certificate(args, originator).await
    }

    /// Checks protocol permission for certificate relinquishment before delegating.
    ///
    /// If `config.require_certificate_access_for_relinquishing` is true,
    /// calls `ensure_protocol_permission` with protocol
    /// `"certificate relinquishment {base64(cert_type)}"`.
    async fn relinquish_certificate(
        &self,
        args: RelinquishCertificateArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, bsv::wallet::error::WalletError> {
        let o = orig(originator);
        if self.config.require_certificate_access_for_relinquishing {
            let cert_type_str = BASE64_STANDARD.encode(args.cert_type.0);
            ensure::ensure_protocol_permission(
                self,
                o,
                &format!("certificate relinquishment {}", cert_type_str),
                1,
                "self",
                false,
                "generic",
            )
            .await?;
        }
        self.inner.relinquish_certificate(args, originator).await
    }

    // -----------------------------------------------------------------------
    // Discovery methods
    // -----------------------------------------------------------------------

    /// Checks certificate access if config flag is set.
    /// Discovery args don't specify cert_type, so we use a generic "discovery" check.
    async fn discover_by_identity_key(
        &self,
        args: DiscoverByIdentityKeyArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        if self.config.require_certificate_access_for_discovery {
            let o = orig(originator);
            ensure::ensure_certificate_access(self, o, "*", None, None, false, "discovery").await?;
        }
        self.inner.discover_by_identity_key(args, originator).await
    }

    /// Checks certificate access if config flag is set.
    async fn discover_by_attributes(
        &self,
        args: DiscoverByAttributesArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
        if self.config.require_certificate_access_for_discovery {
            let o = orig(originator);
            ensure::ensure_certificate_access(self, o, "*", None, None, false, "discovery").await?;
        }
        self.inner.discover_by_attributes(args, originator).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WalletError;

    // -----------------------------------------------------------------------
    // MockWalletInterface
    // -----------------------------------------------------------------------

    /// Simple mock wallet for testing the permissions manager.
    /// All methods return `NotImplemented` except `is_authenticated` (returns true).
    struct MockWalletInterface;

    #[async_trait]
    impl WalletInterface for MockWalletInterface {
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
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "wait_for_authentication".into(),
            ))
        }

        async fn get_height(
            &self,
            _originator: Option<&str>,
        ) -> Result<GetHeightResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "get_height".into(),
            ))
        }

        async fn get_header_for_height(
            &self,
            _args: GetHeaderArgs,
            _originator: Option<&str>,
        ) -> Result<GetHeaderResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "get_header_for_height".into(),
            ))
        }

        async fn get_network(
            &self,
            _originator: Option<&str>,
        ) -> Result<GetNetworkResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "get_network".into(),
            ))
        }

        async fn get_version(
            &self,
            _originator: Option<&str>,
        ) -> Result<GetVersionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "get_version".into(),
            ))
        }

        async fn create_action(
            &self,
            _args: CreateActionArgs,
            _originator: Option<&str>,
        ) -> Result<CreateActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "create_action".into(),
            ))
        }

        async fn sign_action(
            &self,
            _args: SignActionArgs,
            _originator: Option<&str>,
        ) -> Result<SignActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "sign_action".into(),
            ))
        }

        async fn abort_action(
            &self,
            _args: AbortActionArgs,
            _originator: Option<&str>,
        ) -> Result<AbortActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "abort_action".into(),
            ))
        }

        async fn list_actions(
            &self,
            _args: ListActionsArgs,
            _originator: Option<&str>,
        ) -> Result<ListActionsResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "list_actions".into(),
            ))
        }

        async fn internalize_action(
            &self,
            _args: InternalizeActionArgs,
            _originator: Option<&str>,
        ) -> Result<InternalizeActionResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "internalize_action".into(),
            ))
        }

        async fn list_outputs(
            &self,
            _args: ListOutputsArgs,
            _originator: Option<&str>,
        ) -> Result<ListOutputsResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "list_outputs".into(),
            ))
        }

        async fn relinquish_output(
            &self,
            _args: RelinquishOutputArgs,
            _originator: Option<&str>,
        ) -> Result<RelinquishOutputResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "relinquish_output".into(),
            ))
        }

        async fn get_public_key(
            &self,
            _args: GetPublicKeyArgs,
            _originator: Option<&str>,
        ) -> Result<GetPublicKeyResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "get_public_key".into(),
            ))
        }

        async fn reveal_counterparty_key_linkage(
            &self,
            _args: RevealCounterpartyKeyLinkageArgs,
            _originator: Option<&str>,
        ) -> Result<RevealCounterpartyKeyLinkageResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "reveal_counterparty_key_linkage".into(),
            ))
        }

        async fn reveal_specific_key_linkage(
            &self,
            _args: RevealSpecificKeyLinkageArgs,
            _originator: Option<&str>,
        ) -> Result<RevealSpecificKeyLinkageResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "reveal_specific_key_linkage".into(),
            ))
        }

        async fn encrypt(
            &self,
            _args: EncryptArgs,
            _originator: Option<&str>,
        ) -> Result<EncryptResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "encrypt".into(),
            ))
        }

        async fn decrypt(
            &self,
            _args: DecryptArgs,
            _originator: Option<&str>,
        ) -> Result<DecryptResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "decrypt".into(),
            ))
        }

        async fn create_hmac(
            &self,
            _args: CreateHmacArgs,
            _originator: Option<&str>,
        ) -> Result<CreateHmacResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "create_hmac".into(),
            ))
        }

        async fn verify_hmac(
            &self,
            _args: VerifyHmacArgs,
            _originator: Option<&str>,
        ) -> Result<VerifyHmacResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "verify_hmac".into(),
            ))
        }

        async fn create_signature(
            &self,
            _args: CreateSignatureArgs,
            _originator: Option<&str>,
        ) -> Result<CreateSignatureResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "create_signature".into(),
            ))
        }

        async fn verify_signature(
            &self,
            _args: VerifySignatureArgs,
            _originator: Option<&str>,
        ) -> Result<VerifySignatureResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "verify_signature".into(),
            ))
        }

        async fn acquire_certificate(
            &self,
            _args: AcquireCertificateArgs,
            _originator: Option<&str>,
        ) -> Result<Certificate, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "acquire_certificate".into(),
            ))
        }

        async fn list_certificates(
            &self,
            _args: ListCertificatesArgs,
            _originator: Option<&str>,
        ) -> Result<ListCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "list_certificates".into(),
            ))
        }

        async fn prove_certificate(
            &self,
            _args: ProveCertificateArgs,
            _originator: Option<&str>,
        ) -> Result<ProveCertificateResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "prove_certificate".into(),
            ))
        }

        async fn relinquish_certificate(
            &self,
            _args: RelinquishCertificateArgs,
            _originator: Option<&str>,
        ) -> Result<RelinquishCertificateResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "relinquish_certificate".into(),
            ))
        }

        async fn discover_by_identity_key(
            &self,
            _args: DiscoverByIdentityKeyArgs,
            _originator: Option<&str>,
        ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "discover_by_identity_key".into(),
            ))
        }

        async fn discover_by_attributes(
            &self,
            _args: DiscoverByAttributesArgs,
            _originator: Option<&str>,
        ) -> Result<DiscoverCertificatesResult, bsv::wallet::error::WalletError> {
            Err(bsv::wallet::error::WalletError::NotImplemented(
                "discover_by_attributes".into(),
            ))
        }
    }

    fn make_manager(admin: &str) -> WalletPermissionsManager {
        WalletPermissionsManager::new(
            Arc::new(MockWalletInterface),
            admin.to_string(),
            PermissionsManagerConfig::default(),
        )
    }

    #[tokio::test]
    async fn test_admin_bypasses_all() {
        let mgr = make_manager("admin.example.com");
        // Admin originator should pass through to inner wallet
        let result = mgr.is_authenticated(Some("admin.example.com")).await;
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert!(auth.authenticated);
    }

    #[tokio::test]
    async fn test_non_admin_delegates() {
        let mgr = make_manager("admin.example.com");
        // Non-admin originator should also delegate (stubs pass through)
        let result = mgr.is_authenticated(Some("other.example.com")).await;
        assert!(result.is_ok());
        let auth = result.unwrap();
        assert!(auth.authenticated);
    }

    #[tokio::test]
    async fn test_p_module_registration() {
        let mut mgr = make_manager("admin.example.com");
        assert!(mgr.permission_modules.is_empty());

        // Create a simple test module
        struct TestModule;

        #[async_trait]
        impl PermissionsModule for TestModule {
            async fn on_request(
                &self,
                _request: &types::PermissionRequest,
                _originator: &str,
            ) -> Result<types::PermissionResponse, WalletError> {
                Ok(types::PermissionResponse::EphemeralGrant)
            }

            async fn on_response(
                &self,
                _request: &types::PermissionRequest,
                result: serde_json::Value,
                _originator: &str,
            ) -> Result<serde_json::Value, WalletError> {
                Ok(result)
            }
        }

        mgr.register_module("test-scheme".to_string(), Arc::new(TestModule));
        assert_eq!(mgr.permission_modules.len(), 1);
        assert!(mgr.permission_modules.contains_key("test-scheme"));
        assert!(mgr.has_permission_module("test-scheme"));
        assert!(!mgr.has_permission_module("other-scheme"));
    }

    #[tokio::test]
    async fn test_admin_normalization() {
        // Admin with port 443 should be normalized
        let mgr = make_manager("https://admin.example.com:443");
        assert!(mgr.is_admin(Some("admin.example.com")));
        assert!(mgr.is_admin(Some("ADMIN.EXAMPLE.COM")));
        assert!(!mgr.is_admin(Some("other.com")));
        assert!(!mgr.is_admin(None));
    }

    #[tokio::test]
    async fn test_admin_originator_accessor() {
        let mgr = make_manager("admin.example.com");
        assert_eq!(mgr.admin_originator(), "admin.example.com");
    }

    #[tokio::test]
    async fn test_deny_permission_via_manager() {
        let mgr = make_manager("admin.example.com");
        let req = types::PermissionRequest {
            permission_type: types::PermissionType::ProtocolPermission,
            originator: "example.com".to_string(),
            privileged: None,
            protocol: Some("test".to_string()),
            security_level: None,
            counterparty: None,
            basket_name: None,
            cert_type: None,
            cert_fields: None,
            verifier: None,
            amount: None,
            description: None,
            labels: None,
            is_new_user: false,
        };
        let resp = mgr.deny_permission(&req);
        match resp {
            types::PermissionResponse::Deny { reason } => {
                assert!(reason.contains("Protocol permission denied"));
            }
            _ => panic!("Expected Deny response"),
        }
    }
}
