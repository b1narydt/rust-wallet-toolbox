//! Injection tests for `WalletArgs::signing_provider` and the relaxed key deriver.
//!
//! Before 0.4.0 `Wallet` built its own `DefaultWalletSigner` and passed `None`
//! for the signing provider, so it supported exactly one custody model: a local
//! root private key. These tests pin the two halves of the fix:
//!
//! 1. A `SigningProvider` injected via `WalletArgs` / `WalletBuilder` is
//!    actually consulted — `derive_change_locking_script` and `sign_input` are
//!    called, and the scripts that land in the transaction are the provider's,
//!    not ones the local key deriver could have produced.
//! 2. `Arc<dyn KeyDeriverApi>` is accepted everywhere `Arc<CachedKeyDeriver>`
//!    used to be, including a deriver the caller built themselves.
//!
//! Plus the invariant that makes deferred signing work at all: `createAction`
//! and `signAction` share ONE `pending_sign_actions` map, because the provider
//! is injected INTO the wallet's single signer rather than bolted alongside it.

mod common;

#[cfg(feature = "sqlite")]
mod wallet_signing_provider_tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::Utc;

    use bsv::primitives::hash::sha256_hmac;
    use bsv::primitives::private_key::PrivateKey;
    use bsv::primitives::public_key::PublicKey;
    use bsv::primitives::symmetric_key::SymmetricKey;
    use bsv::transaction::beef::Beef;
    use bsv::transaction::transaction::Transaction as BsvTransaction;
    use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
    use bsv::wallet::error::WalletError as SdkWalletError;
    use bsv::wallet::interfaces::{
        CreateActionArgs, CreateActionOptions, CreateActionOutput, CreateHmacArgs,
        CreateSignatureArgs, DecryptArgs, EncryptArgs, GetPublicKeyArgs,
        RevealCounterpartyKeyLinkageArgs, RevealSpecificKeyLinkageArgs, VerifyHmacArgs,
        VerifySignatureArgs, WalletInterface,
    };
    use bsv::wallet::proto_wallet::ProtoWallet;
    use bsv::wallet::types::{BooleanDefaultTrue, Counterparty, CounterpartyType, Protocol};
    use bsv::wallet::KeyDeriverApi;

    use bsv_wallet_toolbox::error::{WalletError, WalletResult};
    use bsv_wallet_toolbox::signer::signing_context::{
        CallerRef, ContextualWallet, SigningContext,
    };
    use bsv_wallet_toolbox::signer::signing_provider::SigningProvider;
    use bsv_wallet_toolbox::signer::standard_provider::StandardSigningProvider;
    use bsv_wallet_toolbox::status::TransactionStatus;
    use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
    use bsv_wallet_toolbox::tables::{Output, OutputBasket, Transaction};
    use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
    use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
    use bsv_wallet_toolbox::wallet::setup::{SetupWallet, WalletBuilder};

    use super::common::{self, MockWalletServices};

    // -----------------------------------------------------------------------
    // Recording provider
    // -----------------------------------------------------------------------

    /// A `SigningProvider` that counts every hook it is asked for and answers
    /// from its OWN key — deliberately a different key than the wallet's.
    ///
    /// Real derivation and real signatures (it delegates to
    /// `StandardSigningProvider`), so the transaction it produces has to be
    /// valid. But because the key is not the wallet's, every script it emits is
    /// one the local derivation path provably could not have produced — which
    /// is what turns "the provider was consulted, and local derivation did NOT
    /// happen" into a checkable claim rather than an assumption.
    struct RecordingProvider {
        inner: StandardSigningProvider,
        root_key: PrivateKey,
        identity_pub_key: PublicKey,
        /// Every (derivation_prefix, derivation_suffix) it was asked to lock.
        seen_change: Mutex<Vec<(String, String)>>,
        /// Every `CallerRef` handed to a ceremony-driving method
        /// (`prepare_spend_contexts` and `sign_input`), in call order.
        seen_callers: Mutex<Vec<CallerRef>>,
        derive_change_calls: AtomicUsize,
        sign_input_calls: AtomicUsize,
        prepare_spend_context_calls: AtomicUsize,
        derive_public_key_calls: AtomicUsize,
        derive_symmetric_key_calls: AtomicUsize,
        create_signature_calls: AtomicUsize,
    }

    impl RecordingProvider {
        fn new(root_key: PrivateKey) -> Self {
            let identity_pub_key = root_key.to_public_key();
            Self {
                inner: StandardSigningProvider::new(
                    Arc::new(CachedKeyDeriver::new(root_key.clone(), None)),
                    identity_pub_key.clone(),
                ),
                root_key,
                identity_pub_key,
                seen_change: Mutex::new(Vec::new()),
                seen_callers: Mutex::new(Vec::new()),
                derive_change_calls: AtomicUsize::new(0),
                sign_input_calls: AtomicUsize::new(0),
                prepare_spend_context_calls: AtomicUsize::new(0),
                derive_public_key_calls: AtomicUsize::new(0),
                derive_symmetric_key_calls: AtomicUsize::new(0),
                create_signature_calls: AtomicUsize::new(0),
            }
        }

        /// The provider's own deriver — the reference for what its key
        /// operations must have produced.
        fn deriver(&self) -> &dyn KeyDeriverApi {
            self.inner.key_deriver()
        }

        fn derive_changes(&self) -> usize {
            self.derive_change_calls.load(Ordering::SeqCst)
        }

        fn signs(&self) -> usize {
            self.sign_input_calls.load(Ordering::SeqCst)
        }

        /// The change locking script this provider returns for a given
        /// derivation pair — locked with ITS root key, not the wallet's.
        fn change_script(&self, prefix: &str, suffix: &str) -> Vec<u8> {
            brc29_lock(&self.root_key, &self.identity_pub_key, prefix, suffix)
        }
    }

    #[async_trait]
    impl SigningProvider for RecordingProvider {
        async fn derive_change_locking_script(
            &self,
            derivation_prefix: &str,
            derivation_suffix: &str,
        ) -> WalletResult<Vec<u8>> {
            self.derive_change_calls.fetch_add(1, Ordering::SeqCst);
            self.seen_change
                .lock()
                .unwrap()
                .push((derivation_prefix.to_string(), derivation_suffix.to_string()));
            self.inner
                .derive_change_locking_script(derivation_prefix, derivation_suffix)
                .await
        }

        async fn sign_input(
            &self,
            sighash: &[u8; 32],
            sighash_type: u32,
            derivation_prefix: &str,
            derivation_suffix: &str,
            unlocker_pub_key: &PublicKey,
            ctx: &SigningContext,
        ) -> WalletResult<Vec<u8>> {
            self.sign_input_calls.fetch_add(1, Ordering::SeqCst);
            self.seen_callers.lock().unwrap().push(ctx.caller.clone());
            self.inner
                .sign_input(
                    sighash,
                    sighash_type,
                    derivation_prefix,
                    derivation_suffix,
                    unlocker_pub_key,
                    ctx,
                )
                .await
        }

        async fn prepare_spend_contexts(
            &self,
            _tx: &BsvTransaction,
            _pending_inputs: &[bsv_wallet_toolbox::signer::types::PendingStorageInput],
            ctx: &SigningContext,
        ) -> WalletResult<()> {
            self.prepare_spend_context_calls
                .fetch_add(1, Ordering::SeqCst);
            self.seen_callers.lock().unwrap().push(ctx.caller.clone());
            Ok(())
        }

        async fn derive_public_key(
            &self,
            protocol: &Protocol,
            key_id: &str,
            counterparty: &Counterparty,
            for_self: bool,
        ) -> WalletResult<PublicKey> {
            self.derive_public_key_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .derive_public_key(protocol, key_id, counterparty, for_self)
                .await
        }

        async fn derive_symmetric_key(
            &self,
            protocol: &Protocol,
            key_id: &str,
            counterparty: &Counterparty,
        ) -> WalletResult<[u8; 32]> {
            self.derive_symmetric_key_calls
                .fetch_add(1, Ordering::SeqCst);
            self.inner
                .derive_symmetric_key(protocol, key_id, counterparty)
                .await
        }

        async fn create_signature(
            &self,
            protocol: &Protocol,
            key_id: &str,
            counterparty: &Counterparty,
            digest: &[u8; 32],
            ctx: &SigningContext,
        ) -> WalletResult<Vec<u8>> {
            self.create_signature_calls.fetch_add(1, Ordering::SeqCst);
            self.inner
                .create_signature(protocol, key_id, counterparty, digest, ctx)
                .await
        }

        fn identity_public_key(&self) -> &PublicKey {
            &self.identity_pub_key
        }
    }

    // -----------------------------------------------------------------------
    // Fixtures
    // -----------------------------------------------------------------------

    const SEED_PREFIX: &str = "c2VlZHByZWZpeA==";
    const SEED_SUFFIX: &str = "c2VlZHN1ZmZpeA==";

    /// Seed spendable BRC-29 change UTXOs the wallet's UTXO selection will pick
    /// up, so `createAction` can actually fund a transaction offline.
    ///
    /// The funding transaction is a real serialized transaction with no inputs,
    /// stored with its true txid and raw bytes. That is what lets the signer
    /// assemble a BEEF for the spend entirely from local storage — no merkle
    /// proof service, no network.
    async fn seed_spendable_change(
        storage: &WalletStorageManager,
        identity_key: &str,
        count: usize,
        satoshis: i64,
        locking_script: Vec<u8>,
    ) {
        use bsv::script::locking_script::LockingScript;
        use bsv::transaction::transaction_output::TransactionOutput;

        let now = Utc::now().naive_utc();
        let (user, _) = storage
            .find_or_insert_user(identity_key)
            .await
            .expect("find_or_insert_user");

        let basket_id = storage
            .insert_output_basket(&OutputBasket {
                created_at: now,
                updated_at: now,
                basket_id: 0,
                user_id: user.user_id,
                name: "default".to_string(),
                number_of_desired_utxos: 10,
                minimum_desired_utxo_value: 1000,
                is_deleted: false,
            })
            .await
            .expect("insert basket");

        // Build the funding transaction the seeded UTXOs actually come from.
        let mut funding = BsvTransaction::new();
        for _ in 0..count {
            funding.add_output(TransactionOutput {
                satoshis: Some(satoshis as u64),
                locking_script: LockingScript::from_binary(&locking_script),
                change: false,
            });
        }
        let mut funding_raw = Vec::new();
        funding
            .to_binary(&mut funding_raw)
            .expect("serialize funding tx");
        let funding_txid = funding.id().expect("funding txid");

        let tx_id = storage
            .insert_transaction(&Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: format!("seed-{}", rand::random::<u32>()),
                is_outgoing: false,
                satoshis: satoshis * count as i64,
                description: "seed funding".to_string(),
                version: Some(funding.version as i32),
                lock_time: Some(funding.lock_time as i32),
                txid: Some(funding_txid.clone()),
                input_beef: None,
                raw_tx: Some(funding_raw),
            })
            .await
            .expect("insert funding tx");

        for i in 0..count {
            storage
                .insert_output(&Output {
                    created_at: now,
                    updated_at: now,
                    output_id: 0,
                    user_id: user.user_id,
                    transaction_id: tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: true,
                    output_description: Some(format!("seed change {i}")),
                    vout: i as i32,
                    satoshis,
                    provided_by: StorageProvidedBy::Storage,
                    purpose: "change".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(funding_txid.clone()),
                    sender_identity_key: None,
                    derivation_prefix: Some(SEED_PREFIX.to_string()),
                    derivation_suffix: Some(SEED_SUFFIX.to_string()),
                    custom_instructions: None,
                    spent_by: None,
                    sequence_number: None,
                    spending_description: None,
                    script_length: Some(locking_script.len() as i64),
                    script_offset: None,
                    locking_script: Some(locking_script.clone()),
                })
                .await
                .expect("insert seed utxo");
        }
    }

    /// A funded wallet, optionally with a delegated custody backend.
    ///
    /// The seeded UTXOs are locked to whichever key will have to unlock them:
    /// the provider's when custody is delegated, the wallet's own otherwise.
    /// That mirrors reality — a delegated wallet's coins were locked by the
    /// provider — and it means a signature only validates if the right backend
    /// produced it.
    async fn funded_wallet(
        root_key: PrivateKey,
        provider: Option<Arc<RecordingProvider>>,
    ) -> SetupWallet {
        let lock_key = match &provider {
            Some(p) => p.root_key.clone(),
            None => root_key.clone(),
        };

        let mut builder = WalletBuilder::new()
            .chain(Chain::Test)
            .root_key(root_key)
            .with_sqlite_memory()
            .with_services(Arc::new(MockWalletServices))
            .without_monitor();
        if let Some(p) = provider {
            builder = builder.with_signing_provider(p);
        }
        let setup = builder.build().await.expect("build funded wallet");

        let lock = brc29_lock(
            &lock_key,
            &lock_key.to_public_key(),
            SEED_PREFIX,
            SEED_SUFFIX,
        );
        seed_spendable_change(&setup.storage, &setup.identity_key, 3, 50_000, lock).await;
        setup
    }

    /// A createAction paying a fixed external P2PKH, which forces storage to
    /// allocate both a change input and a change output.
    fn payment_args(sign_and_process: bool) -> CreateActionArgs {
        CreateActionArgs {
            description: "provider injection test".to_string(),
            inputs: vec![],
            outputs: vec![CreateActionOutput {
                locking_script: Some(vec![
                    0x76, 0xa9, 0x14, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
                    0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0x88, 0xac,
                ]),
                satoshis: 5_000,
                output_description: "payment".to_string(),
                basket: None,
                custom_instructions: None,
                tags: vec![],
            }],
            lock_time: None,
            version: None,
            labels: vec![],
            options: Some(CreateActionOptions {
                sign_and_process: BooleanDefaultTrue(Some(sign_and_process)),
                ..Default::default()
            }),
            input_beef: None,
            reference: None,
        }
    }

    /// Extract the subject transaction from an Atomic BEEF result.
    fn subject_tx(beef_bytes: &[u8]) -> BsvTransaction {
        Beef::from_binary(&mut Cursor::new(beef_bytes))
            .expect("parse result BEEF")
            .into_transaction()
            .expect("subject transaction")
    }

    // -----------------------------------------------------------------------
    // 1. The injected provider is consulted, and local derivation does not run
    // -----------------------------------------------------------------------

    /// An injected `SigningProvider` must own BOTH custody decisions in
    /// `createAction`: the change output's locking script and every input
    /// signature. Asserted on the produced transaction, not just call counts —
    /// the scripts must be the provider's, which the wallet's own root key
    /// could never have derived.
    #[tokio::test]
    async fn injected_provider_derives_change_and_signs_inputs() {
        let wallet_root = common::random_root_key();
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = funded_wallet(wallet_root.clone(), Some(provider.clone())).await;

        let result = setup
            .wallet
            .create_action(payment_args(true), None)
            .await
            .expect("delegated createAction should succeed");

        // Both hooks fired.
        assert!(
            provider.derive_changes() >= 1,
            "provider.derive_change_locking_script must be called for the change output"
        );
        assert!(
            provider.signs() >= 1,
            "provider.sign_input must be called for each BRC-29 input"
        );
        assert_eq!(
            provider.prepare_spend_context_calls.load(Ordering::SeqCst),
            1,
            "the spend-context hook must fire exactly once before signing"
        );

        // And the transaction carries the provider's work, not local derivation's.
        let tx = subject_tx(&result.tx.expect("signed tx BEEF"));
        let scripts: Vec<Vec<u8>> = tx
            .outputs
            .iter()
            .map(|o| o.locking_script.to_binary())
            .collect();

        let seen = provider.seen_change.lock().unwrap().clone();
        assert!(!seen.is_empty(), "provider must have been asked for change");
        let wallet_identity = wallet_root.to_public_key();
        for (prefix, suffix) in &seen {
            let provider_script = provider.change_script(prefix, suffix);
            let local_script = brc29_lock(&wallet_root, &wallet_identity, prefix, suffix);
            assert_ne!(
                provider_script, local_script,
                "fixture is only meaningful if the two keys derive differently"
            );
            assert!(
                scripts.contains(&provider_script),
                "the change output must be locked by the PROVIDER's derivation"
            );
            assert!(
                !scripts.contains(&local_script),
                "no output may carry a script derived from the wallet's local root key"
            );
        }

        // Every input is unlocked by a key derived from the PROVIDER's root
        // key. The public key pushed in the unlocking script must hash to the
        // pubkey hash the seeded UTXO was locked to — which only the provider's
        // derivation can satisfy.
        let expected_hash = seed_pubkey_hash(&provider.root_key);
        assert!(!tx.inputs.is_empty(), "expected a funded input to sign");
        for (i, input) in tx.inputs.iter().enumerate() {
            let unlock = input
                .unlocking_script
                .as_ref()
                .expect("input must be signed")
                .to_binary();
            assert_eq!(
                unlocking_script_pubkey_hash(&unlock),
                expected_hash,
                "input {i} must be unlocked by the provider's derived key"
            );
            assert_ne!(
                unlocking_script_pubkey_hash(&unlock),
                seed_pubkey_hash(&wallet_root),
                "input {i} must NOT be unlocked by the wallet's local root key"
            );
        }
    }

    /// The mirror image: with no provider injected, behaviour is exactly as it
    /// always was — the change output is locked by local root-key derivation.
    #[tokio::test]
    async fn without_provider_change_is_derived_locally() {
        let wallet_root = common::random_root_key();
        let setup = funded_wallet(wallet_root.clone(), None).await;

        let result = setup
            .wallet
            .create_action(payment_args(true), None)
            .await
            .expect("local createAction should succeed");

        let txid = result.txid.clone().expect("signed txid");
        let tx = subject_tx(&result.tx.expect("signed tx BEEF"));
        let scripts: Vec<Vec<u8>> = tx
            .outputs
            .iter()
            .map(|o| o.locking_script.to_binary())
            .collect();
        assert!(
            scripts.contains(&payment_script()),
            "the requested payment output must be present unchanged"
        );

        // Recompute, from what storage recorded, the exact script the local
        // BRC-29 path should have produced for each change output.
        let derivations = change_derivations(&setup.storage, &txid).await;
        assert!(!derivations.is_empty(), "expected a change output");
        let identity = wallet_root.to_public_key();
        for (prefix, suffix) in &derivations {
            assert!(
                scripts.contains(&brc29_lock(&wallet_root, &identity, prefix, suffix)),
                "without a provider, change must be locked by the local root key"
            );
        }

        // Inputs are unlocked by the wallet's own derived key.
        let expected_hash = seed_pubkey_hash(&wallet_root);
        assert!(!tx.inputs.is_empty(), "expected a funded input to sign");
        for input in &tx.inputs {
            let unlock = input
                .unlocking_script
                .as_ref()
                .expect("input must be signed")
                .to_binary();
            assert_eq!(unlocking_script_pubkey_hash(&unlock), expected_hash);
        }

        assert!(
            setup.wallet.signing_provider().is_none(),
            "no provider should be attached"
        );
    }

    /// The 🔴 single-map invariant: `createAction` in delayed-signing mode
    /// records the pending action, and `signAction` must find it by reference.
    ///
    /// A provider bolted alongside the wallet's signer instead of injected into
    /// it would give each its own `pending_sign_actions` map, and this lookup
    /// would fail with "a reference for an existing unsigned transaction".
    #[tokio::test]
    async fn delegated_sign_action_finds_the_pending_reference() {
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = funded_wallet(common::random_root_key(), Some(provider.clone())).await;

        // sign_and_process = false → deferred signing, pending action recorded.
        let created = setup
            .wallet
            .create_action(payment_args(false), None)
            .await
            .expect("delegated deferred createAction should succeed");

        let signable = created
            .signable_transaction
            .expect("deferred createAction must return a signable transaction");
        assert_eq!(
            provider.signs(),
            0,
            "deferred createAction must not sign anything yet"
        );

        // The unsigned transaction handed back already carries the provider's
        // change lock, so the deferred branch routes through the provider too.
        let unsigned = subject_tx(&signable.tx);
        let seen = provider.seen_change.lock().unwrap().clone();
        assert!(!seen.is_empty(), "provider must have derived the change");
        let scripts: Vec<Vec<u8>> = unsigned
            .outputs
            .iter()
            .map(|o| o.locking_script.to_binary())
            .collect();
        for (prefix, suffix) in &seen {
            assert!(
                scripts.contains(&provider.change_script(prefix, suffix)),
                "the deferred signable transaction must carry the provider's change lock"
            );
        }

        // The pending action lives in the signer's map, keyed by this reference.
        // `signAction` reads that same map — see
        // `delegated_sign_action_routes_signing_through_the_provider` for the
        // signing half, which BRC-100 forbids reaching from here (signAction
        // requires at least one caller-supplied spend, and this action has none).
        assert!(
            !signable.reference.is_empty(),
            "deferred createAction must return the reference signAction will look up"
        );
    }

    /// `signAction` routes input signing through the injected provider.
    ///
    /// Driven at the signer level because BRC-100 rejects a `signAction` whose
    /// `spends` map is empty, and an action funded purely from storage change
    /// has no caller-supplied input to spend.
    #[tokio::test]
    async fn delegated_sign_action_routes_signing_through_the_provider() {
        use bsv_wallet_toolbox::signer::backend::SigningBackend;
        use bsv_wallet_toolbox::signer::methods::sign_action::signer_sign_action;

        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = funded_wallet(common::random_root_key(), Some(provider.clone())).await;

        // Produce a real pending action by running the delegated createAction
        // pipeline in deferred mode, then rebuild it from storage the same way
        // an out-of-session signAction would.
        let created = setup
            .wallet
            .create_action(payment_args(false), None)
            .await
            .expect("deferred createAction");
        let signable = created.signable_transaction.expect("signable transaction");
        let reference = String::from_utf8(signable.reference).expect("reference is utf8");
        assert_eq!(provider.signs(), 0, "nothing signed yet");

        let pending = pending_from_signable(&reference, &signable.tx, &provider.root_key);
        let result = signer_sign_action(
            setup.storage.as_ref(),
            &MockWalletServices,
            &SigningBackend::Delegated(provider.as_ref()),
            &setup.identity_key,
            &valid_sign_action_args(&reference),
            &pending,
            &SigningContext::itself(),
        )
        .await
        .expect("delegated signer_sign_action");

        assert!(
            provider.signs() >= 1,
            "signAction must route input signing through the injected provider"
        );
        let tx = subject_tx(&result.tx.expect("signed tx BEEF"));
        let expected_hash = seed_pubkey_hash(&provider.root_key);
        assert!(!tx.inputs.is_empty(), "expected a funded input to sign");
        for input in &tx.inputs {
            assert_eq!(
                unlocking_script_pubkey_hash(&input.unlocking_script.as_ref().unwrap().to_binary()),
                expected_hash,
                "signAction inputs must be unlocked by the provider's derived key"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 1b. The caller context reaches the signing seam
    // -----------------------------------------------------------------------

    /// `ContextualWallet::create_action_in` must deliver the transport-
    /// authenticated caller, unchanged, to every ceremony-driving provider
    /// method (`prepare_spend_contexts`, then `sign_input` per input).
    #[tokio::test]
    async fn create_action_in_threads_authenticated_caller_to_provider() {
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = funded_wallet(common::random_root_key(), Some(provider.clone())).await;

        // Any string the transport vouched for; the toolbox must not interpret it.
        let principal = "02a1b2c3d4e5f60718293a4b5c6d7e8f9091a2b3c4d5e6f708192a3b4c5d6e7f80";
        setup
            .wallet
            .create_action_in(
                payment_args(true),
                None,
                &SigningContext::authenticated(principal),
            )
            .await
            .expect("delegated create_action_in should succeed");

        let seen = provider.seen_callers.lock().unwrap().clone();
        assert!(
            seen.len() >= 2,
            "prepare_spend_contexts and at least one sign_input must fire; saw {seen:?}"
        );
        for caller in &seen {
            assert_eq!(
                caller,
                &CallerRef::Authenticated(principal.to_string()),
                "every ceremony-driving call must carry the authenticated caller"
            );
        }
    }

    /// The plain BRC-100 `WalletInterface::create_action` has no caller
    /// parameter, so it must reach the seam as `CallerRef::Itself` — the
    /// wallet's own authority stated explicitly, never an inherited default.
    #[tokio::test]
    async fn wallet_interface_create_action_reaches_seam_as_itself() {
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = funded_wallet(common::random_root_key(), Some(provider.clone())).await;

        setup
            .wallet
            .create_action(payment_args(true), None)
            .await
            .expect("delegated createAction should succeed");

        let seen = provider.seen_callers.lock().unwrap().clone();
        assert!(
            seen.len() >= 2,
            "prepare_spend_contexts and at least one sign_input must fire; saw {seen:?}"
        );
        for caller in &seen {
            assert_eq!(
                caller,
                &CallerRef::Itself,
                "the BRC-100 surface must sign as the wallet itself"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 2. The key deriver is a trait object now
    // -----------------------------------------------------------------------

    /// `Arc<dyn KeyDeriverApi>` is accepted everywhere `Arc<CachedKeyDeriver>`
    /// used to be — through `WalletBuilder::key_deriver` and through
    /// `WalletArgs` directly, without a root key ever being handed to the
    /// builder.
    #[tokio::test]
    async fn wallet_accepts_arc_dyn_key_deriver() {
        use bsv_wallet_toolbox::wallet::types::WalletArgs;
        use bsv_wallet_toolbox::wallet::wallet::Wallet;

        let root_key = common::random_root_key();
        // The coercion under test: a concrete deriver behind the trait object.
        let key_deriver: Arc<dyn KeyDeriverApi> = Arc::new(CachedKeyDeriver::new(root_key, None));
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));

        let setup = WalletBuilder::new()
            .chain(Chain::Test)
            .key_deriver(key_deriver.clone())
            .with_signing_provider(provider.clone())
            .with_sqlite_memory()
            .with_services(Arc::new(MockWalletServices))
            .without_monitor()
            .build()
            .await
            .expect("WalletBuilder must accept Arc<dyn KeyDeriverApi> with no root_key");

        assert_eq!(setup.identity_key, key_deriver.identity_key().to_der_hex());
        assert!(setup.wallet.signing_provider().is_some());

        // ... and the same deriver goes straight into WalletArgs.
        let wallet = Wallet::new(WalletArgs {
            chain: setup.chain.clone(),
            key_deriver,
            signing_provider: Some(provider),
            storage: setup.storage.clone(),
            services: setup.services.clone(),
            monitor: None,
            privileged_key_manager: None,
            settings_manager: None,
            lookup_resolver: None,
        })
        .expect("WalletArgs must accept Arc<dyn KeyDeriverApi>");

        assert_eq!(wallet.identity_key.to_der_hex(), setup.identity_key);
    }

    /// The two `Wallet` helpers that lock BRC-29 outputs with the local root key
    /// have no provider equivalent, so under delegation they must fail closed
    /// rather than hand back a key that locks nothing the wallet can spend.
    #[tokio::test]
    async fn root_key_helpers_fail_closed_under_delegation() {
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let delegated = funded_wallet(common::random_root_key(), Some(provider)).await;
        let local = funded_wallet(common::random_root_key(), None).await;

        assert!(
            delegated.wallet.get_client_change_key_pair().is_err(),
            "get_client_change_key_pair must refuse to expose a root key under delegation"
        );
        assert!(
            local.wallet.get_client_change_key_pair().is_ok(),
            "local wallets keep the existing behaviour"
        );

        assert!(
            delegated.wallet.sweep_to(&local.wallet).await.is_err(),
            "sweep_to locks with the local root key, so it must refuse under delegation"
        );
    }

    // -----------------------------------------------------------------------
    // 3. The nine BRC-100 crypto methods route through the provider
    // -----------------------------------------------------------------------

    fn crypto_protocol() -> Protocol {
        Protocol {
            security_level: 2,
            protocol: "delegated crypto tests".to_string(),
        }
    }

    fn self_cp() -> Counterparty {
        Counterparty {
            counterparty_type: CounterpartyType::Self_,
            public_key: None,
        }
    }

    /// A wallet for pure-crypto tests: no seeded UTXOs needed.
    async fn crypto_wallet(
        root_key: PrivateKey,
        provider: Option<Arc<dyn SigningProvider>>,
    ) -> SetupWallet {
        let mut builder = WalletBuilder::new()
            .chain(Chain::Test)
            .root_key(root_key)
            .with_sqlite_memory()
            .with_services(Arc::new(MockWalletServices))
            .without_monitor();
        if let Some(p) = provider {
            builder = builder.with_signing_provider(p);
        }
        builder.build().await.expect("build crypto wallet")
    }

    fn get_public_key_args(identity_key: bool, key_id: &str) -> GetPublicKeyArgs {
        GetPublicKeyArgs {
            identity_key,
            protocol_id: (!identity_key).then(crypto_protocol),
            key_id: (!identity_key).then(|| key_id.to_string()),
            counterparty: (!identity_key).then(self_cp),
            privileged: false,
            privileged_reason: None,
            for_self: (!identity_key).then_some(true),
            seek_permission: None,
        }
    }

    /// An injected provider must answer ALL nine BRC-100 crypto methods; the
    /// wallet's internal ProtoWallet (built over the local root key) must not.
    /// Attribution is by key, exactly like the transaction tests: the provider
    /// holds a DIFFERENT key than the wallet, so every answer is checked to be
    /// one only the provider's key can produce — plus the recording counters
    /// prove the provider's three key operations were actually consulted.
    #[tokio::test]
    async fn injected_provider_answers_all_nine_crypto_methods() {
        let wallet_root = common::random_root_key();
        let provider = Arc::new(RecordingProvider::new(common::random_root_key()));
        let setup = crypto_wallet(wallet_root.clone(), Some(provider.clone())).await;
        let wallet = &setup.wallet;

        let protocol = crypto_protocol();
        let key_id = "delegated-crypto-1";
        let local_deriver = CachedKeyDeriver::new(wallet_root.clone(), None);
        let local_proto = ProtoWallet::new(wallet_root.clone());

        // 1. getPublicKey (identity): the provider's identity, not the local root's.
        let identity = wallet
            .get_public_key(get_public_key_args(true, ""), None)
            .await
            .expect("delegated identity getPublicKey");
        assert_eq!(
            identity.public_key.to_der_hex(),
            provider.identity_public_key().to_der_hex()
        );
        assert_ne!(
            identity.public_key.to_der_hex(),
            wallet_root.to_public_key().to_der_hex(),
            "identity must come from the provider, not the wallet's local root key"
        );

        // 2. getPublicKey (derived): byte-identical to the provider's own
        // derivation, and provably not the local deriver's.
        let derived = wallet
            .get_public_key(get_public_key_args(false, key_id), None)
            .await
            .expect("delegated derived getPublicKey");
        let provider_derived = provider
            .deriver()
            .derive_public_key(&protocol, key_id, &self_cp(), true)
            .expect("provider-side reference derivation");
        assert_eq!(
            derived.public_key.to_der_hex(),
            provider_derived.to_der_hex()
        );
        let local_derived = local_deriver
            .derive_public_key(&protocol, key_id, &self_cp(), true)
            .expect("local reference derivation");
        assert_ne!(
            derived.public_key.to_der_hex(),
            local_derived.to_der_hex(),
            "derived key must not be the local root's derivation"
        );

        // 3 + 4. createSignature / verifySignature round-trip through the
        // wallet; the signature must NOT verify under the local root's
        // derivation, proving the provider's key signed.
        let data = b"delegated signature data".to_vec();
        let sig = wallet
            .create_signature(
                CreateSignatureArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: Some(data.clone()),
                    hash_to_directly_sign: None,
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated createSignature");
        let verified = wallet
            .verify_signature(
                VerifySignatureArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: Some(data.clone()),
                    hash_to_directly_verify: None,
                    signature: sig.signature.clone(),
                    for_self: Some(true),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated verifySignature");
        assert!(verified.valid);
        let local_verifies = local_proto
            .verify_signature_sync(
                Some(&data),
                None,
                &sig.signature,
                &protocol,
                key_id,
                &self_cp(),
                true,
            )
            .unwrap_or(false);
        assert!(
            !local_verifies,
            "the signature must not verify under the local root key — only the provider signed"
        );

        // 5 + 6. encrypt / decrypt round-trip; the ciphertext must open under
        // the provider's derived symmetric key and NOT under the local root's.
        let plaintext = b"delegated encryption plaintext".to_vec();
        let enc = wallet
            .encrypt(
                EncryptArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    plaintext: plaintext.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated encrypt");
        let dec = wallet
            .decrypt(
                DecryptArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    ciphertext: enc.ciphertext.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated decrypt");
        assert_eq!(dec.plaintext, plaintext);
        let provider_sym = provider
            .deriver()
            .derive_symmetric_key(&protocol, key_id, &self_cp())
            .expect("provider-side symmetric key");
        assert_eq!(
            provider_sym
                .decrypt(&enc.ciphertext)
                .expect("provider key must open the ciphertext"),
            plaintext
        );
        let local_sym = local_deriver
            .derive_symmetric_key(&protocol, key_id, &self_cp())
            .expect("local symmetric key");
        assert!(
            local_sym.decrypt(&enc.ciphertext).is_err(),
            "the local root's symmetric key must NOT open the ciphertext"
        );

        // 7 + 8. createHmac / verifyHmac: keyed with the provider's derived
        // key (minimal big-endian), and the round-trip verifies.
        let hmac_data = b"delegated hmac data".to_vec();
        let hmac = wallet
            .create_hmac(
                CreateHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: hmac_data.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated createHmac");
        assert_eq!(
            hmac.hmac,
            sha256_hmac(&provider_sym.to_hmac_key_bytes(), &hmac_data).to_vec(),
            "HMAC must be keyed by the provider's derived key"
        );
        let hmac_ok = wallet
            .verify_hmac(
                VerifyHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: hmac_data.clone(),
                    hmac: hmac.hmac.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("delegated verifyHmac");
        assert!(hmac_ok.valid);

        // 9. Key-linkage reveals are unrepresentable through the provider's
        // three key operations, so they must fail closed, not silently answer
        // from the local root.
        let other = common::random_root_key().to_public_key();
        let verifier = common::random_root_key().to_public_key();
        let reveal_cp = wallet
            .reveal_counterparty_key_linkage(
                RevealCounterpartyKeyLinkageArgs {
                    counterparty: other.clone(),
                    verifier: verifier.clone(),
                    privileged: None,
                    privileged_reason: None,
                },
                None,
            )
            .await;
        assert!(
            matches!(reveal_cp, Err(SdkWalletError::NotImplemented(_))),
            "revealCounterpartyKeyLinkage must be NotImplemented under delegation, got {reveal_cp:?}"
        );
        let reveal_sp = wallet
            .reveal_specific_key_linkage(
                RevealSpecificKeyLinkageArgs {
                    counterparty: Counterparty {
                        counterparty_type: CounterpartyType::Other,
                        public_key: Some(other),
                    },
                    verifier,
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    privileged: None,
                    privileged_reason: None,
                },
                None,
            )
            .await;
        assert!(
            matches!(reveal_sp, Err(SdkWalletError::NotImplemented(_))),
            "revealSpecificKeyLinkage must be NotImplemented under delegation, got {reveal_sp:?}"
        );

        // The provider's three key operations were consulted for every answer.
        assert!(
            provider.derive_public_key_calls.load(Ordering::SeqCst) >= 2,
            "getPublicKey and verifySignature must consult provider.derive_public_key"
        );
        assert_eq!(
            provider.derive_symmetric_key_calls.load(Ordering::SeqCst),
            4,
            "encrypt, decrypt, createHmac, verifyHmac must each consult provider.derive_symmetric_key"
        );
        assert_eq!(
            provider.create_signature_calls.load(Ordering::SeqCst),
            1,
            "createSignature must consult provider.create_signature"
        );
    }

    /// With no provider, every crypto method must behave exactly as a
    /// ProtoWallet over the wallet's root key — the pre-existing local path.
    #[tokio::test]
    async fn without_provider_crypto_matches_local_proto() {
        let wallet_root = common::random_root_key();
        let setup = crypto_wallet(wallet_root.clone(), None).await;
        let wallet = &setup.wallet;

        let protocol = crypto_protocol();
        let key_id = "local-crypto-1";
        let reference = ProtoWallet::new(wallet_root.clone());

        // Identity + derived keys match the reference ProtoWallet.
        let identity = wallet
            .get_public_key(get_public_key_args(true, ""), None)
            .await
            .expect("local identity getPublicKey");
        assert_eq!(
            identity.public_key.to_der_hex(),
            wallet_root.to_public_key().to_der_hex()
        );
        let derived = wallet
            .get_public_key(get_public_key_args(false, key_id), None)
            .await
            .expect("local derived getPublicKey");
        let expected = reference
            .get_public_key_sync(&protocol, key_id, &self_cp(), true, false)
            .expect("reference derivation");
        assert_eq!(derived.public_key.to_der_hex(), expected.to_der_hex());

        // Signatures come from the root key's derivation.
        let data = b"local signature data".to_vec();
        let sig = wallet
            .create_signature(
                CreateSignatureArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: Some(data.clone()),
                    hash_to_directly_sign: None,
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("local createSignature");
        assert!(reference
            .verify_signature_sync(
                Some(&data),
                None,
                &sig.signature,
                &protocol,
                key_id,
                &self_cp(),
                true,
            )
            .expect("reference verify"));

        // Ciphertext opens under the reference ProtoWallet, HMAC matches it.
        let plaintext = b"local encryption plaintext".to_vec();
        let enc = wallet
            .encrypt(
                EncryptArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    plaintext: plaintext.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("local encrypt");
        assert_eq!(
            reference
                .decrypt_sync(&enc.ciphertext, &protocol, key_id, &self_cp())
                .expect("reference decrypt"),
            plaintext
        );
        let hmac_data = b"local hmac data".to_vec();
        let hmac = wallet
            .create_hmac(
                CreateHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: hmac_data.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("local createHmac");
        assert_eq!(
            hmac.hmac,
            reference
                .create_hmac_sync(&hmac_data, &protocol, key_id, &self_cp())
                .expect("reference hmac")
        );

        // Key-linkage reveals still work locally.
        let other = common::random_root_key().to_public_key();
        let verifier = common::random_root_key().to_public_key();
        wallet
            .reveal_counterparty_key_linkage(
                RevealCounterpartyKeyLinkageArgs {
                    counterparty: other.clone(),
                    verifier: verifier.clone(),
                    privileged: None,
                    privileged_reason: None,
                },
                None,
            )
            .await
            .expect("local revealCounterpartyKeyLinkage must still work");
        wallet
            .reveal_specific_key_linkage(
                RevealSpecificKeyLinkageArgs {
                    counterparty: Counterparty {
                        counterparty_type: CounterpartyType::Other,
                        public_key: Some(other),
                    },
                    verifier,
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    privileged: None,
                    privileged_reason: None,
                },
                None,
            )
            .await
            .expect("local revealSpecificKeyLinkage must still work");
    }

    // -----------------------------------------------------------------------
    // 4. HMAC keying: minimal big-endian, proven with a leading-zero key
    // -----------------------------------------------------------------------

    /// Provider that returns a FIXED symmetric key, for pinning exactly how
    /// the wallet keys its local AES and HMAC composition.
    struct FixedKeyProvider {
        key: [u8; 32],
        identity: PublicKey,
    }

    #[async_trait]
    impl SigningProvider for FixedKeyProvider {
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
            _ctx: &SigningContext,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal("not used".to_string()))
        }

        async fn derive_public_key(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
            _for_self: bool,
        ) -> WalletResult<PublicKey> {
            Err(WalletError::Internal("not used".to_string()))
        }

        async fn derive_symmetric_key(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
        ) -> WalletResult<[u8; 32]> {
            Ok(self.key)
        }

        async fn create_signature(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
            _digest: &[u8; 32],
            _ctx: &SigningContext,
        ) -> WalletResult<Vec<u8>> {
            Err(WalletError::Internal("not used".to_string()))
        }

        fn identity_public_key(&self) -> &PublicKey {
            &self.identity
        }
    }

    /// 🔴 The HMAC must be keyed with the MINIMAL big-endian encoding of the
    /// symmetric key: a key with a leading zero byte keys as 31 bytes, matching
    /// TS `key.toArray()`. Keying with the fixed 32 bytes diverges silently for
    /// ~1 derived key in 256. AES is the opposite — it always uses all 32.
    #[tokio::test]
    async fn hmac_keys_with_minimal_encoding_leading_zero_key() {
        let wallet_root = common::random_root_key();
        let mut key = [0x5au8; 32];
        key[0] = 0x00; // the 1-in-256 case: minimal encoding is 31 bytes
        let provider = Arc::new(FixedKeyProvider {
            key,
            identity: wallet_root.to_public_key(),
        });
        let setup = crypto_wallet(wallet_root, Some(provider)).await;
        let wallet = &setup.wallet;

        let protocol = crypto_protocol();
        let key_id = "leading-zero-1";
        let data = b"leading zero hmac interop".to_vec();

        let hmac = wallet
            .create_hmac(
                CreateHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: data.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("createHmac with leading-zero key");

        let minimal_keyed = sha256_hmac(&key[1..], &data).to_vec();
        let full_keyed = sha256_hmac(&key, &data).to_vec();
        assert_ne!(
            minimal_keyed, full_keyed,
            "fixture must actually distinguish the two keyings"
        );
        assert_eq!(
            hmac.hmac, minimal_keyed,
            "HMAC must key with the 31-byte minimal encoding, as TS does"
        );

        // Round-trip verifies; the 32-byte-keyed digest is rejected.
        let ok = wallet
            .verify_hmac(
                VerifyHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: data.clone(),
                    hmac: hmac.hmac.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("verifyHmac round-trip");
        assert!(ok.valid);
        let bad = wallet
            .verify_hmac(
                VerifyHmacArgs {
                    protocol_id: protocol.clone(),
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    data: data.clone(),
                    hmac: full_keyed,
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await;
        assert!(
            matches!(bad, Err(SdkWalletError::InvalidHmac)),
            "a 32-byte-keyed digest must NOT verify, got {bad:?}"
        );

        // AES keying is unaffected by the leading zero: ciphertext opens under
        // the full 32-byte key.
        let plaintext = b"aes uses all 32 bytes".to_vec();
        let enc = wallet
            .encrypt(
                EncryptArgs {
                    protocol_id: protocol,
                    key_id: key_id.to_string(),
                    counterparty: self_cp(),
                    plaintext: plaintext.clone(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await
            .expect("encrypt with leading-zero key");
        let sym = SymmetricKey::from_bytes(&key).expect("32-byte key");
        assert_eq!(
            sym.decrypt(&enc.ciphertext).expect("full-32-byte AES key"),
            plaintext
        );
    }

    // -----------------------------------------------------------------------
    // 5. 🔴 ProtoWallet wraps the DERIVER, not the deriver's root key
    // -----------------------------------------------------------------------

    /// A deriver shaped like a threshold vault: its identity is a real public
    /// key Q, its root key is a throwaway placeholder nobody controls, and it
    /// cannot derive locally.
    struct ThresholdDeriver {
        identity: PublicKey,
        throwaway: PrivateKey,
    }

    impl ThresholdDeriver {
        fn err<T>() -> Result<T, SdkWalletError> {
            Err(SdkWalletError::Internal(
                "threshold deriver cannot derive locally".to_string(),
            ))
        }
    }

    impl KeyDeriverApi for ThresholdDeriver {
        fn root_key(&self) -> &PrivateKey {
            &self.throwaway
        }

        fn identity_key(&self) -> PublicKey {
            self.identity.clone()
        }

        fn derive_public_key(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
            _for_self: bool,
        ) -> Result<PublicKey, SdkWalletError> {
            Self::err()
        }

        fn derive_private_key(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
        ) -> Result<PrivateKey, SdkWalletError> {
            Self::err()
        }

        fn derive_symmetric_key(
            &self,
            _protocol: &Protocol,
            _key_id: &str,
            _counterparty: &Counterparty,
        ) -> Result<SymmetricKey, SdkWalletError> {
            Self::err()
        }

        fn reveal_counterparty_secret(
            &self,
            _counterparty: &Counterparty,
        ) -> Result<PublicKey, SdkWalletError> {
            Self::err()
        }

        fn reveal_specific_secret(
            &self,
            _counterparty: &Counterparty,
            _protocol: &Protocol,
            _key_id: &str,
        ) -> Result<Vec<u8>, SdkWalletError> {
            Self::err()
        }
    }

    /// 🔴 Security regression guard: `Wallet` used to build its ProtoWallet
    /// from `key_deriver.root_key()`, reaching PAST the deriver. For a deriver
    /// whose identity Q is decoupled from a throwaway root (a threshold vault),
    /// that silently answered every crypto call with throwaway-derived keys —
    /// certificates got bound to keys nobody controls, with no error. The
    /// ProtoWallet must wrap the deriver itself: identity queries answer Q,
    /// real derivations surface the deriver's error.
    #[tokio::test]
    async fn proto_wallet_wraps_the_deriver_not_its_root_key() {
        let q_source = common::random_root_key();
        let throwaway = common::random_root_key();
        let deriver: Arc<dyn KeyDeriverApi> = Arc::new(ThresholdDeriver {
            identity: q_source.to_public_key(),
            throwaway: throwaway.clone(),
        });

        let setup = WalletBuilder::new()
            .chain(Chain::Test)
            .key_deriver(deriver)
            .with_sqlite_memory()
            .with_services(Arc::new(MockWalletServices))
            .without_monitor()
            .build()
            .await
            .expect("build wallet on a threshold-shaped deriver");
        let wallet = &setup.wallet;

        // Identity is Q — never the throwaway's public key.
        let identity = wallet
            .get_public_key(get_public_key_args(true, ""), None)
            .await
            .expect("identity getPublicKey");
        assert_eq!(
            identity.public_key.to_der_hex(),
            q_source.to_public_key().to_der_hex()
        );
        assert_ne!(
            identity.public_key.to_der_hex(),
            throwaway.to_public_key().to_der_hex(),
            "identity must never come from the throwaway root"
        );

        // A real derivation ERRORS — it must not silently answer with a
        // throwaway-derived key.
        let derived = wallet
            .get_public_key(get_public_key_args(false, "threshold-1"), None)
            .await;
        assert!(
            derived.is_err(),
            "derivation on a non-deriving deriver must error, not silently answer: {derived:?}"
        );

        // The same holds for an operation needing a symmetric key.
        let enc = wallet
            .encrypt(
                EncryptArgs {
                    protocol_id: crypto_protocol(),
                    key_id: "threshold-1".to_string(),
                    counterparty: self_cp(),
                    plaintext: b"must not encrypt".to_vec(),
                    privileged: false,
                    privileged_reason: None,
                    seek_permission: None,
                },
                None,
            )
            .await;
        assert!(
            enc.is_err(),
            "encrypt on a non-deriving deriver must error, not use a throwaway key: {enc:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Script attribution helpers
    // -----------------------------------------------------------------------

    fn payment_script() -> Vec<u8> {
        vec![
            0x76, 0xa9, 0x14, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb,
            0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0xbb, 0x88, 0xac,
        ]
    }

    /// The BRC-29 self-payment locking script `root_key` produces for a
    /// derivation pair — the exact script the local (non-delegated) path builds.
    fn brc29_lock(
        root_key: &PrivateKey,
        identity_pub_key: &PublicKey,
        prefix: &str,
        suffix: &str,
    ) -> Vec<u8> {
        ScriptTemplateBRC29::new(prefix.to_string(), suffix.to_string())
            .lock(root_key, identity_pub_key)
            .expect("BRC-29 lock")
    }

    /// Rebuild the pending sign action from the signable transaction the
    /// deferred `createAction` handed back, plus the known shape of the seeded
    /// UTXOs — the same data a caller holds between the two calls.
    ///
    /// `dcr.input_beef` is left empty on purpose: with no source transaction in
    /// the BEEF, unlock-script verification is skipped, which keeps this test
    /// focused on which backend produced the signature.
    fn pending_from_signable(
        reference: &str,
        signable_beef: &[u8],
        lock_key: &PrivateKey,
    ) -> bsv_wallet_toolbox::signer::types::PendingSignAction {
        use bsv_wallet_toolbox::signer::types::{PendingSignAction, PendingStorageInput};
        use bsv_wallet_toolbox::storage::action_types::StorageCreateActionResult;

        let unsigned = subject_tx(signable_beef);
        let mut raw_tx = Vec::new();
        unsigned
            .to_binary(&mut raw_tx)
            .expect("serialize unsigned tx");

        // Every seeded UTXO is locked identically, so each allocated input needs
        // the same derivation data — only the vin differs.
        let seed_lock = brc29_lock(
            lock_key,
            &lock_key.to_public_key(),
            SEED_PREFIX,
            SEED_SUFFIX,
        );
        let locking_script_hex: String = seed_lock.iter().map(|b| format!("{b:02x}")).collect();
        let pdi = (0..unsigned.inputs.len())
            .map(|vin| PendingStorageInput {
                vin: vin as u32,
                derivation_prefix: SEED_PREFIX.to_string(),
                derivation_suffix: SEED_SUFFIX.to_string(),
                unlocker_pub_key: None,
                source_satoshis: 50_000,
                locking_script: locking_script_hex.clone(),
            })
            .collect();

        PendingSignAction {
            reference: reference.to_string(),
            dcr: StorageCreateActionResult {
                reference: reference.to_string(),
                version: unsigned.version,
                lock_time: unsigned.lock_time,
                inputs: vec![],
                outputs: vec![],
                derivation_prefix: String::new(),
                input_beef: None,
                no_send_change_output_vouts: None,
            },
            args: deferred_create_args(),
            tx: raw_tx,
            amount: 0,
            pdi,
        }
    }

    /// `ValidCreateActionArgs` matching the deferred action `payment_args(false)`
    /// produces, for the fields `signer_sign_action` reads back.
    fn deferred_create_args() -> bsv_wallet_toolbox::signer::types::ValidCreateActionArgs {
        bsv_wallet_toolbox::signer::types::ValidCreateActionArgs {
            description: "provider injection test".to_string(),
            inputs: vec![],
            outputs: vec![],
            lock_time: 0,
            version: 1,
            labels: vec![],
            options: CreateActionOptions::default(),
            input_beef: None,
            is_new_tx: true,
            is_sign_action: true,
            is_no_send: false,
            is_delayed: true,
            is_send_with: false,
        }
    }

    fn valid_sign_action_args(
        reference: &str,
    ) -> bsv_wallet_toolbox::signer::types::ValidSignActionArgs {
        bsv_wallet_toolbox::signer::types::ValidSignActionArgs {
            reference: reference.to_string(),
            spends: Default::default(),
            options: Default::default(),
            is_new_tx: true,
            is_no_send: None,
            is_delayed: Some(true),
            is_send_with: None,
        }
    }

    /// The 20-byte pubkey hash the seeded UTXOs are locked to for `root_key` —
    /// i.e. bytes 3..23 of its BRC-29 P2PKH locking script.
    fn seed_pubkey_hash(root_key: &PrivateKey) -> Vec<u8> {
        let lock = brc29_lock(
            root_key,
            &root_key.to_public_key(),
            SEED_PREFIX,
            SEED_SUFFIX,
        );
        lock[3..23].to_vec()
    }

    /// hash160 of the public key pushed by a P2PKH unlocking script, whose
    /// layout is `<sigLen><DER+hashtype><33><pubkey33>`.
    fn unlocking_script_pubkey_hash(unlock: &[u8]) -> Vec<u8> {
        assert!(!unlock.is_empty(), "unlocking script must not be empty");
        let sig_len = unlock[0] as usize;
        let pk_push_idx = 1 + sig_len;
        let pk_len = unlock[pk_push_idx] as usize;
        let pubkey = &unlock[pk_push_idx + 1..pk_push_idx + 1 + pk_len];
        PublicKey::from_der_bytes(pubkey)
            .expect("valid pubkey in unlocking script")
            .to_hash()
    }

    /// The (derivation_prefix, derivation_suffix) storage recorded for the
    /// change outputs of `txid`, so a test can recompute the exact locking
    /// script the local path would have produced.
    async fn change_derivations(
        storage: &WalletStorageManager,
        txid: &str,
    ) -> Vec<(String, String)> {
        use bsv_wallet_toolbox::storage::find_args::{FindOutputsArgs, OutputPartial};
        storage
            .find_outputs_storage(&FindOutputsArgs {
                partial: OutputPartial {
                    txid: Some(txid.to_string()),
                    change: Some(true),
                    ..Default::default()
                },
                no_script: false,
                ..Default::default()
            })
            .await
            .expect("find change outputs")
            .into_iter()
            .filter_map(|o| Some((o.derivation_prefix?, o.derivation_suffix?)))
            .collect()
    }
}
