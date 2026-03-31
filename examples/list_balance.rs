//! List Balance Example
//!
//! Queries the wallet balance and lists individual UTXOs.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> cargo run --example list_balance
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - 64-character hex private key.
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::{ListOutputsArgs, WalletInterface};
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::WalletBuilder;

/// Read the `BSV_CHAIN` env var and return the corresponding `Chain`.
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
    // 1. Load private key and build wallet
    // -----------------------------------------------------------------------
    let private_key = match std::env::var("BSV_PRIVATE_KEY") {
        Ok(hex) => PrivateKey::from_hex(&hex)?,
        Err(_) => {
            eprintln!("BSV_PRIVATE_KEY env var required.");
            eprintln!("Run `cargo run --example setup_wallet` to generate one.");
            std::process::exit(1);
        }
    };

    println!("\nBuilding wallet...");
    let setup = WalletBuilder::new()
        .chain(chain)
        .root_key(private_key)
        .with_sqlite("wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    // -----------------------------------------------------------------------
    // 2. List outputs from the "default" basket with pagination
    // -----------------------------------------------------------------------
    println!("\nListing outputs from \"default\" basket...\n");

    let list_result = setup
        .wallet
        .list_outputs(
            ListOutputsArgs {
                basket: "default".to_string(),
                tags: vec![],
                tag_query_mode: None,
                include: None,
                include_custom_instructions: Default::default(),
                include_tags: Default::default(),
                include_labels: Default::default(),
                limit: Some(25),
                offset: None,
                seek_permission: Default::default(),
            },
            None,
        )
        .await?;

    println!(
        "Total outputs in basket: {}",
        list_result.total_outputs
    );

    if list_result.outputs.is_empty() {
        println!("  (no outputs found)");
    } else {
        println!(
            "\n  {:<64}  {:>10}  {:>9}",
            "Outpoint", "Satoshis", "Spendable"
        );
        println!("  {}", "-".repeat(87));
        for output in &list_result.outputs {
            println!(
                "  {:<64}  {:>10}  {:>9}",
                output.outpoint,
                output.satoshis,
                if output.spendable { "yes" } else { "no" }
            );
        }
    }

    // -----------------------------------------------------------------------
    // 3. Call convenience balance method
    // -----------------------------------------------------------------------
    let balance = setup.wallet.balance(None).await?;

    println!("\nWallet balance (via balance()): {} satoshis", balance);

    Ok(())
}
