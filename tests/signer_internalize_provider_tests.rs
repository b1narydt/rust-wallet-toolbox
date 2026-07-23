//! SigningProvider delegation tests for signer-level internalize_action.
//!
//! Covers issue #31: delegated BRC-29 wallet-payment locking-script
//! derivation. Twin of the TS fork suite
//! `test/signer/internalizeAction.provider.test.ts` (b1narydt/wallet-toolbox
//! branch `paragon/seam-2x` @ a6c5ee9):
//!
//! 1. Equivalence — provider-derived script is byte-identical to the legacy
//!    sender-side `ScriptTemplateBRC29::lock` script and to the local
//!    receiver-side `derive_public_key(..., for_self = true)` script.
//! 2. Provider mode — the hook short-circuits the local deriver (a poisoned
//!    deriver is never consulted) and receives the base64 STRING forms.
//! 3. Default (`Ok(None)`) / no provider — byte-identical to the 0.3.1
//!    behavior; tampered locking scripts are still rejected.

mod common;

#[cfg(feature = "sqlite")]
mod signer_internalize_provider_tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use base64::Engine as _;

    use bsv::primitives::private_key::PrivateKey;
    use bsv::primitives::public_key::PublicKey;
    use bsv::script::templates::p2pkh::P2PKH;
    use bsv::script::templates::ScriptTemplateLock;
    use bsv::script::{LockingScript, UnlockingScript};
    use bsv::transaction::beef_tx::BeefTx;
    use bsv::transaction::{
        Beef, Transaction as BsvTransaction, TransactionInput, TransactionOutput,
    };
    use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
    use bsv::wallet::interfaces::{InternalizeOutput, Payment};
    use bsv::wallet::types::{Counterparty, CounterpartyType};

    use bsv_wallet_toolbox::error::{WalletError, WalletResult};
    use bsv_wallet_toolbox::signer::methods::internalize_action::signer_internalize_action;
    use bsv_wallet_toolbox::signer::signing_provider::SigningProvider;
    use bsv_wallet_toolbox::signer::standard_provider::StandardSigningProvider;
    use bsv_wallet_toolbox::signer::types::ValidInternalizeActionArgs;
    use bsv_wallet_toolbox::utility::script_template_brc29::{brc29_protocol, ScriptTemplateBRC29};

    use super::common::{self, MockWalletServices};

    // -----------------------------------------------------------------------
    // Test providers
    // -----------------------------------------------------------------------

    /// Provider that records the arguments it receives and returns a
    /// pre-configured script (or `None`).
    struct RecordingProvider {
        script: Option<Vec<u8>>,
        identity: PublicKey,
        /// (derivation_prefix, derivation_suffix, sender DER bytes)
        seen: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl RecordingProvider {
        fn new(script: Option<Vec<u8>>, identity: PublicKey) -> Self {
            Self {
                script,
                identity,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SigningProvider for RecordingProvider {
        async fn derive_change_locking_script(
            &self,
            _derivation_prefix: &str,
            _derivation_suffix: &str,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal(
                "derive_change_locking_script must not be called".to_string(),
            ))
        }

        async fn sign_input(
            &self,
            _sighash: &[u8; 32],
            _sighash_type: u32,
            _derivation_prefix: &str,
            _derivation_suffix: &str,
            _unlocker_pub_key: &PublicKey,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal(
                "sign_input must not be called".to_string(),
            ))
        }

        async fn derive_wallet_payment_locking_script(
            &self,
            derivation_prefix: &str,
            derivation_suffix: &str,
            sender_identity_key: &PublicKey,
        ) -> WalletResult<Option<Vec<u8>>> {
            self.seen.lock().unwrap().push((
                derivation_prefix.to_string(),
                derivation_suffix.to_string(),
                sender_identity_key.to_der(),
            ));
            Ok(self.script.clone())
        }

        fn identity_public_key(&self) -> &PublicKey {
            &self.identity
        }
    }

    /// Provider that relies on the trait's DEFAULT
    /// `derive_wallet_payment_locking_script` (returns `Ok(None)`), proving
    /// the defaulted method is non-breaking for existing implementors.
    struct DefaultedProvider {
        identity: PublicKey,
    }

    #[async_trait]
    impl SigningProvider for DefaultedProvider {
        async fn derive_change_locking_script(
            &self,
            _derivation_prefix: &str,
            _derivation_suffix: &str,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal("not used".to_string()))
        }

        async fn sign_input(
            &self,
            _sighash: &[u8; 32],
            _sighash_type: u32,
            _derivation_prefix: &str,
            _derivation_suffix: &str,
            _unlocker_pub_key: &PublicKey,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal("not used".to_string()))
        }

        fn identity_public_key(&self) -> &PublicKey {
            &self.identity
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Build the expected BRC-29 locking script the way the SENDER does:
    /// `ScriptTemplateBRC29::lock(sender_priv, receiver_pub)` with the
    /// base64 string forms of the derivation params (legacy path from the
    /// TS twin's equivalence test).
    fn sender_lock_script(
        sender_priv: &PrivateKey,
        receiver_pub: &PublicKey,
        prefix_b64: &str,
        suffix_b64: &str,
    ) -> Vec<u8> {
        ScriptTemplateBRC29::new(prefix_b64.to_string(), suffix_b64.to_string())
            .lock(sender_priv, receiver_pub)
            .expect("sender-side BRC-29 lock")
    }

    /// Build a 1-in/1-out transaction paying `locking_script` and wrap it in
    /// an AtomicBEEF, returning (beef_bytes, txid).
    fn build_payment_beef(locking_script: &[u8], satoshis: u64) -> (Vec<u8>, String) {
        let mut tx = BsvTransaction::new();
        tx.version = 1;
        tx.lock_time = 0;

        tx.add_input(TransactionInput {
            source_transaction: None,
            source_txid: Some("b".repeat(64)),
            source_output_index: 0,
            unlocking_script: Some(UnlockingScript::from_binary(&[0x00])),
            sequence: 0xFFFFFFFF,
        });

        tx.add_output(TransactionOutput {
            satoshis: Some(satoshis),
            locking_script: LockingScript::from_binary(locking_script),
            change: false,
        });

        let txid = tx.id().expect("compute txid");

        let beef_tx = BeefTx::from_tx(tx, None).expect("create beef tx");
        let mut beef = Beef::new(bsv::transaction::beef::BEEF_V1);
        beef.txs.push(beef_tx);
        beef.atomic_txid = Some(txid.clone());

        let mut beef_bytes = Vec::new();
        beef.to_binary(&mut beef_bytes).expect("serialize beef");
        (beef_bytes, txid)
    }

    fn wallet_payment_args(
        beef_bytes: Vec<u8>,
        raw_prefix: &[u8],
        raw_suffix: &[u8],
        sender_pub: &PublicKey,
    ) -> ValidInternalizeActionArgs {
        ValidInternalizeActionArgs {
            tx: beef_bytes,
            description: "provider internalize test".to_string(),
            labels: vec![],
            outputs: vec![InternalizeOutput::WalletPayment {
                output_index: 0,
                payment: Payment {
                    derivation_prefix: raw_prefix.to_vec(),
                    derivation_suffix: raw_suffix.to_vec(),
                    sender_identity_key: sender_pub.clone(),
                },
            }],
        }
    }

    // -----------------------------------------------------------------------
    // 1. Equivalence: provider script == legacy scripts (pure derivation)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_provider_script_matches_legacy_derivations() {
        let sender_priv = PrivateKey::from_hex("11").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_priv = PrivateKey::from_hex("22").unwrap();
        let receiver_pub = receiver_priv.to_public_key();

        let prefix_b64 = b64(b"prefix-equiv");
        let suffix_b64 = b64(b"suffix-equiv");

        // (a) Sender-side legacy lock script.
        let sender_script =
            sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);

        // (b) Receiver-side local derivation (0.3.1 internalize path).
        let key_deriver = CachedKeyDeriver::new(receiver_priv.clone(), None);
        let key_id = format!("{prefix_b64} {suffix_b64}");
        let counterparty = Counterparty {
            counterparty_type: CounterpartyType::Other,
            public_key: Some(sender_pub.clone()),
        };
        let derived_pub = key_deriver
            .derive_public_key(&brc29_protocol(), &key_id, &counterparty, true)
            .expect("local derive_public_key");
        let hash_vec = derived_pub.to_hash();
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&hash_vec);
        let local_script = P2PKH::from_public_key_hash(hash)
            .lock()
            .expect("local P2PKH lock")
            .to_binary();

        // (c) StandardSigningProvider delegated derivation.
        let provider = StandardSigningProvider::new(
            CachedKeyDeriver::new(receiver_priv, None),
            receiver_pub.clone(),
        );
        let provider_script = provider
            .derive_wallet_payment_locking_script(&prefix_b64, &suffix_b64, &sender_pub)
            .await
            .expect("provider derivation")
            .expect("StandardSigningProvider must return Some");

        assert_eq!(
            sender_script, local_script,
            "sender-side lock and receiver-side local derivation must agree (BRC-42)"
        );
        assert_eq!(
            provider_script, local_script,
            "provider-derived script must be byte-identical to the local key_deriver path"
        );
        // 25-byte P2PKH shape: 76 a9 14 <hash20> 88 ac
        assert_eq!(provider_script.len(), 25);
        assert_eq!(&provider_script[..3], &[0x76, 0xa9, 0x14]);
        assert_eq!(&provider_script[23..], &[0x88, 0xac]);
    }

    // -----------------------------------------------------------------------
    // 2. Provider mode: matching script passes, short-circuits the deriver
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_provider_matching_script_accepts() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("33").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        let raw_prefix = b"prefix-ok";
        let raw_suffix = b"suffix-ok";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        let (beef_bytes, txid) = build_payment_beef(&lock, 1500);
        let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

        let provider = RecordingProvider::new(Some(lock.clone()), receiver_pub.clone());

        let result = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &setup.key_deriver,
            &setup.identity_key,
            &args,
            Some(&provider),
        )
        .await
        .expect("internalize with matching provider script must succeed");

        assert!(result.accepted);
        assert_eq!(result.txid, txid);

        // The provider saw exactly one call with the base64 STRING forms
        // and the sender identity key.
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, prefix_b64);
        assert_eq!(seen[0].1, suffix_b64);
        assert_eq!(seen[0].2, sender_pub.to_der());
    }

    /// Provider mode short-circuits the local deriver: a poisoned key_deriver
    /// (wrong root key — its local derivation would NOT match the output)
    /// is never consulted when the provider supplies the script.
    #[tokio::test]
    async fn test_provider_short_circuits_poisoned_deriver() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("44").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        let raw_prefix = b"prefix-sc";
        let raw_suffix = b"suffix-sc";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        let (beef_bytes, _txid) = build_payment_beef(&lock, 1500);
        let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

        // Poisoned deriver: unrelated root key. If the local path ran, the
        // derived script would mismatch and internalize would reject.
        let poisoned = CachedKeyDeriver::new(PrivateKey::from_hex("dead").unwrap(), None);

        let provider = RecordingProvider::new(Some(lock.clone()), receiver_pub);

        let result = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &poisoned,
            &setup.identity_key,
            &args,
            Some(&provider),
        )
        .await
        .expect("provider script must short-circuit the poisoned local deriver");

        assert!(result.accepted);
        assert_eq!(provider.seen.lock().unwrap().len(), 1);
    }

    // -----------------------------------------------------------------------
    // 3. Tamper case: provider returning a wrong script rejects
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_provider_wrong_script_rejects() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("55").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        let raw_prefix = b"prefix-bad";
        let raw_suffix = b"suffix-bad";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        let (beef_bytes, _txid) = build_payment_beef(&lock, 1500);
        let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

        // Wrong-but-plausible 25-byte P2PKH script.
        let mut wrong = lock.clone();
        wrong[10] ^= 0xFF;
        let provider = RecordingProvider::new(Some(wrong), receiver_pub);

        let err = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &setup.key_deriver,
            &setup.identity_key,
            &args,
            Some(&provider),
        )
        .await
        .expect_err("mismatched provider script must be rejected");

        let msg = format!("{err}");
        assert!(
            msg.contains("doesn't match BRC-29 derivation"),
            "unexpected error: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Fallback: Ok(None) / no provider == 0.3.1 local behavior
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_fallback_none_uses_local_derivation() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("66").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        let raw_prefix = b"prefix-fb";
        let raw_suffix = b"suffix-fb";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        let (beef_bytes, txid) = build_payment_beef(&lock, 2500);
        let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

        // Provider explicitly declines (returns Ok(None)) → local path runs.
        let provider = RecordingProvider::new(None, receiver_pub.clone());

        let result = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &setup.key_deriver,
            &setup.identity_key,
            &args,
            Some(&provider),
        )
        .await
        .expect("Ok(None) must fall back to the local key_deriver path");

        assert!(result.accepted);
        assert_eq!(result.txid, txid);
        assert_eq!(provider.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_no_provider_matches_defaulted_provider_behavior() {
        // Same payment internalized twice (fresh wallets with the same root
        // key): once with signing_provider = None, once with a provider that
        // relies on the trait's DEFAULT method (Ok(None)). Both must accept —
        // proving the defaulted method is byte-compatible with 0.3.1.
        let root = common::random_root_key();

        let sender_priv = PrivateKey::from_hex("77").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = root.to_public_key();

        let raw_prefix = b"prefix-def";
        let raw_suffix = b"suffix-def";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);

        for use_defaulted_provider in [false, true] {
            let setup = common::create_test_wallet_with_key(root.clone()).await;
            let services = MockWalletServices;
            let (beef_bytes, txid) = build_payment_beef(&lock, 3500);
            let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

            let defaulted = DefaultedProvider {
                identity: receiver_pub.clone(),
            };
            let provider: Option<&dyn SigningProvider> = if use_defaulted_provider {
                Some(&defaulted)
            } else {
                None
            };

            let result = signer_internalize_action(
                setup.storage.as_ref(),
                &services,
                &setup.key_deriver,
                &setup.identity_key,
                &args,
                provider,
            )
            .await
            .expect("local derivation path must accept the matching payment");

            assert!(result.accepted);
            assert_eq!(result.txid, txid);
        }
    }

    /// Tampered locking script is still rejected on the fallback path
    /// (0.3.1 security behavior preserved).
    #[tokio::test]
    async fn test_fallback_tampered_script_rejects() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("88").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        let raw_prefix = b"prefix-tmp";
        let raw_suffix = b"suffix-tmp";
        let prefix_b64 = b64(raw_prefix);
        let suffix_b64 = b64(raw_suffix);

        let mut lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        lock[10] ^= 0xFF; // tamper with the pubkey hash

        let (beef_bytes, _txid) = build_payment_beef(&lock, 1500);
        let args = wallet_payment_args(beef_bytes, raw_prefix, raw_suffix, &sender_pub);

        let err = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &setup.key_deriver,
            &setup.identity_key,
            &args,
            None,
        )
        .await
        .expect_err("tampered locking script must be rejected on the local path");

        let msg = format!("{err}");
        assert!(
            msg.contains("doesn't match BRC-29 derivation"),
            "unexpected error: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Base64 string preservation end-to-end (non-UTF-8 raw bytes)
    // -----------------------------------------------------------------------

    /// Regression twin of the storage-layer non-UTF-8 test: raw derivation
    /// bytes that `from_utf8_lossy` would corrupt to U+FFFD must reach the
    /// provider as their exact base64 encodings, and the payment must verify.
    #[tokio::test]
    async fn test_provider_receives_base64_for_non_utf8_params() {
        let setup = common::create_test_wallet().await;
        let services = MockWalletServices;

        let sender_priv = PrivateKey::from_hex("99").unwrap();
        let sender_pub = sender_priv.to_public_key();
        let receiver_pub = setup.key_deriver.root_key().to_public_key();

        // Bytes outside ASCII, mostly invalid UTF-8 start bytes.
        let raw_prefix: Vec<u8> = vec![0xFF, 0xFE, 0xFD, 0x80, 0xC0, 0xC1, 0xF5, 0xF6];
        let raw_suffix: Vec<u8> = vec![0x81, 0x82, 0x83, 0x84, 0xE0, 0xE1, 0xE2, 0xE3];
        let prefix_b64 = b64(&raw_prefix);
        let suffix_b64 = b64(&raw_suffix);

        let lock = sender_lock_script(&sender_priv, &receiver_pub, &prefix_b64, &suffix_b64);
        let (beef_bytes, txid) = build_payment_beef(&lock, 4500);
        let args = wallet_payment_args(beef_bytes, &raw_prefix, &raw_suffix, &sender_pub);

        // Delegate to a real StandardSigningProvider via a recording wrapper:
        // record args, then return the standard provider's derivation.
        let std_provider = StandardSigningProvider::new(
            CachedKeyDeriver::new(setup.key_deriver.root_key().clone(), None),
            receiver_pub.clone(),
        );
        let expected_script = std_provider
            .derive_wallet_payment_locking_script(&prefix_b64, &suffix_b64, &sender_pub)
            .await
            .unwrap()
            .unwrap();
        let provider = RecordingProvider::new(Some(expected_script), receiver_pub);

        let result = signer_internalize_action(
            setup.storage.as_ref(),
            &services,
            &setup.key_deriver,
            &setup.identity_key,
            &args,
            Some(&provider),
        )
        .await
        .expect("non-UTF-8 derivation params must round-trip via base64");

        assert!(result.accepted);
        assert_eq!(result.txid, txid);

        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].0, prefix_b64,
            "derivation_prefix must be the exact base64 encoding, not from_utf8_lossy"
        );
        assert_eq!(
            seen[0].1, suffix_b64,
            "derivation_suffix must be the exact base64 encoding, not from_utf8_lossy"
        );
    }
}
