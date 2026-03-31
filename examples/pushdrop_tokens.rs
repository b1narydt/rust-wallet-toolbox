//! PushDrop Tokens Example
//!
//! Demonstrates minting data-bearing PushDrop tokens on-chain. A PushDrop token
//! embeds arbitrary data fields into a locking script that can only be spent by
//! the holder of the signing key.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> cargo run --example pushdrop_tokens
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - 64-character hex private key (must be funded).
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::script::templates::push_drop::PushDrop;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface,
};
use bsv::wallet::types::BooleanDefaultFalse;
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
            eprintln!("BSV_PRIVATE_KEY env var required (must be funded).");
            eprintln!("Run `cargo run --example setup_wallet` to generate one.");
            std::process::exit(1);
        }
    };

    println!("\nBuilding wallet...");
    let setup = WalletBuilder::new()
        .chain(chain)
        .root_key(private_key.clone())
        .with_sqlite("examples/data/wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    let balance = setup.wallet.balance(None).await?;
    println!("  Balance:      {} satoshis", balance);

    if balance < 100 {
        eprintln!("\nBalance too low to mint a PushDrop token.");
        eprintln!("Fund the wallet first via `cargo run --example setup_wallet`.");
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // 2. Create PushDrop locking script with data fields
    // -----------------------------------------------------------------------
    // The data fields are arbitrary byte arrays. In a real application these
    // could be JSON metadata, file hashes, protocol identifiers, etc.
    let data_fields: Vec<Vec<u8>> = vec![
        vec![1, 2, 3],       // Field 0: some identifier
        vec![4, 5, 6],       // Field 1: some payload
        b"hello token".to_vec(), // Field 2: human-readable label
    ];

    // Generate a signing key for the PushDrop. The holder of this key can
    // spend (redeem) the token later. Here we use a random key for simplicity.
    let signing_key = PrivateKey::from_random()?;

    println!("\nMinting PushDrop token...");
    println!("  Data fields: {} fields", data_fields.len());
    println!(
        "  Signing key (save to redeem): {}",
        signing_key.to_hex()
    );

    let pushdrop = PushDrop::new(data_fields.clone(), signing_key.clone());
    let locking_script = pushdrop.lock()?;
    let script_bytes = locking_script.to_binary();

    println!("  Locking script: {} bytes", script_bytes.len());

    // -----------------------------------------------------------------------
    // 3. Mint: create action with PushDrop output
    // -----------------------------------------------------------------------
    let mint_result = setup
        .wallet
        .create_action(
            CreateActionArgs {
                description: "Mint PushDrop token".to_string(),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(script_bytes),
                    satoshis: 1,
                    output_description: "PushDrop token output".to_string(),
                    basket: Some("tokens".to_string()),
                    custom_instructions: None,
                    tags: vec!["pushdrop-example".to_string()],
                }],
                lock_time: None,
                version: None,
                labels: vec!["pushdrop-mint".to_string()],
                options: Some(CreateActionOptions {
                    no_send: BooleanDefaultFalse(Some(false)),
                    ..Default::default()
                }),
                reference: None,
            },
            None,
        )
        .await?;

    let txid = mint_result.txid.as_deref().unwrap_or("(none)");
    println!("\n  Minted token!");
    println!("  TXID: {}", txid);
    println!("  Outpoint: {}.0", txid);

    // -----------------------------------------------------------------------
    // 4. Print token info and redemption instructions
    // -----------------------------------------------------------------------
    // In a full implementation, redeeming the token would involve:
    // 1. Looking up the UTXO by outpoint (txid.0)
    // 2. Creating a new transaction that spends the PushDrop UTXO as an input
    // 3. Providing the unlocking script (signature) using the signing key
    // 4. Calling create_action with the input referencing the minted outpoint
    //
    // The PushDrop unlocking script requires a valid signature from the signing
    // key used during minting. The `PushDrop::unlock()` method on the bsv-sdk
    // provides this functionality within a transaction signing flow.

    println!("\n--- Redemption Info ---");
    println!("To redeem this token in a future transaction:");
    println!("  1. Reference outpoint: {}.0", txid);
    println!(
        "  2. Use signing key: {}",
        signing_key.to_hex()
    );
    println!("  3. Create a transaction input spending that outpoint");
    println!("  4. The PushDrop template will produce the unlocking script");

    // Print data fields that were embedded
    println!("\n--- Embedded Data Fields ---");
    for (i, field) in data_fields.iter().enumerate() {
        if let Ok(s) = std::str::from_utf8(field) {
            println!("  Field {}: {:?} (text: \"{}\")", i, field, s);
        } else {
            println!("  Field {}: {:?}", i, field);
        }
    }

    let balance_after = setup.wallet.balance(None).await?;
    println!("\nBalance after mint: {} satoshis", balance_after);

    Ok(())
}
