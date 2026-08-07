//! Per-call caller identity for the signing seam.
//!
//! A wallet authenticates its caller at the transport, then loses that
//! identity before signing, because no signer seam carries it. When the
//! signer is a different trust domain (an MPC cosigner below this seam),
//! it enforces blind — applying the union of every grant on the vault
//! instead of just the caller's. [`SigningContext`] threads the
//! transport-authenticated identity down to [`SigningProvider`], so an
//! enforcement point below the seam can scope its decision to the caller.
//!
//! [`SigningProvider`]: crate::signer::signing_provider::SigningProvider

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;

use bsv::wallet::error::WalletError as SdkWalletError;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionResult, SignActionArgs, SignActionResult, WalletInterface,
};

/// Who a wallet call is being made *on behalf of*.
///
/// Not an `Option`. A wallet acting for itself says so explicitly, so a call
/// site that forgets cannot silently inherit the wallet's own authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallerRef {
    /// The principal the wallet's transport authenticated, on whose behalf this
    /// call is made.
    ///
    /// **The toolbox never interprets this string.** Its form is the transport's
    /// and its meaning is the [`SigningProvider`]'s: a browser-substrate wallet
    /// will put an origin domain here, a mutually-authenticated server boundary
    /// an identity public key. A provider that cares MUST document which it
    /// expects.
    ///
    /// It is deliberately NOT a BRC-100 `originator`: that field is specified as
    /// an FQDN and validated as one, so a value of another shape cannot ride in
    /// it.
    ///
    /// [`SigningProvider`]: crate::signer::signing_provider::SigningProvider
    Authenticated(String),
    /// The wallet acting on its own behalf — a CLI, an automation, a scheduled
    /// task. Carries no principal because there is no third party.
    Itself,
}

/// Per-call context threaded to the signing seam.
///
/// `#[non_exhaustive]` does not make this change non-breaking — nothing does.
/// It makes the *next* field free: hosts construct via [`SigningContext::itself`]
/// or [`SigningContext::authenticated`], so adding a field later breaks no one.
#[non_exhaustive]
#[derive(Clone)]
pub struct SigningContext {
    /// The identity this call acts on behalf of.
    pub caller: CallerRef,
    /// Opaque host payload. The toolbox NEVER inspects it — it exists so a host
    /// can carry an authorization artifact whose shape the toolbox must not know.
    pub host: Option<Arc<dyn Any + Send + Sync>>,
}

impl SigningContext {
    /// The wallet acting for itself.
    pub fn itself() -> Self {
        Self {
            caller: CallerRef::Itself,
            host: None,
        }
    }

    /// An authenticated external caller. `principal` is whatever the transport
    /// authenticated — see [`CallerRef::Authenticated`] for its (deliberately
    /// uninterpreted) meaning.
    pub fn authenticated(principal: impl Into<String>) -> Self {
        Self {
            caller: CallerRef::Authenticated(principal.into()),
            host: None,
        }
    }
}

impl std::fmt::Debug for SigningContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningContext")
            .field("caller", &self.caller)
            .field("host", &self.host.as_ref().map(|_| "<opaque>"))
            .finish()
    }
}

/// Object-safe companion to [`WalletInterface`] for hosts that authenticate
/// their caller at the transport and need that identity to reach the signing
/// seam.
///
/// `WalletInterface` belongs to `bsv-sdk` and is the BRC-100 surface, so it
/// cannot carry a [`SigningContext`]; this trait adds the context-taking
/// variants alongside it. The plain `WalletInterface::create_action` /
/// `sign_action` delegate with [`SigningContext::itself`], so no BRC-100
/// client can observe the difference.
#[async_trait]
pub trait ContextualWallet: WalletInterface {
    /// `createAction` on behalf of `ctx.caller`.
    async fn create_action_in(
        &self,
        args: CreateActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<CreateActionResult, SdkWalletError>;

    /// `signAction` on behalf of `ctx.caller`.
    async fn sign_action_in(
        &self,
        args: SignActionArgs,
        originator: Option<&str>,
        ctx: &SigningContext,
    ) -> Result<SignActionResult, SdkWalletError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_the_caller() {
        assert_eq!(SigningContext::itself().caller, CallerRef::Itself);
        let k = "02abc";
        assert_eq!(
            SigningContext::authenticated(k).caller,
            CallerRef::Authenticated(k.to_string())
        );
        assert!(SigningContext::itself().host.is_none());
    }
}
