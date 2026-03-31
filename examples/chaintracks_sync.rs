//! Chaintracks Sync Example
//!
//! Demonstrates local block header synchronization using the Chaintracks
//! engine with in-memory storage and the WhatsOnChain bulk ingestor.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! 1. Fetches recent headers from WhatsOnChain
//! 2. Inserts them into `MemoryStorage` with real block heights
//! 3. Wraps storage in a `Chaintracks` instance
//! 4. Prints the chain tip and chaintracks info
//!
//! # Usage
//!
//! ```bash
//! # Testnet (default):
//! cargo run --example chaintracks_sync
//!
//! # Mainnet:
//! BSV_CHAIN=main cargo run --example chaintracks_sync
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
    // 1. Fetch recent headers from WhatsOnChain
    // -----------------------------------------------------------------------
    let ingestor_opts = match chain {
        Chain::Main => BulkWocOptions::mainnet(),
        Chain::Test => BulkWocOptions::testnet(),
    };
    let ingestor = BulkWocIngestor::new(ingestor_opts)?;

    println!("Fetching recent headers from WhatsOnChain...");
    let headers = ingestor.get_recent_headers().await?;
    println!("Received {} headers.", headers.len());

    // -----------------------------------------------------------------------
    // 2. Insert headers into storage with real heights
    // -----------------------------------------------------------------------
    // We populate storage before creating the Chaintracks orchestrator so
    // that each header retains its real block height. The orchestrator's
    // add_header + process_pending_headers pipeline recomputes height from
    // parent linkage, which requires all ancestors to be present. With only
    // ~10 recent headers the first parent is missing and heights reset to 0.
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
    println!("Inserted {} headers into storage.", sorted_headers.len());

    // -----------------------------------------------------------------------
    // 3. Wrap storage in Chaintracks
    // -----------------------------------------------------------------------
    let options = match chain {
        Chain::Main => ChaintracksOptions::default_mainnet(),
        Chain::Test => ChaintracksOptions::default_testnet(),
    };
    let ct = Chaintracks::new(options, Box::new(storage));
    ct.make_available().await?;

    // -----------------------------------------------------------------------
    // 4. Print chain tip
    // -----------------------------------------------------------------------
    match ct.find_chain_tip_header().await? {
        Some(tip) => {
            println!();
            println!("Chain tip:");
            println!("  Height : {}", tip.height);
            println!("  Hash   : {}", tip.hash);
            println!("  Merkle : {}", tip.merkle_root);
            println!("  Time   : {}", tip.time);
        }
        None => {
            println!("No chain tip found (storage may be empty).");
        }
    }

    // -----------------------------------------------------------------------
    // 5. Print chaintracks info
    // -----------------------------------------------------------------------
    let info = ct.get_info().await?;
    println!();
    println!("Chaintracks info:");
    println!("  Chain          : {:?}", info.chain);
    println!("  Storage type   : {}", info.storage_type);
    println!("  Tip height     : {:?}", info.tip_height);
    println!("  Live range     : {:?}", info.live_range);
    println!("  Ingestor count : {}", info.ingestor_count);
    println!("  Is listening   : {}", info.is_listening);
    println!("  Is synchronised: {}", info.is_synchronized);

    Ok(())
}
