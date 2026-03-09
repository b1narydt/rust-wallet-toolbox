//! PrivilegedKeyManager trait for sensitive cryptographic operations.
//!
//! Defines the trait that privileged key managers must implement, mirroring
//! the 9 crypto methods from the bsv-sdk WalletInterface plus `destroy_key`.
//! Implementations handle secure key storage, obfuscation, and retention.

use async_trait::async_trait;

use bsv::wallet::error::WalletError;
use bsv::wallet::interfaces::{
    CreateHmacArgs, CreateHmacResult, CreateSignatureArgs, CreateSignatureResult, DecryptArgs,
    DecryptResult, EncryptArgs, EncryptResult, GetPublicKeyArgs, GetPublicKeyResult,
    RevealCounterpartyKeyLinkageArgs, RevealCounterpartyKeyLinkageResult,
    RevealSpecificKeyLinkageArgs, RevealSpecificKeyLinkageResult, VerifyHmacArgs, VerifyHmacResult,
    VerifySignatureArgs, VerifySignatureResult,
};

/// Trait for managing a privileged private key and performing crypto operations.
///
/// Implementations are expected to securely store the key (e.g. using HSM,
/// secure enclave, or memory obfuscation) and expose the standard wallet
/// crypto interface. The key may have a limited retention period.
///
/// All methods take the same argument/result types as `WalletInterface` crypto
/// methods from the bsv-sdk, using `bsv::wallet::error::WalletError` as the
/// error type for consistency.
#[async_trait]
pub trait PrivilegedKeyManager: Send + Sync {
    /// Get the public key for the given protocol/key derivation parameters.
    async fn get_public_key(
        &self,
        args: GetPublicKeyArgs,
    ) -> Result<GetPublicKeyResult, WalletError>;

    /// Encrypt plaintext using the derived key.
    async fn encrypt(&self, args: EncryptArgs) -> Result<EncryptResult, WalletError>;

    /// Decrypt ciphertext using the derived key.
    async fn decrypt(&self, args: DecryptArgs) -> Result<DecryptResult, WalletError>;

    /// Create an HMAC of the given data.
    async fn create_hmac(&self, args: CreateHmacArgs) -> Result<CreateHmacResult, WalletError>;

    /// Verify an HMAC against expected data.
    async fn verify_hmac(&self, args: VerifyHmacArgs) -> Result<VerifyHmacResult, WalletError>;

    /// Create a digital signature of the given data.
    async fn create_signature(
        &self,
        args: CreateSignatureArgs,
    ) -> Result<CreateSignatureResult, WalletError>;

    /// Verify a digital signature.
    async fn verify_signature(
        &self,
        args: VerifySignatureArgs,
    ) -> Result<VerifySignatureResult, WalletError>;

    /// Reveal the counterparty key linkage to a verifier.
    async fn reveal_counterparty_key_linkage(
        &self,
        args: RevealCounterpartyKeyLinkageArgs,
    ) -> Result<RevealCounterpartyKeyLinkageResult, WalletError>;

    /// Reveal a specific key linkage to a verifier.
    async fn reveal_specific_key_linkage(
        &self,
        args: RevealSpecificKeyLinkageArgs,
    ) -> Result<RevealSpecificKeyLinkageResult, WalletError>;

    /// Destroy the in-memory key material immediately.
    ///
    /// After calling this, the manager must re-acquire the key from its
    /// secure source before performing any further operations.
    async fn destroy_key(&self) -> Result<(), WalletError>;
}

/// A no-op PrivilegedKeyManager that returns `NotImplemented` for all operations.
///
/// Used as a placeholder during authentication flows before the real key manager
/// is constructed from derived root key material.
pub struct NoOpPrivilegedKeyManager;

#[async_trait]
impl PrivilegedKeyManager for NoOpPrivilegedKeyManager {
    async fn get_public_key(
        &self,
        _args: GetPublicKeyArgs,
    ) -> Result<GetPublicKeyResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn encrypt(&self, _args: EncryptArgs) -> Result<EncryptResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn decrypt(&self, _args: DecryptArgs) -> Result<DecryptResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn create_hmac(&self, _args: CreateHmacArgs) -> Result<CreateHmacResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn verify_hmac(&self, _args: VerifyHmacArgs) -> Result<VerifyHmacResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn create_signature(
        &self,
        _args: CreateSignatureArgs,
    ) -> Result<CreateSignatureResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn verify_signature(
        &self,
        _args: VerifySignatureArgs,
    ) -> Result<VerifySignatureResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn reveal_counterparty_key_linkage(
        &self,
        _args: RevealCounterpartyKeyLinkageArgs,
    ) -> Result<RevealCounterpartyKeyLinkageResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn reveal_specific_key_linkage(
        &self,
        _args: RevealSpecificKeyLinkageArgs,
    ) -> Result<RevealSpecificKeyLinkageResult, WalletError> {
        Err(WalletError::NotImplemented(
            "NoOpPrivilegedKeyManager".into(),
        ))
    }

    async fn destroy_key(&self) -> Result<(), WalletError> {
        Ok(())
    }
}
