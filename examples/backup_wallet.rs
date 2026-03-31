//! Backup Wallet Example
//!
//! Demonstrates building a wallet with SQLite storage and triggering a backup
//! sync via `update_backups`. The `WalletStorageManager` supports multiple
//! storage providers -- a primary (active) and one or more backup providers.
//!
//! This example uses a single-provider setup (no backup provider configured)
//! and shows how `update_backups` works. In production you would configure
//! additional backup providers via `WalletStorageManager::new()`.
//!
//! Reads configuration from `examples/.env` (created by `setup_wallet`).
//! You can also set env vars directly.
//!
//! # Usage
//!
//! ```bash
//! BSV_PRIVATE_KEY=<hex> cargo run --example backup_wallet
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
    // 1. Load private key and build wallet with SQLite storage
    // -----------------------------------------------------------------------
    let private_key = match std::env::var("BSV_PRIVATE_KEY") {
        Ok(hex) => PrivateKey::from_hex(&hex)?,
        Err(_) => {
            let key = PrivateKey::from_random()?;
            println!("No BSV_PRIVATE_KEY set; generated a new one.");
            println!("  BSV_PRIVATE_KEY={}", key.to_hex());
            println!("  Save it to re-open this wallet later.\n");
            key
        }
    };

    let db_path = "backup_example.db";
    println!("Building wallet with SQLite storage at '{}'...", db_path);

    let setup = WalletBuilder::new()
        .chain(chain.clone())
        .root_key(private_key)
        .with_sqlite(db_path)
        .with_default_services()
        .build()
        .await?;

    println!("Wallet ready.");
    println!("  Identity key: {}", setup.identity_key);

    let balance = setup.wallet.balance(None).await?;
    println!("  Balance:      {} satoshis", balance);

    // -----------------------------------------------------------------------
    // 2. Run update_backups on the storage manager
    // -----------------------------------------------------------------------
    // The `update_backups` method syncs data from the active storage provider
    // to all configured backup providers. With a single provider (no backups),
    // this completes quickly with zero inserts/updates.
    //
    // In a production setup with backup providers, this would:
    // - Acquire sync-level locks
    // - For each backup provider, run a sync loop transferring new/modified
    //   records (transactions, outputs, certificates, etc.)
    // - Return (total_inserts, total_updates, log)

    println!("\nRunning update_backups...");
    let (inserts, updates, log) = setup.storage.update_backups(None).await?;

    println!("  Inserts: {}", inserts);
    println!("  Updates: {}", updates);
    if !log.is_empty() {
        println!("  Log:\n{}", log);
    } else {
        println!("  Log: (empty -- no backup providers configured)");
    }

    // -----------------------------------------------------------------------
    // 3. Print database file info
    // -----------------------------------------------------------------------
    let db_file = std::path::Path::new(db_path);
    if db_file.exists() {
        let metadata = std::fs::metadata(db_file)?;
        println!("\nDatabase file: {}", db_path);
        println!("  Size: {} bytes ({:.1} KB)", metadata.len(), metadata.len() as f64 / 1024.0);
    }

    // -----------------------------------------------------------------------
    // 4. Print backup architecture overview
    // -----------------------------------------------------------------------
    println!("\n--- Backup Architecture ---");
    println!();
    println!("The WalletStorageManager supports multi-provider backup sync:");
    println!();
    println!("  // To set up with a backup provider, construct the manager manually:");
    println!("  //");
    println!("  // use bsv_wallet_toolbox::storage::manager::WalletStorageManager;");
    println!("  //");
    println!("  // let primary = Arc::new(SqliteStorage::new_sqlite(primary_config, chain).await?);");
    println!("  // let backup  = Arc::new(SqliteStorage::new_sqlite(backup_config, chain).await?);");
    println!("  //");
    println!("  // let manager = WalletStorageManager::new(");
    println!("  //     identity_key,");
    println!("  //     Some(primary),     // Active provider");
    println!("  //     vec![backup],      // Backup providers");
    println!("  // );");
    println!("  // manager.make_available().await?;");
    println!("  //");
    println!("  // // Sync active -> all backups:");
    println!("  // let (inserts, updates, log) = manager.update_backups(None).await?;");
    println!();
    println!("Backup providers can be:");
    println!("  - Another SQLite file (local backup)");
    println!("  - A MySQL or PostgreSQL database (remote backup)");
    println!("  - Any implementation of the WalletStorageProvider trait");

    Ok(())
}
