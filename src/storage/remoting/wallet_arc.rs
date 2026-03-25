//! `WalletArc<W>` — `Clone`-able wrapper for any `WalletInterface` implementor.
//!
//! # Problem
//!
//! `StorageClient<W>` holds `AuthFetch<W>`, which requires `W: Clone` because it
//! clones the wallet when creating a BRC-31 `Peer` during each handshake.
//! `ProtoWallet` (bsv-sdk 0.1.75) does not implement `Clone`, which prevents
//! `StorageClient<ProtoWallet>` from compiling.
//!
//! # Solution
//!
//! `WalletArc<W>` wraps any `WalletInterface` in an `Arc<W>` and derives `Clone`
//! (cheaply — arc ref-count increment). All `WalletInterface` methods are delegated
//! to the inner `W`. This lets callers use `StorageClient<WalletArc<ProtoWallet>>`
//! without needing `ProtoWallet: Clone`.
//!
//! # Example
//!
//! ```rust,ignore
//! use bsv::primitives::private_key::PrivateKey;
//! use bsv_wallet_toolbox::ProtoWallet;
//! use bsv_wallet_toolbox::storage::remoting::{StorageClient, WalletArc};
//!
//! let key = PrivateKey::from_random().unwrap();
//! let proto = ProtoWallet::new(key);
//! let wallet = WalletArc::new(proto);
//! let client = StorageClient::new(wallet, "https://staging-storage.babbage.systems");
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use bsv::wallet::interfaces::{
    AcquireCertificateArgs, AuthenticatedResult, Certificate, CreateActionArgs, CreateActionResult,
    CreateHmacArgs, CreateHmacResult, CreateSignatureArgs, CreateSignatureResult, DecryptArgs,
    DecryptResult, DiscoverByAttributesArgs, DiscoverByIdentityKeyArgs, DiscoverCertificatesResult,
    EncryptArgs, EncryptResult, GetHeaderArgs, GetHeaderResult, GetHeightResult, GetNetworkResult,
    GetPublicKeyArgs, GetPublicKeyResult, GetVersionResult, InternalizeActionArgs,
    InternalizeActionResult, ListActionsArgs, ListActionsResult, ListCertificatesArgs,
    ListCertificatesResult, ListOutputsArgs, ListOutputsResult, ProveCertificateArgs,
    ProveCertificateResult, RelinquishCertificateArgs, RelinquishCertificateResult,
    RelinquishOutputArgs, RelinquishOutputResult, RevealCounterpartyKeyLinkageArgs,
    RevealCounterpartyKeyLinkageResult, RevealSpecificKeyLinkageArgs,
    RevealSpecificKeyLinkageResult, SignActionArgs, SignActionResult, VerifyHmacArgs,
    VerifyHmacResult, VerifySignatureArgs, VerifySignatureResult, WalletInterface,
    AbortActionArgs, AbortActionResult,
};
use bsv::wallet::error::WalletError;

/// A `Clone`-able wrapper around any `WalletInterface` implementor.
///
/// Stores the inner wallet in an `Arc` so cloning is cheap (ref-count increment).
/// All `WalletInterface` methods are delegated to the inner `W` via deref.
///
/// `Clone` is manually implemented (not derived) so that it works even when
/// `W: !Clone` — `Arc<W>` is always cloneable regardless of `W`.
///
/// See module documentation for usage.
pub struct WalletArc<W: WalletInterface + Send + Sync + 'static>(Arc<W>);

impl<W: WalletInterface + Send + Sync + 'static> Clone for WalletArc<W> {
    fn clone(&self) -> Self {
        WalletArc(Arc::clone(&self.0))
    }
}

impl<W: WalletInterface + Send + Sync + std::fmt::Debug + 'static> std::fmt::Debug
    for WalletArc<W>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WalletArc").field(&self.0).finish()
    }
}

impl<W: WalletInterface + Send + Sync + 'static> WalletArc<W> {
    /// Wrap a `WalletInterface` implementor in an `Arc`.
    pub fn new(wallet: W) -> Self {
        WalletArc(Arc::new(wallet))
    }
}

#[async_trait]
impl<W: WalletInterface + Send + Sync + 'static> WalletInterface for WalletArc<W> {
    async fn create_action(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
    ) -> Result<CreateActionResult, WalletError> {
        self.0.create_action(args, originator).await
    }

    async fn sign_action(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
    ) -> Result<SignActionResult, WalletError> {
        self.0.sign_action(args, originator).await
    }

    async fn abort_action(
        &self,
        args: AbortActionArgs,
        originator: Option<&str>,
    ) -> Result<AbortActionResult, WalletError> {
        self.0.abort_action(args, originator).await
    }

    async fn list_actions(
        &self,
        args: ListActionsArgs,
        originator: Option<&str>,
    ) -> Result<ListActionsResult, WalletError> {
        self.0.list_actions(args, originator).await
    }

    async fn internalize_action(
        &self,
        args: InternalizeActionArgs,
        originator: Option<&str>,
    ) -> Result<InternalizeActionResult, WalletError> {
        self.0.internalize_action(args, originator).await
    }

    async fn list_outputs(
        &self,
        args: ListOutputsArgs,
        originator: Option<&str>,
    ) -> Result<ListOutputsResult, WalletError> {
        self.0.list_outputs(args, originator).await
    }

    async fn relinquish_output(
        &self,
        args: RelinquishOutputArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishOutputResult, WalletError> {
        self.0.relinquish_output(args, originator).await
    }

    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
        originator: Option<&str>,
    ) -> Result<GetPublicKeyResult, WalletError> {
        self.0.get_public_key(args, originator).await
    }

    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealCounterpartyKeyLinkageResult, WalletError> {
        self.0.reveal_counterparty_key_linkage(args, originator).await
    }

    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
        originator: Option<&str>,
    ) -> Result<RevealSpecificKeyLinkageResult, WalletError> {
        self.0.reveal_specific_key_linkage(args, originator).await
    }

    async fn encrypt(
        &self,
        args: EncryptArgs,
        originator: Option<&str>,
    ) -> Result<EncryptResult, WalletError> {
        self.0.encrypt(args, originator).await
    }

    async fn decrypt(
        &self,
        args: DecryptArgs,
        originator: Option<&str>,
    ) -> Result<DecryptResult, WalletError> {
        self.0.decrypt(args, originator).await
    }

    async fn create_hmac(
        &self,
        args: CreateHmacArgs,
        originator: Option<&str>,
    ) -> Result<CreateHmacResult, WalletError> {
        self.0.create_hmac(args, originator).await
    }

    async fn verify_hmac(
        &self,
        args: VerifyHmacArgs,
        originator: Option<&str>,
    ) -> Result<VerifyHmacResult, WalletError> {
        self.0.verify_hmac(args, originator).await
    }

    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
        originator: Option<&str>,
    ) -> Result<CreateSignatureResult, WalletError> {
        self.0.create_signature(args, originator).await
    }

    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
        originator: Option<&str>,
    ) -> Result<VerifySignatureResult, WalletError> {
        self.0.verify_signature(args, originator).await
    }

    async fn acquire_certificate(
        &self,
        args: AcquireCertificateArgs,
        originator: Option<&str>,
    ) -> Result<Certificate, WalletError> {
        self.0.acquire_certificate(args, originator).await
    }

    async fn list_certificates(
        &self,
        args: ListCertificatesArgs,
        originator: Option<&str>,
    ) -> Result<ListCertificatesResult, WalletError> {
        self.0.list_certificates(args, originator).await
    }

    async fn prove_certificate(
        &self,
        args: ProveCertificateArgs,
        originator: Option<&str>,
    ) -> Result<ProveCertificateResult, WalletError> {
        self.0.prove_certificate(args, originator).await
    }

    async fn relinquish_certificate(
        &self,
        args: RelinquishCertificateArgs,
        originator: Option<&str>,
    ) -> Result<RelinquishCertificateResult, WalletError> {
        self.0.relinquish_certificate(args, originator).await
    }

    async fn discover_by_identity_key(
        &self,
        args: DiscoverByIdentityKeyArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, WalletError> {
        self.0.discover_by_identity_key(args, originator).await
    }

    async fn discover_by_attributes(
        &self,
        args: DiscoverByAttributesArgs,
        originator: Option<&str>,
    ) -> Result<DiscoverCertificatesResult, WalletError> {
        self.0.discover_by_attributes(args, originator).await
    }

    async fn is_authenticated(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, WalletError> {
        self.0.is_authenticated(originator).await
    }

    async fn wait_for_authentication(
        &self,
        originator: Option<&str>,
    ) -> Result<AuthenticatedResult, WalletError> {
        self.0.wait_for_authentication(originator).await
    }

    async fn get_height(&self, originator: Option<&str>) -> Result<GetHeightResult, WalletError> {
        self.0.get_height(originator).await
    }

    async fn get_header_for_height(
        &self,
        args: GetHeaderArgs,
        originator: Option<&str>,
    ) -> Result<GetHeaderResult, WalletError> {
        self.0.get_header_for_height(args, originator).await
    }

    async fn get_network(&self, originator: Option<&str>) -> Result<GetNetworkResult, WalletError> {
        self.0.get_network(originator).await
    }

    async fn get_version(&self, originator: Option<&str>) -> Result<GetVersionResult, WalletError> {
        self.0.get_version(originator).await
    }
}
