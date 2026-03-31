//! Setup Wallet Example
//!
//! Generates or imports a private key, builds a wallet with SQLite storage,
//! displays a P2PKH address for faucet funding, and checks the balance.
//!
//! If `BSV_FUND_TXID` is set, fetches that transaction from WhatsOnChain,
//! finds the output paying to our P2PKH address, and internalizes it so
//! the wallet recognizes the funds. The txid is then persisted to
//! `examples/.env` so subsequent runs skip re-internalization.
//!
//! Creates an `examples/.env` file with `BSV_CHAIN`, `BSV_PRIVATE_KEY`, and
//! `BSV_PRIVATE_KEY_2` so that other examples can read configuration
//! automatically without manually exporting environment variables.
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
//! # Internalize a faucet transaction:
//! BSV_FUND_TXID=<txid-hex> cargo run --example setup_wallet
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
//! - `BSV_FUND_TXID` - Transaction ID of a faucet payment to internalize.

use bsv::primitives::private_key::PrivateKey;
use bsv::script::address::Address;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::transaction::beef::{Beef, BEEF_V2};
use bsv::transaction::beef_tx::BeefTx;
use bsv::transaction::transaction::Transaction;
use bsv::wallet::interfaces::{
    BasketInsertion, InternalizeActionArgs, InternalizeOutput, WalletInterface,
};
use bsv::wallet::types::BooleanDefaultTrue;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::WalletBuilder;
use std::io::Cursor;

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
    // Load any existing .env so re-running picks up previous values.
    dotenvy::from_filename("examples/.env").ok();

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
    // 1b. Second private key — read from env or generate
    // -----------------------------------------------------------------------
    let private_key_2 = match std::env::var("BSV_PRIVATE_KEY_2") {
        Ok(hex) => {
            let key = PrivateKey::from_hex(&hex)?;
            println!("Loaded second private key from BSV_PRIVATE_KEY_2");
            key
        }
        Err(_) => {
            let key = PrivateKey::from_random()?;
            println!("Generated second private key (for two-wallet examples).");
            key
        }
    };

    // -----------------------------------------------------------------------
    // 1c. Capture BSV_FUND_TXID early (before writing .env)
    // -----------------------------------------------------------------------
    let fund_txid = std::env::var("BSV_FUND_TXID").ok().filter(|s| !s.is_empty());

    // -----------------------------------------------------------------------
    // 1d. Write examples/.env so other examples pick up these values
    // -----------------------------------------------------------------------
    let chain_str = match chain {
        Chain::Main => "main",
        Chain::Test => "test",
    };
    let mut env_content = format!(
        "BSV_CHAIN={}\nBSV_PRIVATE_KEY={}\nBSV_PRIVATE_KEY_2={}\n",
        chain_str,
        private_key.to_hex(),
        private_key_2.to_hex()
    );
    if let Some(ref txid) = fund_txid {
        env_content.push_str(&format!("BSV_FUND_TXID={}\n", txid));
    }
    std::fs::write("examples/.env", &env_content)?;
    println!();
    println!("Wrote examples/.env (other examples will read this automatically).");

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

    // -----------------------------------------------------------------------
    // 5. Internalize an external P2PKH transaction (e.g. from a faucet)
    // -----------------------------------------------------------------------
    if let Some(ref txid) = fund_txid {
        if balance > 0 {
            println!();
            println!(
                "BSV_FUND_TXID is set but balance is already {} satoshis -- skipping internalization.",
                balance
            );
        } else {
            println!();
            println!("BSV_FUND_TXID={}", txid);
            println!("Fetching raw transaction from services...");

            let services = setup
                .services
                .as_ref()
                .expect("wallet services required for internalization");

            let raw_tx_result = services.get_raw_tx(txid, false).await;

            let raw_tx_bytes = raw_tx_result
                .raw_tx
                .ok_or_else(|| {
                    let msg = raw_tx_result
                        .error
                        .unwrap_or_else(|| "tx not found".to_string());
                    format!("Failed to fetch raw tx {}: {}", txid, msg)
                })?;

            println!("  Raw tx: {} bytes", raw_tx_bytes.len());

            // Parse the transaction to inspect outputs.
            let tx = Transaction::from_binary(&mut Cursor::new(&raw_tx_bytes))?;

            // Build the expected P2PKH locking script for our public key.
            let pub_key = private_key.to_public_key();
            let pub_key_hash = pub_key.to_hash();
            let mut hash20 = [0u8; 20];
            hash20.copy_from_slice(&pub_key_hash);
            let p2pkh = P2PKH::from_public_key_hash(hash20);
            let expected_lock = p2pkh.lock()?;
            let expected_lock_bytes = expected_lock.to_binary();

            // Find the output that pays to our address.
            let mut found_index: Option<u32> = None;
            for (i, output) in tx.outputs.iter().enumerate() {
                if output.locking_script.to_binary() == expected_lock_bytes {
                    found_index = Some(i as u32);
                    let sats = output.satoshis.unwrap_or(0);
                    println!(
                        "  Found our output at index {} ({} satoshis)",
                        i, sats
                    );
                    break;
                }
            }

            let output_index = found_index.ok_or_else(|| {
                format!(
                    "No output in tx {} pays to our P2PKH address {}",
                    txid, address
                )
            })?;

            // Build an Atomic BEEF wrapping this single transaction.
            let beef_tx = BeefTx::from_tx(tx, None)?;
            let mut beef = Beef::new(BEEF_V2);
            beef.txs.push(beef_tx);
            let beef_bytes = beef.to_binary_atomic(txid)?;

            println!(
                "  Built Atomic BEEF: {} bytes, internalizing...",
                beef_bytes.len()
            );

            // Internalize via BasketInsertion (not WalletPayment -- this is
            // a plain P2PKH, not a BRC-29 payment).
            let result = setup
                .wallet
                .internalize_action(
                    InternalizeActionArgs {
                        tx: beef_bytes,
                        description: "Faucet funding".to_string(),
                        labels: vec!["funding".to_string()],
                        seek_permission: BooleanDefaultTrue(Some(false)),
                        outputs: vec![InternalizeOutput::BasketInsertion {
                            output_index,
                            insertion: BasketInsertion {
                                basket: "default".to_string(),
                                custom_instructions: None,
                                tags: vec![],
                            },
                        }],
                    },
                    None,
                )
                .await?;

            println!("  Accepted: {}", result.accepted);

            // Re-check and display the updated balance.
            let new_balance = setup.wallet.balance(None).await?;
            println!();
            println!(
                "Updated balance: {} satoshis  (was {} before internalization)",
                new_balance, balance
            );

            // .env already includes BSV_FUND_TXID (written in step 1d above).
        }
    }

    Ok(())
}
