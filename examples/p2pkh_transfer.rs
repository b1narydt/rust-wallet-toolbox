//! P2PKH Transfer Example
//!
//! Transfers 42 satoshis between two wallets using P2PKH locking scripts.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> BSV_PRIVATE_KEY_2=<hex> cargo run --example p2pkh_transfer
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - Sender's 64-character hex private key (must be funded).
//! - `BSV_PRIVATE_KEY_2` - Receiver's 64-character hex private key.
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::interfaces::{CreateActionArgs, CreateActionOutput, WalletInterface};
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::WalletBuilder;

/// Read the `BSV_CHAIN` env var and return the corresponding `Chain`.
/// Defaults to `Chain::Test` if unset or unrecognised.
fn get_chain() -> Chain {
    match std::env::var("BSV_CHAIN").as_deref() {
        Ok("main") => Chain::Main,
        _ => Chain::Test,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::from_filename("examples/.env").ok();

    let chain = get_chain();
    println!("Chain: {}", chain);

    // -----------------------------------------------------------------------
    // 1. Load both private keys from env
    // -----------------------------------------------------------------------
    let sender_key = PrivateKey::from_hex(
        &std::env::var("BSV_PRIVATE_KEY")
            .expect("BSV_PRIVATE_KEY env var required (sender, must be funded)"),
    )?;
    let receiver_key = PrivateKey::from_hex(
        &std::env::var("BSV_PRIVATE_KEY_2")
            .expect("BSV_PRIVATE_KEY_2 env var required (receiver)"),
    )?;

    // -----------------------------------------------------------------------
    // 2. Build both wallets with separate SQLite files
    // -----------------------------------------------------------------------
    println!("\nBuilding sender wallet...");
    let sender_setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(sender_key.clone())
        .with_sqlite("wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Building receiver wallet...");
    let receiver_setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(receiver_key.clone())
        .with_sqlite("receiver.db")
        .with_default_services()
        .build()
        .await?;

    // -----------------------------------------------------------------------
    // 3. Print identity keys and balances
    // -----------------------------------------------------------------------
    let sender_balance = sender_setup.wallet.balance(None).await?;
    let receiver_balance = receiver_setup.wallet.balance(None).await?;

    println!("\nSender:");
    println!("  Identity key: {}", sender_setup.identity_key);
    println!("  Balance:      {} satoshis", sender_balance);

    println!("Receiver:");
    println!("  Identity key: {}", receiver_setup.identity_key);
    println!("  Balance:      {} satoshis", receiver_balance);

    if sender_balance < 42 {
        eprintln!("\nSender balance too low. Fund the sender wallet first.");
        eprintln!("Run `cargo run --example setup_wallet` with BSV_PRIVATE_KEY to get a funding address.");
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // 4. Create P2PKH locking script from receiver's public key
    // -----------------------------------------------------------------------
    let receiver_pub = receiver_key.to_public_key();
    let hash_vec = receiver_pub.to_hash();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&hash_vec);

    let p2pkh = P2PKH::from_public_key_hash(hash);
    let locking_script = p2pkh.lock()?;
    let lock_bytes = locking_script.to_binary();

    // -----------------------------------------------------------------------
    // 5. Sender creates action sending 42 satoshis to receiver
    // -----------------------------------------------------------------------
    println!("\nCreating P2PKH transfer of 42 satoshis...");

    let result = sender_setup
        .wallet
        .create_action(
            CreateActionArgs {
                description: "P2PKH transfer example".to_string(),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(lock_bytes),
                    satoshis: 42,
                    output_description: "P2PKH to receiver".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: vec![],
                }],
                lock_time: None,
                version: None,
                labels: vec!["p2pkh-transfer".to_string()],
                options: None,
                reference: None,
            },
            None,
        )
        .await?;

    // -----------------------------------------------------------------------
    // 6. Print txid and updated balances
    // -----------------------------------------------------------------------
    if let Some(ref txid) = result.txid {
        println!("\nTransaction sent!");
        println!("  TXID: {}", txid);
    } else {
        println!("\nTransaction created (no txid returned -- check sign_and_process option).");
    }

    let sender_balance_after = sender_setup.wallet.balance(None).await?;
    let receiver_balance_after = receiver_setup.wallet.balance(None).await?;

    println!("\nUpdated balances:");
    println!("  Sender:   {} satoshis", sender_balance_after);
    println!("  Receiver: {} satoshis", receiver_balance_after);

    Ok(())
}
