//! Proof of the customer-held recovery path, and of backup wiring.
//!
//! PARAGON's enterprise offering commits to a specific sentence: *Binary can
//! freeze. Binary cannot steal. The customer can always recover without us.*
//! The third clause is the one that needs evidence. Binary can withhold a
//! customer's derivation store, and that metadata is not recoverable from
//! anywhere else -- BRC-42 output derivation is not enumerable, so the key
//! shares alone recover nothing. The restore path is therefore load-bearing,
//! and an untested restore path is an unsupported claim.
//!
//! `recovery_from_container_alone_rederives_keys_and_spends` is that evidence:
//! starting from a BRC-39 container and a password, with a services
//! implementation that panics on contact, it restores a wallet and validates a
//! real spend of a restored output through the script interpreter.

#![cfg(feature = "sqlite")]

use std::sync::Arc;

use async_trait::async_trait;
use bsv::primitives::hash::sha256d;
use bsv::primitives::private_key::PrivateKey;
use bsv::script::spend::{Spend, SpendParams};
use bsv::script::{LockingScript, UnlockingScript};
use bsv::transaction::{Transaction as BsvTransaction, TransactionInput, TransactionOutput};

use bsv::transaction::chain_tracker::ChainTracker;
use bsv_wallet_toolbox::error::WalletResult;
use bsv_wallet_toolbox::services::traits::WalletServices;
use bsv_wallet_toolbox::services::types;
use bsv_wallet_toolbox::status::TransactionStatus;
use bsv_wallet_toolbox::storage::portable::{
    decrypt_brc39, encrypt_brc39, export_brc38, import_brc38, Brc38ImportOptions, ImportMode,
};
use bsv_wallet_toolbox::storage::sqlx_impl::SqliteStorage;
use bsv_wallet_toolbox::storage::traits::provider::StorageProvider;
use bsv_wallet_toolbox::storage::traits::reader::StorageReader;
use bsv_wallet_toolbox::storage::traits::reader_writer::StorageReaderWriter;
use bsv_wallet_toolbox::storage::{find_args::*, StorageConfig};
use bsv_wallet_toolbox::tables::{Output, Transaction};
use bsv_wallet_toolbox::types::{Chain, StorageProvidedBy};
use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
use bsv_wallet_toolbox::wallet::setup::WalletBuilder;

const PASSWORD: &str = "recovery proof password";

/// A fresh temp directory, named per test so parallel runs cannot collide.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bsv-recovery-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// A `WalletServices` that fails the test on contact.
///
/// Recovery has to work when Binary is gone -- not slow, not refusing, gone.
/// A test that merely happens to avoid the network proves nothing, because the
/// next refactor can add a call and stay green. This makes any network reach
/// from the recovery path an immediate, named panic.
///
/// The three methods that answer locally (`chain`, `hash_output_script`,
/// `get_services_call_history`) are honoured; every method that would leave the
/// process panics.
struct NetworkTripwire;

impl NetworkTripwire {
    fn tripped(method: &str) -> ! {
        panic!(
            "network isolation violated: recovery path called WalletServices::{method}. \
             Restoring a wallet from a customer-held backup must not require Binary or \
             any other network service."
        );
    }
}

#[async_trait]
impl WalletServices for NetworkTripwire {
    fn chain(&self) -> Chain {
        Chain::Test
    }

    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>> {
        Self::tripped("get_chain_tracker")
    }

    async fn get_merkle_path(&self, _txid: &str, _use_next: bool) -> types::GetMerklePathResult {
        Self::tripped("get_merkle_path")
    }

    async fn get_raw_tx(&self, _txid: &str, _use_next: bool) -> types::GetRawTxResult {
        Self::tripped("get_raw_tx")
    }

    async fn post_beef(&self, _beef: &[u8], _txids: &[String]) -> Vec<types::PostBeefResult> {
        Self::tripped("post_beef")
    }

    async fn get_utxo_status(
        &self,
        _output: &str,
        _output_format: Option<types::GetUtxoStatusOutputFormat>,
        _outpoint: Option<&str>,
        _use_next: bool,
    ) -> types::GetUtxoStatusResult {
        Self::tripped("get_utxo_status")
    }

    async fn get_status_for_txids(
        &self,
        _txids: &[String],
        _use_next: bool,
    ) -> types::GetStatusForTxidsResult {
        Self::tripped("get_status_for_txids")
    }

    async fn get_script_hash_history(
        &self,
        _hash: &str,
        _use_next: bool,
    ) -> types::GetScriptHashHistoryResult {
        Self::tripped("get_script_hash_history")
    }

    async fn hash_to_header(&self, _hash: &str) -> WalletResult<types::BlockHeader> {
        Self::tripped("hash_to_header")
    }

    async fn get_header_for_height(&self, _height: u32) -> WalletResult<Vec<u8>> {
        Self::tripped("get_header_for_height")
    }

    async fn get_height(&self) -> WalletResult<u32> {
        Self::tripped("get_height")
    }

    async fn n_lock_time_is_final(&self, _input: types::NLockTimeInput) -> WalletResult<bool> {
        Self::tripped("n_lock_time_is_final")
    }

    async fn get_bsv_exchange_rate(&self) -> WalletResult<types::BsvExchangeRate> {
        Self::tripped("get_bsv_exchange_rate")
    }

    async fn get_fiat_exchange_rate(
        &self,
        _currency: &str,
        _base: Option<&str>,
    ) -> WalletResult<f64> {
        Self::tripped("get_fiat_exchange_rate")
    }

    async fn get_fiat_exchange_rates(
        &self,
        _target_currencies: &[String],
    ) -> WalletResult<types::FiatExchangeRates> {
        Self::tripped("get_fiat_exchange_rates")
    }

    fn get_services_call_history(&self, _reset: bool) -> types::ServicesCallHistory {
        types::ServicesCallHistory { services: vec![] }
    }

    async fn get_beef_for_txid(&self, _txid: &str) -> WalletResult<bsv::transaction::beef::Beef> {
        Self::tripped("get_beef_for_txid")
    }

    fn hash_output_script(&self, script: &[u8]) -> String {
        let mut h = sha256d(script);
        h.reverse();
        hex::encode(h)
    }

    async fn is_utxo(&self, _locking_script: &[u8], _txid: &str, _vout: u32) -> WalletResult<bool> {
        Self::tripped("is_utxo")
    }
}

/// A received BRC-29 payment, described the way the wallet must be able to
/// reconstruct it: the derivation coordinates plus who sent it.
struct SeededPayment {
    txid: String,
    vout: i32,
    satoshis: i64,
    derivation_prefix: String,
    derivation_suffix: String,
    sender_identity_key: String,
    locking_script: Vec<u8>,
}

async fn open_sqlite(path: &str) -> SqliteStorage {
    let config = StorageConfig {
        url: format!("sqlite:{path}"),
        ..Default::default()
    };
    let storage = SqliteStorage::new_sqlite(config, Chain::Test)
        .await
        .unwrap();
    storage.migrate_database().await.unwrap();
    storage.make_available().await.unwrap();
    storage
}

/// Seed one BRC-29 wallet payment from `sender` to the wallet identified by
/// `receiver_identity`, with a locking script derived exactly as the sender
/// would derive it.
async fn seed_payment(
    storage: &SqliteStorage,
    receiver_identity: &str,
    receiver_root: &PrivateKey,
    sender: &PrivateKey,
) -> SeededPayment {
    let (user, _) = storage
        .find_or_insert_user(receiver_identity, None)
        .await
        .unwrap();
    let basket = storage
        .find_or_insert_output_basket(user.user_id, "default", None)
        .await
        .unwrap();

    let derivation_prefix = "cmVjb3ZlcnlwcmVmaXg=".to_string();
    let derivation_suffix = "cmVjb3ZlcnlzdWZmaXg=".to_string();

    let template = ScriptTemplateBRC29::new(derivation_prefix.clone(), derivation_suffix.clone());
    // The sender locks to a key derived from the receiver's identity key.
    let receiver_pub = receiver_root.to_public_key();
    let locking_script = template.lock(sender, &receiver_pub).unwrap();
    let sender_identity_key = sender.to_public_key().to_der_hex();

    let txid = "1111222233334444555566667777888899990000aaaabbbbccccddddeeeeffff".to_string();
    let satoshis = 12_345i64;
    let now = chrono::Utc::now().naive_utc();

    let tx_id = storage
        .insert_transaction(
            &Transaction {
                created_at: now,
                updated_at: now,
                transaction_id: 0,
                user_id: user.user_id,
                proven_tx_id: None,
                status: TransactionStatus::Completed,
                reference: "recovery-seed".to_string(),
                is_outgoing: false,
                satoshis,
                description: "received payment".to_string(),
                version: Some(1),
                lock_time: Some(0),
                txid: Some(txid.clone()),
                input_beef: None,
                raw_tx: None,
            },
            None,
        )
        .await
        .unwrap();

    storage
        .insert_output(
            &Output {
                created_at: now,
                updated_at: now,
                output_id: 0,
                user_id: user.user_id,
                transaction_id: tx_id,
                basket_id: Some(basket.basket_id),
                spendable: true,
                change: false,
                output_description: Some("received payment".to_string()),
                vout: 0,
                satoshis,
                provided_by: StorageProvidedBy::You,
                purpose: "".to_string(),
                output_type: "P2PKH".to_string(),
                txid: Some(txid.clone()),
                sender_identity_key: Some(sender_identity_key.clone()),
                derivation_prefix: Some(derivation_prefix.clone()),
                derivation_suffix: Some(derivation_suffix.clone()),
                custom_instructions: None,
                spent_by: None,
                sequence_number: None,
                spending_description: None,
                script_length: Some(locking_script.len() as i64),
                script_offset: Some(0),
                locking_script: Some(locking_script.clone()),
            },
            None,
        )
        .await
        .unwrap();

    SeededPayment {
        txid,
        vout: 0,
        satoshis,
        derivation_prefix,
        derivation_suffix,
        sender_identity_key,
        locking_script,
    }
}

/// Spend a restored output through the script interpreter.
///
/// This is the claim that matters. Not "the rows came back" -- rows can come
/// back with the wrong derivation coordinates and produce a wallet that owns
/// nothing. This derives the private key from the *restored* coordinates and
/// makes the interpreter accept the resulting unlocking script against the
/// *restored* locking script. Nothing here touches the network: a signature is
/// arithmetic, and broadcast is not required to prove authority to spend.
fn assert_restored_output_is_spendable(
    restored: &Output,
    receiver_root: &PrivateKey,
    sender_identity_key: &str,
) {
    let prefix = restored
        .derivation_prefix
        .clone()
        .expect("restored output must carry its derivation prefix");
    let suffix = restored
        .derivation_suffix
        .clone()
        .expect("restored output must carry its derivation suffix");
    let locking_script_bytes = restored
        .locking_script
        .clone()
        .expect("restored output must carry its locking script");

    let sender_pub =
        bsv::primitives::public_key::PublicKey::from_string(sender_identity_key).unwrap();
    let template = ScriptTemplateBRC29::new(prefix, suffix);
    let p2pkh = template
        .unlock(receiver_root, &sender_pub)
        .expect("restored coordinates must derive a spending key");

    let source_locking_script = LockingScript::from_binary(&locking_script_bytes);
    let source_satoshis = restored.satoshis as u64;
    let sighash_type = 0x41; // SIGHASH_ALL | SIGHASH_FORKID

    let mut tx = BsvTransaction::new();
    tx.add_input(TransactionInput {
        source_transaction: None,
        source_txid: Some(restored.txid.clone().unwrap()),
        source_output_index: restored.vout as u32,
        unlocking_script: Some(UnlockingScript::from_binary(&[])),
        sequence: 0xFFFF_FFFF,
    });
    tx.add_output(TransactionOutput {
        satoshis: Some(source_satoshis - 100),
        locking_script: LockingScript::from_binary(&[
            0x76, 0xa9, 0x14, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
            0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x88, 0xac,
        ]),
        change: false,
    });

    let preimage = tx
        .sighash_preimage(0, sighash_type, source_satoshis, &source_locking_script)
        .unwrap();
    let unlocking_script = p2pkh.unlock(&preimage).expect("produce unlocking script");

    let params = SpendParams {
        locking_script: source_locking_script,
        unlocking_script,
        source_txid: restored.txid.clone().unwrap(),
        source_output_index: restored.vout as usize,
        source_satoshis,
        transaction_version: tx.version,
        transaction_lock_time: tx.lock_time,
        transaction_sequence: tx.inputs[0].sequence,
        other_inputs: vec![],
        other_outputs: tx.outputs.clone(),
        input_index: 0,
    };

    assert!(
        Spend::new(params).validate().unwrap_or(false),
        "a key derived from the restored coordinates must satisfy the restored locking script"
    );
}

/// The whole claim, end to end: container + password in, spendable wallet out,
/// with the network wired to explode.
#[tokio::test]
async fn recovery_from_container_alone_rederives_keys_and_spends() {
    let dir = temp_dir("recover");
    let active_path = dir.join("active.db").to_string_lossy().to_string();
    let restored_path = dir.join("restored.db").to_string_lossy().to_string();

    let receiver_root = PrivateKey::from_random().unwrap();
    let sender = PrivateKey::from_random().unwrap();
    let receiver_identity = receiver_root.to_public_key().to_der_hex();

    // --- Before: a funded wallet, and the backup its owner takes away ---
    let seeded = {
        let active = open_sqlite(&active_path).await;
        let seeded = seed_payment(&active, &receiver_identity, &receiver_root, &sender).await;
        let document = export_brc38(&active, &receiver_identity).await.unwrap();
        let container = encrypt_brc39(&document, PASSWORD, None).unwrap();
        std::fs::write(dir.join("backup.bin"), &container).unwrap();
        seeded
    };

    // The original store is now gone as far as the rest of this test is
    // concerned: everything below starts from the container and the password.
    std::fs::remove_file(&active_path).unwrap();

    // --- After: recover ---
    let container = std::fs::read(dir.join("backup.bin")).unwrap();
    let document = decrypt_brc39(&container, PASSWORD).unwrap();

    let restored_storage = open_sqlite(&restored_path).await;
    import_brc38(
        &restored_storage,
        &document,
        &Brc38ImportOptions {
            mode: ImportMode::Restore,
        },
    )
    .await
    .unwrap();

    // The recovered wallet opens with the network wired to panic. If any part
    // of opening or reading a restored wallet reaches for a service, this
    // build call or the queries below abort the test by name.
    let recovered = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(receiver_root.clone())
        .with_sqlite(&restored_path)
        .with_services(Arc::new(NetworkTripwire))
        .without_monitor()
        .build()
        .await
        .expect("a recovered wallet must build with no network available");

    let outputs = restored_storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    txid: Some(seeded.txid.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(outputs.len(), 1, "the funded output must come back");
    let restored_output = &outputs[0];

    // Every field the spend depends on, compared against what was backed up.
    // A restore that returns rows but loses a derivation coordinate produces a
    // wallet that appears healthy and owns nothing, so these are asserted
    // individually rather than by row count.
    assert_eq!(restored_output.satoshis, seeded.satoshis);
    assert_eq!(restored_output.vout, seeded.vout);
    assert_eq!(
        restored_output.derivation_prefix.as_deref(),
        Some(seeded.derivation_prefix.as_str()),
        "derivation prefix is unrecoverable if lost -- 2^128 per output"
    );
    assert_eq!(
        restored_output.derivation_suffix.as_deref(),
        Some(seeded.derivation_suffix.as_str())
    );
    assert_eq!(
        restored_output.sender_identity_key.as_deref(),
        Some(seeded.sender_identity_key.as_str()),
        "the counterparty is half of the BRC-42 derivation"
    );
    assert_eq!(
        restored_output.locking_script.as_deref(),
        Some(seeded.locking_script.as_slice())
    );
    assert!(
        restored_output.spendable,
        "restored output must be spendable"
    );

    // The proof: derive from the restored coordinates and satisfy the script.
    assert_restored_output_is_spendable(
        restored_output,
        &receiver_root,
        &seeded.sender_identity_key,
    );

    recovered.wallet.destroy().await.unwrap();
}

/// The tripwire fires, so a green recovery test means something.
///
/// What this establishes, precisely: `WalletServices` is the network seam for
/// the restore path. `src/storage/`, `src/wallet/setup.rs` and `src/signer/`
/// contain no direct HTTP client -- every outbound call in this crate lives in
/// `services/`, `chaintracks/` (reached through `get_chain_tracker`, which
/// panics here) or `wab_client/` (a different feature, not on this path). A
/// caller that bypassed the trait entirely would not be caught, so this is a
/// guard on the seam, not a sandbox.
#[tokio::test]
#[should_panic(expected = "network isolation violated")]
async fn the_network_tripwire_fires() {
    let _ = NetworkTripwire.get_height().await;
}

/// A backup configured on the builder must exist as a backup on the manager.
#[tokio::test]
async fn builder_wires_a_backup_store() {
    let dir = temp_dir("wire");
    let active = dir.join("active.db").to_string_lossy().to_string();
    let backup = dir.join("backup.db").to_string_lossy().to_string();

    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(PrivateKey::from_random().unwrap())
        .with_sqlite(&active)
        .with_backup_sqlite(&backup)
        .with_services(Arc::new(NetworkTripwire))
        .without_monitor()
        .build()
        .await
        .expect("build with a backup");

    assert!(
        setup.storage.has_backup(),
        "a builder-configured backup must reach the storage manager"
    );
    assert_eq!(
        setup.storage.get_backup_stores().await.len(),
        1,
        "the backup must be partitioned as a backup, not as a second active"
    );

    setup.wallet.destroy().await.unwrap();
}

/// A configured backup must actually hold the active store's contents.
///
/// `make_available` only partitions the stores. Without the build-time
/// replication this asserts, a freshly-configured backup is an empty file that
/// looks like protection -- the worst possible failure for this feature,
/// because it is silent until the day it is needed.
#[tokio::test]
async fn a_configured_backup_receives_the_active_contents() {
    let dir = temp_dir("recover");
    let active_path = dir.join("active.db").to_string_lossy().to_string();
    let backup_path = dir.join("backup.db").to_string_lossy().to_string();

    let receiver_root = PrivateKey::from_random().unwrap();
    let sender = PrivateKey::from_random().unwrap();
    let identity = receiver_root.to_public_key().to_der_hex();

    // Seed the active store before the wallet is built, so the build-time
    // replication has something to copy.
    let seeded = {
        let active = open_sqlite(&active_path).await;
        seed_payment(&active, &identity, &receiver_root, &sender).await
    };

    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(receiver_root.clone())
        .with_sqlite(&active_path)
        .with_backup_sqlite(&backup_path)
        .with_services(Arc::new(NetworkTripwire))
        .without_monitor()
        .build()
        .await
        .expect("build with a backup");

    let backup = open_sqlite(&backup_path).await;
    let outputs = backup
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    txid: Some(seeded.txid.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        outputs.len(),
        1,
        "the backup must hold the active store's outputs after build"
    );
    assert_eq!(
        outputs[0].derivation_prefix.as_deref(),
        Some(seeded.derivation_prefix.as_str()),
        "a backup without derivation coordinates is not a backup"
    );

    setup.wallet.destroy().await.unwrap();
}
