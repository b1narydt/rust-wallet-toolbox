//! Reference implementation of [`SigningProvider`] for single-key wallets.
//!
//! Wraps [`CachedKeyDeriver`] behind the async [`SigningProvider`] trait,
//! preserving the existing local-key derivation and signing behavior.
//! This is the default provider for wallets that hold a single private key.
//!
//! [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver

use async_trait::async_trait;

use bsv::primitives::public_key::PublicKey;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;

use crate::error::{WalletError, WalletResult};
use crate::signer::signing_provider::SigningProvider;
use crate::utility::script_template_brc29::ScriptTemplateBRC29;

/// Standard signing provider using a single private key via [`CachedKeyDeriver`].
///
/// This is the reference implementation of [`SigningProvider`]. It delegates
/// BRC-42 key derivation and ECDSA signing to the existing
/// [`CachedKeyDeriver`] + [`ScriptTemplateBRC29`] code path.
///
/// Since all operations are local, the async methods return immediately.
///
/// # Example
///
/// ```rust,ignore
/// use bsv_wallet_toolbox::signer::StandardSigningProvider;
///
/// let provider = StandardSigningProvider::new(key_deriver, identity_pub_key);
/// // Use with build_signable_transaction_with_provider, etc.
/// ```
///
/// [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver
/// [`ScriptTemplateBRC29`]: crate::utility::script_template_brc29::ScriptTemplateBRC29
pub struct StandardSigningProvider {
    key_deriver: CachedKeyDeriver,
    identity_pub_key: PublicKey,
}

impl StandardSigningProvider {
    /// Create a new standard provider from a key deriver and identity key.
    pub fn new(key_deriver: CachedKeyDeriver, identity_pub_key: PublicKey) -> Self {
        Self {
            key_deriver,
            identity_pub_key,
        }
    }

    /// Access the underlying [`CachedKeyDeriver`].
    ///
    /// Useful for backward compatibility with code that expects direct
    /// access to the key deriver.
    ///
    /// [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver
    pub fn key_deriver(&self) -> &CachedKeyDeriver {
        &self.key_deriver
    }
}

#[async_trait]
impl SigningProvider for StandardSigningProvider {
    async fn derive_change_locking_script(
        &self,
        derivation_prefix: &str,
        derivation_suffix: &str,
    ) -> WalletResult<Vec<u8>> {
        let template =
            ScriptTemplateBRC29::new(derivation_prefix.to_string(), derivation_suffix.to_string());
        template.lock(self.key_deriver.root_key(), &self.identity_pub_key)
    }

    async fn sign_input(
        &self,
        sighash: &[u8; 32],
        sighash_type: u32,
        derivation_prefix: &str,
        derivation_suffix: &str,
        unlocker_pub_key: &PublicKey,
    ) -> WalletResult<Vec<u8>> {
        let template =
            ScriptTemplateBRC29::new(derivation_prefix.to_string(), derivation_suffix.to_string());
        let p2pkh = template.unlock(self.key_deriver.root_key(), unlocker_pub_key)?;

        let priv_key = p2pkh.private_key.as_ref().ok_or_else(|| {
            WalletError::Internal("P2PKH template has no private key".to_string())
        })?;

        let sig_obj = priv_key
            .sign(sighash, true)
            .map_err(|e| WalletError::Internal(format!("ECDSA sign failed: {e}")))?;

        let der = sig_obj.to_der();
        let pub_key = priv_key.to_public_key();
        let pubkey_bytes = pub_key.to_der();

        // Build P2PKH unlocking script: <push sig_len> <DER + hashtype> <push 33> <pubkey>
        let hashtype_byte = (sighash_type & 0xFF) as u8;
        let sig_with_ht_len = der.len() + 1;
        let mut script = Vec::with_capacity(1 + sig_with_ht_len + 1 + 33);
        script.push(sig_with_ht_len as u8);
        script.extend_from_slice(&der);
        script.push(hashtype_byte);
        script.push(33);
        script.extend_from_slice(&pubkey_bytes);

        Ok(script)
    }

    fn identity_public_key(&self) -> &PublicKey {
        &self.identity_pub_key
    }
}
