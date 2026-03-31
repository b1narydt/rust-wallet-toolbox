//! List Actions Example
//!
//! Lists the transaction history of a wallet with pagination.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> cargo run --example list_actions
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - 64-character hex private key.
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::{ListActionsArgs, WalletInterface};
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
    // 2. List actions with pagination
    // -----------------------------------------------------------------------
    println!("\nListing recent actions...\n");

    let result = setup
        .wallet
        .list_actions(
            ListActionsArgs {
                labels: vec!["funding".to_string()],
                label_query_mode: None,
                include_labels: Default::default(),
                include_inputs: Default::default(),
                include_input_source_locking_scripts: Default::default(),
                include_input_unlocking_scripts: Default::default(),
                include_outputs: Default::default(),
                include_output_locking_scripts: Default::default(),
                limit: Some(25),
                offset: None,
                seek_permission: Default::default(),
            },
            None,
        )
        .await?;

    println!("Total actions: {}", result.total_actions);

    if result.actions.is_empty() {
        println!("  (no actions found)");
        println!("\nCreate some transactions first:");
        println!("  cargo run --example p2pkh_transfer");
        println!("  cargo run --example brc29_transfer");
    } else {
        // -----------------------------------------------------------------------
        // 3. Print table of actions
        // -----------------------------------------------------------------------
        println!(
            "\n  {:<64}  {:>10}  {:>5}  {:>11}  {}",
            "TXID", "Satoshis", "Dir", "Status", "Description"
        );
        println!("  {}", "-".repeat(110));

        for action in &result.actions {
            let direction = if action.is_outgoing { "out" } else { "in" };
            // Truncate description if too long
            let desc = if action.description.len() > 30 {
                format!("{}...", &action.description[..27])
            } else {
                action.description.clone()
            };

            println!(
                "  {:<64}  {:>10}  {:>5}  {:>11}  {}",
                action.txid,
                action.satoshis,
                direction,
                action.status.as_str(),
                desc,
            );
        }
    }

    Ok(())
}
