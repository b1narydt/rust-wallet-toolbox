//! Test utilities for integration tests.
//!
//! Provides helper functions for creating test wallets, seeding storage data,
//! and mock services -- enabling rapid test setup with no network calls.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use bsv::primitives::private_key::PrivateKey;
use bsv::transaction::Beef;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::TransactionError;

use bsv_wallet_toolbox::error::{WalletError, WalletResult};
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::manager::WalletStorageManager;
use bsv_wallet_toolbox::tables::{Output, OutputBasket, Transaction};
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
use bsv_wallet_toolbox::wallet::setup::{SetupWallet, WalletBuilder};

// ---------------------------------------------------------------------------
// Random root key generation
// ---------------------------------------------------------------------------

/// Generate a random PrivateKey for testing.
#[allow(dead_code)]
pub fn random_root_key() -> PrivateKey {
    PrivateKey::from_random().expect("random key generation")
}

// ---------------------------------------------------------------------------
// Test wallet creation
// ---------------------------------------------------------------------------

/// Create a test wallet with mock services, SQLite in-memory storage, and Chain::Test.
///
/// Uses WalletBuilder internally. The returned SetupWallet has all components
/// accessible for inspection and seeding.
#[allow(dead_code)]
pub async fn create_test_wallet() -> SetupWallet {
    let root_key = random_root_key();
    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key)
        .with_sqlite_memory()
        .with_services(Arc::new(MockWalletServices))
        .build()
        .await
        .expect("Failed to create test wallet");
    // Ensure user record exists in storage so queries don't fail with "User not found"
    setup
        .storage
        .active()
        .find_or_insert_user(&setup.identity_key, None)
        .await
        .expect("ensure user exists");
    setup
}

/// Create a test wallet with a specific root key and mock services.
#[allow(dead_code)]
pub async fn create_test_wallet_with_key(root_key: PrivateKey) -> SetupWallet {
    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key)
        .with_sqlite_memory()
        .with_services(Arc::new(MockWalletServices))
        .build()
        .await
        .expect("Failed to create test wallet with key");
    setup
        .storage
        .active()
        .find_or_insert_user(&setup.identity_key, None)
        .await
        .expect("ensure user exists");
    setup
}

// ---------------------------------------------------------------------------
// Mock services
// ---------------------------------------------------------------------------

/// Mock ChainTracker that always returns valid.
struct MockChainTracker;

#[async_trait]
impl ChainTracker for MockChainTracker {
    async fn is_valid_root_for_height(
        &self,
        _root: &str,
        _height: u32,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}

/// Mock WalletServices implementation for testing.
///
/// All methods return Ok with reasonable defaults (empty vecs, zero height, etc.).
/// No real network calls are made.
pub struct MockWalletServices;

#[async_trait]
impl WalletServices for MockWalletServices {
    fn chain(&self) -> Chain {
        Chain::Test
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        Ok(Box::new(MockChainTracker))
    }

    async fn get_merkle_path(
        &self,
        _txid: &str,
        _use_next: bool,
    ) -> types::GetMerklePathResult {
        types::GetMerklePathResult {
            name: Some("mock".to_string()),
            merkle_path: None,
            header: None,
            error: None,
        }
    }

    async fn get_raw_tx(&self, txid: &str, _use_next: bool) -> types::GetRawTxResult {
        types::GetRawTxResult {
            txid: txid.to_string(),
            name: Some("mock".to_string()),
            raw_tx: None,
            error: None,
        }
    }

    async fn post_beef(
        &self,
        _beef: &[u8],
        _txids: &[String],
    ) -> Vec<types::PostBeefResult> {
        vec![]
    }

    async fn get_utxo_status(
        &self,
        _output: &str,
        _output_format: Option<types::GetUtxoStatusOutputFormat>,
        _outpoint: Option<&str>,
        _use_next: bool,
    ) -> types::GetUtxoStatusResult {
        types::GetUtxoStatusResult {
            name: "mock".to_string(),
            status: "success".to_string(),
            error: None,
            is_utxo: Some(false),
            details: vec![],
        }
    }

    async fn get_status_for_txids(
        &self,
        _txids: &[String],
        _use_next: bool,
    ) -> types::GetStatusForTxidsResult {
        types::GetStatusForTxidsResult {
            name: "mock".to_string(),
            status: "success".to_string(),
            error: None,
            results: vec![],
        }
    }

    async fn get_script_hash_history(
        &self,
        _hash: &str,
        _use_next: bool,
    ) -> types::GetScriptHashHistoryResult {
        types::GetScriptHashHistoryResult {
            name: "mock".to_string(),
            status: "success".to_string(),
            error: None,
            history: vec![],
        }
    }

    async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
        Err(WalletError::NotImplemented("mock".to_string()))
    }

    async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
        Ok(vec![0u8; 80])
    }

    async fn get_height(&self) -> WalletResult<u32> {
        Ok(100_000)
    }

    async fn n_lock_time_is_final(
        &self,
        _input: types::NLockTimeInput,
    ) -> WalletResult<bool> {
        Ok(true)
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
        Ok(types::BsvExchangeRate::default())
    }

    async fn get_fiat_exchange_rate(
        &self,
        _currency: &str,
        _base: Option<&str>,
    ) -> WalletResult<f64> {
        Ok(1.0)
    }

    async fn get_fiat_exchange_rates(
        &self,
        _target_currencies: &[String],
    ) -> WalletResult<types::FiatExchangeRates> {
        Ok(types::FiatExchangeRates::default())
    }

    fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
        types::ServicesCallHistory {
            services: vec![],
        }
    }

    async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<Beef> {
        Err(WalletError::NotImplemented("mock".to_string()))
    }

    fn hash_output_script(&self, _script: &[u8]) -> String {
        String::new()
    }

    async fn is_utxo(
        &self,
        _locking_script: &[u8],
        _txid: &str,
        _vout: u32,
    ) -> WalletResult<bool> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Seeding helpers
// ---------------------------------------------------------------------------

/// Seed `count` spendable outputs into the storage for the given user identity key.
///
/// Creates a "default" basket (if needed), a completed transaction, and `count`
/// output records each with the specified satoshis. Returns the created output IDs.
#[allow(dead_code)]
pub async fn seed_outputs(
    storage: &WalletStorageManager,
    user_identity_key: &str,
    count: usize,
    satoshis: u64,
) -> Vec<i64> {
    let provider = storage.active();
    let now = Utc::now().naive_utc();

    // Ensure user exists
    let (user, _) = provider
        .find_or_insert_user(user_identity_key, None)
        .await
        .expect("find_or_insert_user");
    let user_id = user.user_id;

    // Create or find "default" basket
    let basket_id = {
        use bsv_wallet_toolbox::storage::find_args::{FindOutputBasketsArgs, OutputBasketPartial};
        let existing = provider
            .find_output_baskets(
                &FindOutputBasketsArgs {
                    partial: OutputBasketPartial {
                        user_id: Some(user_id),
                        name: Some("default".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("find baskets");
        if let Some(b) = existing.first() {
            b.basket_id
        } else {
            provider
                .insert_output_basket(
                    &OutputBasket {
                        created_at: now,
                        updated_at: now,
                        basket_id: 0,
                        user_id,
                        name: "default".to_string(),
                        number_of_desired_utxos: 10,
                        minimum_desired_utxo_value: 1000,
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .expect("insert basket")
        }
    };

    // Create a parent transaction (completed status so outputs are visible)
    let txid_hex = format!("{:064x}", rand::random::<u64>());
    let tx_id = provider
        .insert_transaction(
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: format!("seed-{}", rand::random::<u32>()),
                is_outgoing: false,
                satoshis: (satoshis * count as u64) as i64,
                description: "seed transaction".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(txid_hex.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert transaction");

    // Create outputs
    let mut output_ids = Vec::with_capacity(count);
    for i in 0..count {
        let oid = provider
            .insert_output(
                &Output {
                    created_at: now,
                    updated_at: now,
                    output_id: 0,
                    user_id,
                    transaction_id: tx_id,
                    basket_id: Some(basket_id),
                    spendable: true,
                    change: false,
                    output_description: Some(format!("seed output {}", i)),
                    vout: i as i32,
                    satoshis: satoshis as i64,
                    provided_by: StorageProvidedBy::You,
                    purpose: "change".to_string(),
                    output_type: "P2PKH".to_string(),
                    txid: Some(txid_hex.clone()),
                    sender_identity_key: None,
                    derivation_prefix: None,
                    derivation_suffix: None,
                    custom_instructions: None,
                    spent_by: None,
                    sequence_number: None,
                    spending_description: None,
                    script_length: None,
                    script_offset: None,
                    locking_script: None,
                },
                None,
            )
            .await
            .expect("insert output");
        output_ids.push(oid);
    }

    output_ids
}

/// Seed a single transaction with the given status. Returns the transaction ID.
#[allow(dead_code)]
pub async fn seed_transaction(
    storage: &WalletStorageManager,
    user_identity_key: &str,
    status: TransactionStatus,
) -> i64 {
    let provider = storage.active();
    let now = Utc::now().naive_utc();

    let (user, _) = provider
        .find_or_insert_user(user_identity_key, None)
        .await
        .expect("find_or_insert_user");

    let txid_hex = format!("{:064x}", rand::random::<u64>());
    provider
        .insert_transaction(
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                status,
                reference: format!("seed-tx-{}", rand::random::<u32>()),
                is_outgoing: false,
                satoshis: 0,
                description: "seed transaction".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(txid_hex),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .expect("insert transaction")
}
