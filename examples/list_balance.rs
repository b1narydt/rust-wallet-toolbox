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
        .with_sqlite("examples/data/wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    // -----------------------------------------------------------------------
    // 2. Get balance and UTXO list
    // -----------------------------------------------------------------------
    // Note: BRC-100 reserves the "default" basket name — callers cannot pass
    // it directly to list_outputs. Instead, use balance_and_utxos() which
    // routes through the specOp wallet balance basket internally.
    println!("\nQuerying balance and UTXOs...\n");

    let wallet_balance = setup.wallet.balance_and_utxos(None).await?;

    println!("Total balance: {} satoshis", wallet_balance.total);
    println!("UTXOs: {}", wallet_balance.utxos.len());

    if wallet_balance.utxos.is_empty() {
        println!("  (no spendable outputs found)");
    } else {
        println!("\n  {:<68}  {:>10}", "Outpoint", "Satoshis");
        println!("  {}", "-".repeat(80));
        for utxo in &wallet_balance.utxos {
            println!("  {:<68}  {:>10}", utxo.outpoint, utxo.satoshis,);
        }
    }

    // -----------------------------------------------------------------------
    // 3. Also demonstrate the quick balance() convenience method
    // -----------------------------------------------------------------------
    let balance = setup.wallet.balance(None).await?;
    println!("\nQuick balance check: {} satoshis", balance);

    Ok(())
}
