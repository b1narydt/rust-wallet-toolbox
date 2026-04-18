//! Async trait for pluggable signing backends.
//!
//! Abstracts BRC-42 key derivation and ECDSA signing behind an async
//! interface, enabling the wallet to work with any signing backend
//! without changes to the transaction construction pipeline.

use async_trait::async_trait;
use bsv::primitives::public_key::PublicKey;

use crate::error::WalletResult;

/// Async trait for pluggable signing backends.
///
/// Abstracts BRC-42 key derivation and ECDSA signing behind an async
/// interface, enabling the wallet to work with different signing
/// backends without changes to the transaction construction pipeline.
///
/// Included implementations:
///
/// - **Local keys** — [`StandardSigningProvider`] wraps [`CachedKeyDeriver`]
///   for single-key wallets (the default).
///
/// External implementations can support:
///
/// - **Threshold signing** — distributed protocols (e.g., MPC) where no
///   single party holds the full private key.
/// - **Remote signers** — services that implement BRC-42 derivation and
///   ECDSA signing over a network API.
///
/// The trait never exposes private key material. Implementations receive
/// a sighash and return a complete P2PKH unlocking script. Key derivation follows
/// BRC-42/43 conventions using `derivation_prefix` and `derivation_suffix`
/// as the key ID components.
///
/// # Relationship to [`CachedKeyDeriver`]
///
/// [`StandardSigningProvider`] wraps a `CachedKeyDeriver` and a
/// `PublicKey`, delegating all derivation and signing to the existing
/// single-key code path. External implementations replace that code path
/// entirely with their own backend logic.
///
/// # Example
///
/// ```rust,ignore
/// use bsv_wallet_toolbox::signer::SigningProvider;
/// use bsv_wallet_toolbox::signer::StandardSigningProvider;
///
/// // Local single-key signing (the default):
/// let provider = StandardSigningProvider::new(key_deriver, identity_pub_key);
///
/// // Or implement the trait for your own backend:
/// struct MyRemoteSigner { /* ... */ }
///
/// #[async_trait::async_trait]
/// impl SigningProvider for MyRemoteSigner {
///     async fn derive_change_locking_script(
///         &self, prefix: &str, suffix: &str,
///     ) -> WalletResult<Vec<u8>> {
///         // Call remote signing service for BRC-42 derivation
///         # todo!()
///     }
///     async fn sign_input(
///         &self, sighash: &[u8; 32], sighash_type: u32,
///         prefix: &str, suffix: &str, unlocker: &PublicKey,
///     ) -> WalletResult<Vec<u8>> {
///         // Send sighash to remote signer, return P2PKH unlocking script
///         # todo!()
///     }
///     fn identity_public_key(&self) -> &PublicKey { /* ... */ # todo!() }
/// }
/// ```
///
/// [`StandardSigningProvider`]: crate::signer::StandardSigningProvider
/// [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver
#[async_trait]
pub trait SigningProvider: Send + Sync {
    /// Derive the P2PKH locking script for a BRC-29 change output.
    ///
    /// Performs BRC-42 ECDH key derivation using the given prefix/suffix
    /// pair and returns a 25-byte P2PKH locking script for the derived
    /// public key.
    ///
    /// # Arguments
    /// * `derivation_prefix` — Random per-transaction BRC-29 prefix.
    /// * `derivation_suffix` — Random per-output BRC-29 suffix.
    ///
    /// # Returns
    /// 25-byte P2PKH locking script (`OP_DUP OP_HASH160 <20> <hash> OP_EQUALVERIFY OP_CHECKSIG`).
    async fn derive_change_locking_script(
        &self,
        derivation_prefix: &str,
        derivation_suffix: &str,
    ) -> WalletResult<Vec<u8>>;

    /// Sign a transaction input and return a complete P2PKH unlocking script.
    ///
    /// Given a pre-computed sighash, produces the unlocking script bytes:
    /// `<push sig_len> <DER_signature + hashtype_byte> <push 33> <compressed_pubkey>`.
    ///
    /// # Arguments
    /// * `sighash` — 32-byte double-SHA256 sighash of the input.
    /// * `sighash_type` — Sighash flags (typically `SIGHASH_ALL | SIGHASH_FORKID`).
    /// * `derivation_prefix` — BRC-29 derivation prefix for key offset.
    /// * `derivation_suffix` — BRC-29 derivation suffix for key offset.
    /// * `unlocker_pub_key` — Counterparty identity key (or self) for ECDH.
    ///
    /// # Returns
    /// P2PKH unlocking script bytes ready for insertion into `TransactionInput`.
    async fn sign_input(
        &self,
        sighash: &[u8; 32],
        sighash_type: u32,
        derivation_prefix: &str,
        derivation_suffix: &str,
        unlocker_pub_key: &PublicKey,
    ) -> WalletResult<Vec<u8>>;

    /// Get the wallet's identity public key.
    fn identity_public_key(&self) -> &PublicKey;
}
