//! Reference implementation of [`SigningProvider`] for single-key wallets.
//!
//! Wraps [`CachedKeyDeriver`] behind the async [`SigningProvider`] trait,
//! preserving the existing local-key derivation and signing behavior.
//! This is the default provider for wallets that hold a single private key.
//!
//! [`CachedKeyDeriver`]: bsv::wallet::cached_key_deriver::CachedKeyDeriver

use async_trait::async_trait;

use bsv::primitives::ecdsa::ecdsa_sign;
use bsv::primitives::public_key::PublicKey;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::types::{Counterparty, CounterpartyType};

use crate::error::{WalletError, WalletResult};
use crate::signer::signing_provider::SigningProvider;
use crate::utility::script_template_brc29::{brc29_protocol, ScriptTemplateBRC29};

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

        // `sighash` is already hash256(preimage) = sha256d(preimage), the exact
        // 32-byte digest OP_CHECKSIG verifies against. Sign it RAW via ecdsa_sign;
        // PrivateKey::sign would SHA-256 it again, producing a signature over
        // sha256(sha256d(preimage)) that can never verify. force_low_s = true to
        // match TS (Spend.validate rejects high-S). Mirrors StandardSigningProvider.ts:48.
        let sig_obj = ecdsa_sign(sighash, priv_key.bn(), true)
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

    async fn derive_wallet_payment_locking_script(
        &self,
        derivation_prefix: &str,
        derivation_suffix: &str,
        sender_identity_key: &PublicKey,
    ) -> WalletResult<Option<Vec<u8>>> {
        // BRC-29 key ID: "{prefix} {suffix}" from the base64 string forms.
        let key_id = format!("{derivation_prefix} {derivation_suffix}");

        // The sender's identity key is the counterparty; for_self = true
        // because we are the receiver deriving our own child key.
        let counterparty = Counterparty {
            counterparty_type: CounterpartyType::Other,
            public_key: Some(sender_identity_key.clone()),
        };
        let derived_pub = self
            .key_deriver
            .derive_public_key(&brc29_protocol(), &key_id, &counterparty, true)
            .map_err(|e| {
                WalletError::Internal(format!("BRC-29 wallet-payment derivation failed: {e}"))
            })?;

        // hash160(derived pubkey) → 25-byte P2PKH locking script.
        let hash_vec = derived_pub.to_hash();
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&hash_vec);
        let p2pkh = P2PKH::from_public_key_hash(hash);
        let script = p2pkh.lock().map_err(|e| {
            WalletError::Internal(format!("Failed to build P2PKH locking script: {e}"))
        })?;
        Ok(Some(script.to_binary()))
    }

    fn identity_public_key(&self) -> &PublicKey {
        &self.identity_pub_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::primitives::ecdsa::ecdsa_verify;
    use bsv::primitives::hash::sha256d;
    use bsv::primitives::private_key::PrivateKey;
    use bsv::primitives::signature::Signature;
    use bsv::primitives::transaction_signature::{SIGHASH_ALL, SIGHASH_FORKID};
    use bsv::script::locking_script::LockingScript;
    use bsv::script::unlocking_script::UnlockingScript;
    use bsv::transaction::transaction::Transaction;
    use bsv::transaction::transaction_input::TransactionInput;
    use bsv::transaction::transaction_output::TransactionOutput;

    /// #3 regression: the signature produced by `sign_input` must verify against
    /// the raw sighash under OP_CHECKSIG semantics (ecdsa over hash256(preimage)).
    /// Before the fix, `PrivateKey::sign` hashed the already-hashed sighash a third
    /// time, so this verification always failed.
    #[tokio::test]
    async fn test_sign_input_signature_verifies_over_sighash() {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let identity_pub = priv_key.to_public_key();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        let provider = StandardSigningProvider::new(key_deriver, identity_pub.clone());

        let (prefix, suffix) = ("prefix1", "suffix1");
        let sighash_type = SIGHASH_ALL | SIGHASH_FORKID;

        // Build the BRC-29 source locking script this input spends (self-payment).
        let template = ScriptTemplateBRC29::new(prefix.to_string(), suffix.to_string());
        let source_script_bytes = template
            .lock(provider.key_deriver().root_key(), &identity_pub)
            .unwrap();
        let source_locking_script = LockingScript::from_binary(&source_script_bytes);
        let source_satoshis = 10_000u64;

        // Minimal spending transaction with one input and one output.
        let mut tx = Transaction::new();
        tx.add_input(TransactionInput {
            source_transaction: None,
            source_txid: Some(
                "aaaa1111bbbb2222cccc3333dddd4444aaaa1111bbbb2222cccc3333dddd4444".to_string(),
            ),
            source_output_index: 0,
            unlocking_script: Some(UnlockingScript::from_binary(&[])),
            sequence: 0xFFFFFFFF,
        });
        tx.add_output(TransactionOutput {
            satoshis: Some(5_000),
            locking_script: LockingScript::from_binary(&[
                0x76, 0xa9, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x88, 0xac,
            ]),
            change: false,
        });

        // Compute the sighash exactly as the signing pipeline does.
        let preimage = tx
            .sighash_preimage(0, sighash_type, source_satoshis, &source_locking_script)
            .unwrap();
        let hash = sha256d(&preimage);
        let mut sighash = [0u8; 32];
        sighash.copy_from_slice(&hash);

        let script = provider
            .sign_input(&sighash, sighash_type, prefix, suffix, &identity_pub)
            .await
            .unwrap();

        // Unlocking script layout: <sigLen><DER..><hashtype><33><pubkey33>
        let sig_field_len = script[0] as usize;
        let der = &script[1..1 + sig_field_len - 1];
        let pk_push_idx = 1 + sig_field_len;
        let pk_len = script[pk_push_idx] as usize;
        let pubkey_bytes = &script[pk_push_idx + 1..pk_push_idx + 1 + pk_len];

        let sig = Signature::from_der(der).expect("valid DER signature");
        let pubkey = PublicKey::from_der_bytes(pubkey_bytes).expect("valid pubkey");

        assert!(
            ecdsa_verify(&sighash, &sig, pubkey.point()),
            "sign_input signature must verify against the raw sighash digest"
        );
    }
}
