//! Chaintracks Validate Example
//!
//! Demonstrates SPV merkle root validation using the Chaintracks engine.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! 1. Fetches recent headers and inserts them into storage with real heights
//! 2. Retrieves the chain tip header
//! 3. Validates the real merkle root against its height (should be true)
//! 4. Validates a bogus all-zero root (should be false)
//! 5. Explains how this integrates with BEEF transaction validation
//!
//! # Usage
//!
//! ```bash
//! # Testnet (default):
//! cargo run --example chaintracks_validate
//!
//! # Mainnet:
//! BSV_CHAIN=main cargo run --example chaintracks_validate
//! ```
//!
//! # Environment Variables
//!
//! - `BSV_CHAIN` - `"main"` for mainnet or `"test"` (default) for testnet.

use bsv_wallet_toolbox::chaintracks::{
    calculate_work, BulkWocIngestor, BulkWocOptions, Chaintracks, ChaintracksClient,
    ChaintracksOptions, ChaintracksStorageIngest, LiveBlockHeader, MemoryStorage,
};
use bsv_wallet_toolbox::types::Chain;

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
    // 1. Fetch and insert headers with real heights
    // -----------------------------------------------------------------------
    let ingestor_opts = match chain {
        Chain::Main => BulkWocOptions::mainnet(),
        Chain::Test => BulkWocOptions::testnet(),
    };
    let ingestor = BulkWocIngestor::new(ingestor_opts)?;

    println!("Fetching recent headers from WhatsOnChain...");
    let headers = ingestor.get_recent_headers().await?;
    println!("Received {} headers.", headers.len());

    // Populate storage directly so headers keep their real block heights.
    let storage = MemoryStorage::new(chain.clone());

    let mut sorted_headers = headers.clone();
    sorted_headers.sort_by_key(|h| h.height);

    for header in &sorted_headers {
        let live = LiveBlockHeader {
            version: header.version,
            previous_hash: header.previous_hash.clone(),
            merkle_root: header.merkle_root.clone(),
            time: header.time,
            bits: header.bits,
            nonce: header.nonce,
            height: header.height,
            hash: header.hash.clone(),
            chain_work: calculate_work(header.bits),
            is_chain_tip: false,
            is_active: true,
            header_id: None,
            previous_header_id: None,
        };
        storage.insert_header(live).await?;
    }

    let options = match chain {
        Chain::Main => ChaintracksOptions::default_mainnet(),
        Chain::Test => ChaintracksOptions::default_testnet(),
    };
    let ct = Chaintracks::new(options, Box::new(storage));
    ct.make_available().await?;
    println!("Headers synced.");

    // -----------------------------------------------------------------------
    // 2. Get chain tip header
    // -----------------------------------------------------------------------
    let tip = match ct.find_chain_tip_header().await? {
        Some(t) => t,
        None => {
            println!("No chain tip found -- cannot validate. Exiting.");
            return Ok(());
        }
    };

    println!();
    println!("Chain tip: height={}, hash={}", tip.height, tip.hash);
    println!("  Merkle root: {}", tip.merkle_root);

    // -----------------------------------------------------------------------
    // 3. Validate the REAL merkle root for the tip height
    // -----------------------------------------------------------------------
    let valid = ct
        .is_valid_root_for_height(&tip.merkle_root, tip.height)
        .await?;
    println!();
    println!(
        "is_valid_root_for_height(real_root, {}) => {}",
        tip.height, valid
    );
    assert!(valid, "Real merkle root should validate as true");

    // -----------------------------------------------------------------------
    // 4. Validate a BOGUS merkle root (all zeros)
    // -----------------------------------------------------------------------
    let bogus_root = "0".repeat(64);
    let invalid = ct
        .is_valid_root_for_height(&bogus_root, tip.height)
        .await?;
    println!(
        "is_valid_root_for_height(\"0000...0000\", {}) => {}",
        tip.height, invalid
    );
    assert!(!invalid, "Bogus merkle root should validate as false");

    // -----------------------------------------------------------------------
    // 5. How this integrates with BEEF validation
    // -----------------------------------------------------------------------
    println!();
    println!("--- SPV / BEEF Integration ---");
    println!();
    println!("When receiving a BEEF (Background Evaluation Extended Format)");
    println!("transaction envelope, the recipient must verify that every");
    println!("referenced merkle root is anchored in a valid block header.");
    println!();
    println!("Chaintracks provides `is_valid_root_for_height(root, height)`");
    println!("which checks the locally-synced header chain. This allows");
    println!("offline SPV validation without trusting a third-party service");
    println!("at verification time.");
    println!();
    println!("Workflow:");
    println!("  1. Sync headers (bulk + live ingestors keep the chain current).");
    println!("  2. Receive a BEEF envelope containing merkle proofs.");
    println!("  3. For each proof, compute the merkle root from the tx + path.");
    println!("  4. Call `is_valid_root_for_height(root, height)` to confirm the");
    println!("     root belongs to a known, active block at that height.");
    println!("  5. If all roots validate, the transaction inputs are SPV-proven.");

    Ok(())
}
