//! Setup Wallet Example
//!
//! Creates a local Rust wallet and funds it by requesting a BRC-42 payment
//! from a running BRC-100 desktop wallet (e.g. Babbage, MetaNet Client).
//!
//! Flow:
//! 1. Generate or load a private key for the Rust wallet
//! 2. Connect to the local desktop wallet via `HttpWalletJson` (JSON API)
//! 3. Desktop wallet derives a P2PKH address for our public key (BRC-42)
//! 4. Desktop wallet creates a transaction paying to that address
//! 5. Rust wallet internalizes the payment via `WalletPayment`
//! 6. Balance is now available for other examples
//!
//! # Prerequisites
//!
//! A BRC-100 wallet must be running locally on port 3321 (JSON API).
//!
//! # Usage
//!
//! ```bash
//! # First run — generates keys and funds wallet with 10000 sats
//! cargo run --example setup_wallet
//!
//! # Custom amount
//! cargo run --example setup_wallet -- 50000
//!
//! # Re-use existing key (skip funding if already funded)
//! cargo run --example setup_wallet
//!
//! # Switch to mainnet
//! BSV_CHAIN=main cargo run --example setup_wallet
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` — Hex private key. Generated if not set.
//! - `BSV_CHAIN` — `"main"` or `"test"` (default).
//! - `WALLET_URL` — Desktop wallet JSON API (default: `http://localhost:3321`).

use bsv::primitives::private_key::PrivateKey;
use bsv::primitives::public_key::PublicKey;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOutput, GetPublicKeyArgs, InternalizeActionArgs,
    InternalizeOutput, Payment, WalletInterface,
};
use bsv::wallet::substrates::http_wallet_json::HttpWalletJson;
use bsv::wallet::types::{BooleanDefaultTrue, Counterparty, CounterpartyType, Protocol};
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::WalletBuilder;

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
    let amount: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);

    println!("=== Setup Wallet ===");
    println!("Chain: {}", chain);
    println!("Requested amount: {} satoshis", amount);

    // -----------------------------------------------------------------------
    // 1. Private key — read from env or generate
    // -----------------------------------------------------------------------
    let private_key = match std::env::var("BSV_PRIVATE_KEY") {
        Ok(hex) => {
            println!("\nLoaded private key from BSV_PRIVATE_KEY");
            PrivateKey::from_hex(&hex)?
        }
        Err(_) => {
            let key = PrivateKey::from_random()?;
            println!("\nGenerated new private key.");
            println!("  BSV_PRIVATE_KEY={}", key.to_hex());
            key
        }
    };

    let private_key_2 = match std::env::var("BSV_PRIVATE_KEY_2") {
        Ok(hex) => PrivateKey::from_hex(&hex)?,
        Err(_) => PrivateKey::from_random()?,
    };

    // -----------------------------------------------------------------------
    // 2. Write examples/.env
    // -----------------------------------------------------------------------
    let chain_str = match chain {
        Chain::Main => "main",
        Chain::Test => "test",
    };
    let env_content = format!(
        "BSV_CHAIN={}\nBSV_PRIVATE_KEY={}\nBSV_PRIVATE_KEY_2={}\n",
        chain_str,
        private_key.to_hex(),
        private_key_2.to_hex(),
    );
    std::fs::write("examples/.env", &env_content)?;
    println!("Wrote examples/.env");

    // -----------------------------------------------------------------------
    // 3. Build the Rust wallet
    // -----------------------------------------------------------------------
    println!("\nBuilding Rust wallet...");
    let setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(private_key.clone())
        .with_sqlite("examples/data/wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("  Identity key: {}", setup.identity_key);

    let balance = setup.wallet.balance(None).await?;
    println!("  Balance: {} satoshis", balance);

    if balance >= amount {
        println!("\nWallet already funded. Ready to run other examples.");
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // 4. Connect to local desktop wallet
    // -----------------------------------------------------------------------
    let wallet_url =
        std::env::var("WALLET_URL").unwrap_or_else(|_| "http://localhost:3321".to_string());
    println!("\nConnecting to desktop wallet at {}...", wallet_url);

    let desktop = HttpWalletJson::new("rust-wallet-toolbox-examples", &wallet_url);

    // Get desktop wallet's identity key (the sender/payer)
    let sender_result = desktop
        .get_public_key(
            GetPublicKeyArgs {
                identity_key: true,
                protocol_id: None,
                key_id: None,
                counterparty: None,
                privileged: false,
                privileged_reason: None,
                for_self: None,
                seek_permission: None,
            },
            None,
        )
        .await?;
    let sender_identity_key = sender_result.public_key.clone();
    println!(
        "  Desktop wallet identity: {}",
        sender_identity_key.to_der_hex()
    );

    // Receiver = our Rust wallet's identity key
    let receiver_pub_key = PublicKey::from_string(&setup.identity_key)?;

    // -----------------------------------------------------------------------
    // 5. Desktop wallet derives payment address (BRC-42)
    // -----------------------------------------------------------------------
    // Generate random derivation parameters
    let derivation_prefix = base64_random(10);
    let derivation_suffix = base64_random(10);
    println!(
        "\n  Derivation: {} / {}",
        derivation_prefix, derivation_suffix
    );

    // Desktop wallet derives key for our Rust wallet as counterparty
    let derived_result = desktop
        .get_public_key(
            GetPublicKeyArgs {
                identity_key: false,
                protocol_id: Some(Protocol {
                    security_level: 2,
                    protocol: "3241645161d8".to_string(),
                }),
                key_id: Some(format!("{} {}", derivation_prefix, derivation_suffix)),
                counterparty: Some(Counterparty {
                    counterparty_type: CounterpartyType::Other,
                    public_key: Some(receiver_pub_key.clone()),
                }),
                privileged: false,
                privileged_reason: None,
                for_self: Some(false),
                seek_permission: None,
            },
            None,
        )
        .await?;

    let derived_pub_key = derived_result.public_key;
    println!("  Derived public key: {}", derived_pub_key.to_der_hex());

    // Build P2PKH locking script for the derived key
    let derived_hash = derived_pub_key.to_hash();
    let mut hash20 = [0u8; 20];
    hash20.copy_from_slice(&derived_hash);
    let p2pkh = P2PKH::from_public_key_hash(hash20);
    let locking_script = p2pkh.lock()?;

    // -----------------------------------------------------------------------
    // 6. Desktop wallet creates the payment transaction
    // -----------------------------------------------------------------------
    println!("\n  Desktop wallet sending {} satoshis...", amount);

    let create_result = desktop
        .create_action(
            CreateActionArgs {
                description: "Fund Rust wallet".to_string(),
                outputs: vec![CreateActionOutput {
                    locking_script: Some(locking_script.to_binary()),
                    satoshis: amount,
                    output_description: "BRC-42 payment to Rust wallet".to_string(),
                    basket: None,
                    custom_instructions: Some(serde_json::to_string(&serde_json::json!({
                        "derivationPrefix": derivation_prefix,
                        "derivationSuffix": derivation_suffix,
                        "payee": setup.identity_key,
                    }))?),
                    tags: vec![],
                }],
                inputs: vec![],
                input_beef: None,
                lock_time: None,
                version: None,
                labels: vec![],
                options: None,
                reference: None,
            },
            None,
        )
        .await?;

    let txid = create_result.txid.as_deref().unwrap_or("unknown");
    println!("  Transaction created: {}", txid);

    let beef_bytes = create_result
        .tx
        .ok_or("Desktop wallet did not return tx bytes")?;
    println!("  BEEF: {} bytes", beef_bytes.len());

    // -----------------------------------------------------------------------
    // 7. Rust wallet internalizes the payment
    // -----------------------------------------------------------------------
    println!("\n  Internalizing into Rust wallet...");

    let internalize_result = setup
        .wallet
        .internalize_action(
            InternalizeActionArgs {
                tx: beef_bytes.clone(),
                description: "Funding from desktop wallet".to_string(),
                labels: vec!["funding".to_string()],
                seek_permission: BooleanDefaultTrue(Some(false)),
                outputs: vec![InternalizeOutput::WalletPayment {
                    output_index: 0,
                    payment: Payment {
                        derivation_prefix: derivation_prefix.into_bytes(),
                        derivation_suffix: derivation_suffix.into_bytes(),
                        sender_identity_key: sender_identity_key,
                    },
                }],
            },
            None,
        )
        .await?;

    println!("  Accepted: {}", internalize_result.accepted);

    // -----------------------------------------------------------------------
    // 7b. Broadcast the BEEF to miners
    // -----------------------------------------------------------------------
    println!("\n  Broadcasting to miners...");
    if let Some(ref services) = setup.services {
        use bsv_wallet_toolbox::services::traits::WalletServices;
        let results = services
            .post_beef(&beef_bytes, &[txid.to_string()])
            .await;
        for r in &results {
            let status_msg = if let Some(ref err) = r.error {
                format!("{} ({})", r.status, err)
            } else {
                r.status.clone()
            };
            println!("    {}: {}", r.name, status_msg);
        }
    }

    // -----------------------------------------------------------------------
    // 8. Verify balance
    // -----------------------------------------------------------------------
    let new_balance = setup.wallet.balance(None).await?;
    println!("\n=== Wallet funded! ===");
    println!("  Balance: {} satoshis", new_balance);
    println!("  TXID: {}", txid);
    println!("\n  Ready to run other examples:");
    println!("    cargo run --example list_balance");
    println!("    cargo run --example chaintracks_sync");
    println!("    cargo run --example p2pkh_transfer");

    Ok(())
}

fn base64_random(len: usize) -> String {
    use rand::Rng;
    let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)
}
