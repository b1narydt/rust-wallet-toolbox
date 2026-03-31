//! Chaintracks Sync Example
//!
//! Demonstrates local block header synchronization using the Chaintracks
//! engine with in-memory storage and the WhatsOnChain bulk ingestor.
//!
//! 1. Creates a `MemoryStorage` + `Chaintracks` instance
//! 2. Fetches recent headers from WhatsOnChain
//! 3. Feeds each header into chaintracks as a `BaseBlockHeader`
//! 4. Processes the pending header queue
//! 5. Prints the chain tip and chaintracks info
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
    BaseBlockHeader, BulkWocIngestor, BulkWocOptions, Chaintracks, ChaintracksClient,
    ChaintracksOptions, MemoryStorage,
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
    let chain = get_chain();
    println!("Chain: {}", chain);

    // -----------------------------------------------------------------------
    // 1. Set up Chaintracks with in-memory storage
    // -----------------------------------------------------------------------
    let storage = Box::new(MemoryStorage::new(chain.clone()));
    let options = match chain {
        Chain::Main => ChaintracksOptions::default_mainnet(),
        Chain::Test => ChaintracksOptions::default_testnet(),
    };
    let ct = Chaintracks::new(options, storage);

    ct.make_available().await?;
    println!("Chaintracks storage initialised.");

    // -----------------------------------------------------------------------
    // 2. Fetch recent headers from WhatsOnChain
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
    // 3. Feed each header into chaintracks
    // -----------------------------------------------------------------------
    // Headers from WoC arrive newest-first; sort ascending by height so that
    // parent linkage works correctly during processing.
    let mut sorted_headers = headers.clone();
    sorted_headers.sort_by_key(|h| h.height);

    for header in &sorted_headers {
        let base = BaseBlockHeader {
            version: header.version,
            previous_hash: header.previous_hash.clone(),
            merkle_root: header.merkle_root.clone(),
            time: header.time,
            bits: header.bits,
            nonce: header.nonce,
        };
        ct.add_header(base).await?;
    }
    println!("Added {} headers to the pending queue.", sorted_headers.len());

    // -----------------------------------------------------------------------
    // 4. Process pending headers
    // -----------------------------------------------------------------------
    ct.process_pending_headers().await?;
    println!("Pending headers processed.");

    // -----------------------------------------------------------------------
    // 5. Print chain tip
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
    // 6. Print chaintracks info
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
