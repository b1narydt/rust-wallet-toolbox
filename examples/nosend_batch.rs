//! NoSend Batch Example
//!
//! Demonstrates constructing multiple transactions without broadcasting them,
//! then sending them all at once using the `send_with` batching mechanism.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! The noSend flow:
//! 1. Create transaction with `no_send: true` -- wallet builds and signs it
//!    but does NOT broadcast. Returns `no_send_change` outpoints.
//! 2. Chain subsequent noSend transactions by passing the prior `no_send_change`
//!    so the wallet knows about uncommitted change UTXOs.
//! 3. Finally, create one last transaction with `send_with` containing all
//!    accumulated TXIDs to broadcast them as a batch.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> cargo run --example nosend_batch
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_PRIVATE_KEY` - 64-character hex private key (must be funded).
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv::primitives::private_key::PrivateKey;
use bsv::script::templates::p2pkh::P2PKH;
use bsv::script::templates::ScriptTemplateLock;
use bsv::wallet::interfaces::{
    CreateActionArgs, CreateActionOptions, CreateActionOutput, WalletInterface,
};
use bsv::wallet::types::{BooleanDefaultFalse, BooleanDefaultTrue};
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::WalletBuilder;

/// Read the `BSV_CHAIN` env var and return the corresponding `Chain`.
fn get_chain() -> Chain {
    match std::env::var("BSV_CHAIN").as_deref() {
        Ok("main") => Chain::Main,
        _ => Chain::Test,
    }
}

/// Build a simple P2PKH locking script from a private key.
fn make_p2pkh_lock(key: &PrivateKey) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pub_key = key.to_public_key();
    let hash_vec = pub_key.to_hash();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&hash_vec);
    let p2pkh = P2PKH::from_public_key_hash(hash);
    let script = p2pkh.lock()?;
    Ok(script.to_binary())
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
        .with_sqlite("wallet.db")
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    let balance = setup.wallet.balance(None).await?;
    println!("  Balance:      {} satoshis", balance);

    if balance < 300 {
        eprintln!("\nBalance too low for batch example (need at least 300 satoshis).");
        eprintln!("Fund the wallet first via `cargo run --example setup_wallet`.");
        return Ok(());
    }

    // Build a locking script for self-payment
    let lock_script = make_p2pkh_lock(&private_key)?;

    // -----------------------------------------------------------------------
    // 2. Create 3 noSend transactions, chaining change outpoints
    // -----------------------------------------------------------------------
    let mut collected_txids: Vec<String> = Vec::new();
    let mut accumulated_change: Vec<String> = Vec::new();

    for i in 0..3 {
        println!(
            "\nCreating noSend transaction {} of 3 (1 satoshi)...",
            i + 1
        );

        let result = setup
            .wallet
            .create_action(
                CreateActionArgs {
                    description: format!("noSend batch tx {}", i + 1),
                    input_beef: None,
                    inputs: vec![],
                    outputs: vec![CreateActionOutput {
                        locking_script: Some(lock_script.clone()),
                        satoshis: 1,
                        output_description: format!("batch output {}", i + 1),
                        basket: None,
                        custom_instructions: None,
                        tags: vec![],
                    }],
                    lock_time: None,
                    version: None,
                    labels: vec!["nosend-batch".to_string()],
                    options: Some(CreateActionOptions {
                        no_send: BooleanDefaultFalse(Some(true)),
                        no_send_change: accumulated_change.clone(),
                        randomize_outputs: BooleanDefaultTrue(Some(false)),
                        accept_delayed_broadcast: BooleanDefaultTrue(Some(true)),
                        ..Default::default()
                    }),
                    reference: None,
                },
                None,
            )
            .await?;

        let txid = result.txid.as_deref().unwrap_or("(unknown)");
        println!("  TXID: {}", txid);
        println!(
            "  Change outpoints: {} entries",
            result.no_send_change.len()
        );

        // Collect the txid for the final send_with batch
        if let Some(ref t) = result.txid {
            collected_txids.push(t.clone());
        }

        // Chain the no_send_change from this result into the next transaction
        // so the wallet knows about the uncommitted change UTXOs.
        accumulated_change = result.no_send_change;
    }

    // -----------------------------------------------------------------------
    // 3. Batch send all accumulated transactions
    // -----------------------------------------------------------------------
    println!("\n--- Batch Send ---");
    println!(
        "Sending {} accumulated transactions...",
        collected_txids.len()
    );
    for (i, txid) in collected_txids.iter().enumerate() {
        println!("  [{}] {}", i + 1, txid);
    }

    // The final transaction uses send_with to broadcast all prior noSend
    // transactions along with this one. This is a simple 1-sat self-payment
    // that triggers the batch.
    let batch_result = setup
        .wallet
        .create_action(
            CreateActionArgs {
                description: "Batch send trigger".to_string(),
                input_beef: None,
                inputs: vec![],
                outputs: vec![CreateActionOutput {
                    locking_script: Some(lock_script),
                    satoshis: 1,
                    output_description: "batch trigger output".to_string(),
                    basket: None,
                    custom_instructions: None,
                    tags: vec![],
                }],
                lock_time: None,
                version: None,
                labels: vec!["nosend-batch".to_string()],
                options: Some(CreateActionOptions {
                    no_send: BooleanDefaultFalse(Some(false)),
                    no_send_change: accumulated_change,
                    send_with: collected_txids.clone(),
                    randomize_outputs: BooleanDefaultTrue(Some(false)),
                    accept_delayed_broadcast: BooleanDefaultTrue(Some(true)),
                    ..Default::default()
                }),
                reference: None,
            },
            None,
        )
        .await?;

    let final_txid = batch_result.txid.as_deref().unwrap_or("(none)");
    println!("\nBatch trigger TXID: {}", final_txid);

    // Print send_with results
    if !batch_result.send_with_results.is_empty() {
        println!("\nSend-with results:");
        for swr in &batch_result.send_with_results {
            println!("  {} -> {:?}", swr.txid, swr.status);
        }
    }

    let balance_after = setup.wallet.balance(None).await?;
    println!("\nBalance after batch: {} satoshis", balance_after);

    Ok(())
}
