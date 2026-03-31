//! Internalize Payment Example
//!
//! Demonstrates BRC-29 payment internalization between two wallets.
//! The sender creates a BRC-29 payment, and the receiver internalizes
//! it using `internalize_action` with the BEEF bytes and derivation info.
//!
//! This is the same pattern as `brc29_transfer.rs` but focused on
//! demonstrating the internalization flow explicitly.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> BSV_PRIVATE_KEY_2=<hex> cargo run --example internalize_payment
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - Sender's 64-character hex private key (must be funded).
//! - `BSV_PRIVATE_KEY_2` - Receiver's 64-character hex private key.
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use base64::Engine;
use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, InternalizeActionArgs,
    InternalizeOutput, Payment, WalletInterface,
};
use bsv::wallet::types::BooleanDefaultTrue;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::utility::script_template_brc29::ScriptTemplateBRC29;
use bsv_wallet_toolbox::WalletBuilder;

/// Read the `BSV_CHAIN` env var and return the corresponding `Chain`.
fn get_chain() -> Chain {
    match std::env::var("BSV_CHAIN").as_deref() {
        Ok("main") => Chain::Main,
        _ => Chain::Test,
    }
}

/// Generate a random base64-encoded string of `n` random bytes.
fn random_base64(n: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::from_filename("examples/.env").ok();

    let chain = get_chain();
    println!("Chain: {}", chain);

    // -----------------------------------------------------------------------
    // 1. Load both private keys and build wallets with separate SQLite files
    // -----------------------------------------------------------------------
    let sender_key = PrivateKey::from_hex(
        &std::env::var("BSV_PRIVATE_KEY")
            .expect("BSV_PRIVATE_KEY env var required (sender, must be funded)"),
    )?;
    let receiver_key = PrivateKey::from_hex(
        &std::env::var("BSV_PRIVATE_KEY_2")
            .expect("BSV_PRIVATE_KEY_2 env var required (receiver)"),
    )?;

    println!("\nBuilding sender wallet...");
    let sender_setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(sender_key.clone())
        .with_sqlite("internalize_sender.db")
        .with_default_services()
        .build()
        .await?;

    println!("Building receiver wallet...");
    let receiver_setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(receiver_key.clone())
        .with_sqlite("internalize_receiver.db")
        .with_default_services()
        .build()
        .await?;

    let sender_balance = sender_setup.wallet.balance(None).await?;
    println!("\nSender:");
    println!("  Identity key: {}", sender_setup.identity_key);
    println!("  Balance:      {} satoshis", sender_balance);
    println!("Receiver:");
    println!("  Identity key: {}", receiver_setup.identity_key);

    if sender_balance < 500 {
        eprintln!("\nSender balance too low. Fund the sender wallet first.");
        eprintln!("Run `BSV_PRIVATE_KEY=<sender_key> cargo run --example setup_wallet`");
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // 2. Generate BRC-29 derivation prefix and suffix
    // -----------------------------------------------------------------------
    let derivation_prefix = random_base64(8);
    let derivation_suffix = random_base64(8);

    println!("\nBRC-29 derivation:");
    println!("  Prefix: {}", derivation_prefix);
    println!("  Suffix: {}", derivation_suffix);

    // -----------------------------------------------------------------------
    // 3. Create BRC-29 locking script for the receiver
    // -----------------------------------------------------------------------
    let template = ScriptTemplateBRC29::new(derivation_prefix.clone(), derivation_suffix.clone());
    let receiver_pub = receiver_setup.wallet.identity_key.clone();
    let lock_script = template.lock(&sender_key, &receiver_pub)?;

    // -----------------------------------------------------------------------
    // 4. Sender creates the BRC-29 payment transaction
    // -----------------------------------------------------------------------
    let payment_amount = 500;
    println!(
        "\nSender creating BRC-29 payment of {} satoshis...",
        payment_amount
    );

    let custom_instructions = serde_json::json!({
        "derivationPrefix": derivation_prefix,
        "derivationSuffix": derivation_suffix,
        "type": "BRC29"
    })
    .to_string();

    let result = sender_setup
        .wallet
        .create_action(
            CreateActionArgs {
                description: "BRC29 payment for internalize example".to_string(),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(lock_script),
                    satoshis: payment_amount,
                    output_description: "BRC29 payment".to_string(),
                    basket: None,
                    custom_instructions: Some(custom_instructions),
                    tags: vec![],
                }],
                lock_time: None,
                version: None,
                labels: vec!["internalize-example".to_string()],
                options: Some(CreateActionOptions {
                    randomize_outputs: BooleanDefaultTrue(Some(false)),
                    accept_delayed_broadcast: BooleanDefaultTrue(Some(false)),
                    ..Default::default()
                }),
                reference: None,
            },
            None,
        )
        .await?;

    let txid = result.txid.as_deref().unwrap_or("(none)");
    println!("  TXID: {}", txid);

    let tx_bytes = result
        .tx
        .ok_or("create_action returned no tx bytes (needed for internalize_action)")?;

    println!("  TX bytes (BEEF): {} bytes", tx_bytes.len());

    // -----------------------------------------------------------------------
    // 5. Receiver internalizes the payment
    // -----------------------------------------------------------------------
    println!("\nReceiver internalizing payment...");
    println!("  This registers the incoming UTXO in the receiver's wallet.");

    let internalize_result = receiver_setup
        .wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: tx_bytes,
                description: "BRC29 payment received via internalize".to_string(),
                labels: vec!["internalize-example".to_string()],
                seek_permission: BooleanDefaultTrue(Some(false)),
                outputs: vec![InternalizeOutput::WalletPayment {
                    output_index: 0,
                    payment: Payment {
                        derivation_prefix: derivation_prefix.into_bytes(),
                        derivation_suffix: derivation_suffix.into_bytes(),
                        sender_identity_key: sender_setup.wallet.identity_key.clone(),
                    },
                }],
            },
            None,
        )
        .await?;

    println!("  Accepted: {}", internalize_result.accepted);

    // -----------------------------------------------------------------------
    // 6. Print updated balances
    // -----------------------------------------------------------------------
    let sender_balance_after = sender_setup.wallet.balance(None).await?;
    let receiver_balance_after = receiver_setup.wallet.balance(None).await?;

    println!("\n--- Results ---");
    println!("Sender balance:   {} -> {} satoshis", sender_balance, sender_balance_after);
    println!(
        "Receiver balance: {} satoshis (gained from internalized payment)",
        receiver_balance_after
    );
    println!("Payment TXID:     {}", txid);

    Ok(())
}
