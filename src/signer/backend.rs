//! Which custody backend a signing step derives keys and signatures with.
//!
//! The signer pipeline (`signer_create_action`, `signer_sign_action`) is one
//! implementation with two custody modes. [`SigningBackend`] is the parameter
//! that picks between them at the two points where they actually differ:
//! deriving change locking scripts, and producing input unlocking scripts.

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::KeyDeriverApi;

use crate::signer::signing_provider::SigningProvider;

/// The custody backend a signing step derives keys and signatures with.
///
/// [`Local`] holds a root private key and derives BRC-42/BRC-29 children
/// in-process. [`Delegated`] holds no key material: a [`SigningProvider`]
/// answers every derivation and signature, which is what lets a wallet whose
/// identity key is a joint/threshold public key — with no local root key —
/// use the same pipeline.
///
/// [`Local`]: SigningBackend::Local
/// [`Delegated`]: SigningBackend::Delegated
pub enum SigningBackend<'a> {
    /// Derive and sign in-process from a locally-held root private key.
    Local {
        /// The deriver whose root key backs every BRC-29 derivation.
        key_deriver: &'a dyn KeyDeriverApi,
        /// The wallet's identity public key, used as the self-payment
        /// counterparty when an input carries no explicit unlocker key.
        identity_pub_key: &'a PublicKey,
    },
    /// Delegate every derivation and signature to a [`SigningProvider`].
    ///
    /// In this mode the pipeline never reaches for a root private key, so a
    /// throwaway or absent root key cannot silently produce change outputs
    /// nobody can spend.
    Delegated(&'a dyn SigningProvider),
}
