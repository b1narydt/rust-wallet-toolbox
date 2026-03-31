//! Setup Wallet Example
//!
//! Generates or imports a private key, builds a wallet with SQLite storage,
//! displays a P2PKH address for faucet funding, and checks the balance.
//!
//! # Usage
//!
//! ```bash
//! # Generate a new random key:
//! cargo run --example setup_wallet
//!
//! # Re-use an existing key:
//! BSV_PRIVATE_KEY=<64-char-hex> cargo run --example setup_wallet
//!
//! # Switch to mainnet:
//! BSV_CHAIN=main BSV_PRIVATE_KEY=<hex> cargo run --example setup_wallet
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - 64-character hex private key. If not set, a new
//!   random key is generated and printed so you can save it.
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::script::address::Address;
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
    let chain = get_chain();
    println!("Chain: {}", chain);

    // -----------------------------------------------------------------------
    // 1. Private key — read from env or generate a fresh one
    // -----------------------------------------------------------------------
    let private_key = match std::env::var("BSV_PRIVATE_KEY") {
        Ok(hex) => {
            let key = PrivateKey::from_hex(&hex)?;
            println!("Loaded private key from BSV_PRIVATE_KEY");
            key
        }
        Err(_) => {
            let key = PrivateKey::from_random()?;
            println!("Generated new random private key.");
            println!();
            println!("  BSV_PRIVATE_KEY={}", key.to_hex());
            println!();
            println!("  Save the line above! You will need it to re-open this wallet.");
            key
        }
    };

    // -----------------------------------------------------------------------
    // 2. Build the wallet
    // -----------------------------------------------------------------------
    println!();
    println!("Building wallet...");

    let setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(private_key.clone())
        .with_sqlite("wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    // -----------------------------------------------------------------------
    // 3. Derive a P2PKH address for funding
    // -----------------------------------------------------------------------
    let is_mainnet = chain == Chain::Main;
    let address = Address::from_public_key(&private_key.to_public_key(), is_mainnet);

    println!();
    println!("Funding address (P2PKH): {}", address);

    if !is_mainnet {
        println!();
        println!("Send testnet coins to this address using a faucet:");
        println!("  https://faucet.bitcoincloud.net/");
    }

    // -----------------------------------------------------------------------
    // 4. Check balance
    // -----------------------------------------------------------------------
    let balance = setup.wallet.balance(None).await?;

    println!();
    if balance > 0 {
        println!("Balance: {} satoshis  -- wallet is funded!", balance);
    } else {
        println!("Balance: 0 satoshis  -- not yet funded.");
        println!("Fund the address above, then run this example again.");
    }

    Ok(())
}
