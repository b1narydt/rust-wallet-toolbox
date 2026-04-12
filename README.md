[![Crates.io](https://img.shields.io/crates/v/bsv-wallet-toolbox.svg)](https://crates.io/crates/bsv-wallet-toolbox)
[![Documentation](https://docs.rs/bsv-wallet-toolbox/badge.svg)](https://docs.rs/bsv-wallet-toolbox)
[![License](https://img.shields.io/badge/license-Open%20BSV-blue.svg)](./LICENSE)

# BSV Wallet Toolbox

BSV wallet implementation in Rust, translating the TypeScript wallet-toolbox into idiomatic Rust.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [Feature Flags](#feature-flags)
- [Subsystems](#subsystems)
  - [Storage](#storage)
  - [Services](#services)
  - [Signer](#signer)
  - [Wallet](#wallet)
  - [Monitor](#monitor)
  - [Authentication](#authentication)
  - [Permissions](#permissions)
- [TypeScript to Rust Mapping](#typescript-to-rust-mapping)
- [Rust Tips for TypeScript Developers](#rust-tips-for-typescript-developers)
- [API Documentation](#api-documentation)
- [Contributing](#contributing)
- [License](#license)

## Overview

`bsv-wallet-toolbox` is a complete BSV wallet crate that provides persistent storage, automatic
transaction broadcasting, proof collection, and chain monitoring out of the box. Import a single
crate, configure a `WalletBuilder`, and you have a fully functional wallet ready for production use.

The crate implements the [BRC-100 wallet standard](https://github.com/bitcoin-sv/BRCs/blob/master/wallet/0100.md),
exposing all 29 `WalletInterface` methods for creating actions, managing outputs, handling
certificates, and more. Under the hood it builds on the
[bsv-sdk](https://crates.io/crates/bsv-sdk) for BRC-42/43 key derivation and transaction
primitives.

The primary audience is BSV developers -- especially those migrating from the TypeScript
wallet-toolbox. The Rust crate mirrors the TypeScript architecture closely, with the same
subsystem boundaries (Storage, Services, Signer, Wallet, Monitor, Auth, Permissions) translated
into idiomatic Rust traits and structs. See the [mapping table](#typescript-to-rust-mapping)
below for a quick orientation.

## Architecture

```
Application
    |
    v
+-------------- Wallet (29 WalletInterface methods) ---------------+
|                                                                   |
|  +-- Signer -----+  +-- Storage ------+  +-- Services ---------+ |
|  | BRC-29 keys   |  | SQLite / MySQL  |  | ARC, WhatsOnChain,  | |
|  | Tx building   |  | / PostgreSQL    |  | Bitails, Chaintracks| |
|  +---------------+  +----------------+  +---------------------+ |
|                                                                   |
|  +-- Chaintracks -+  +-- Monitor ------+  +-- Auth -----------+ |
|  | Block headers  |  | 15 bg tasks    |  | WAB client        | |
|  | SPV validation |  | Proofs, sync   |  | Shamir shares     | |
|  | Reorg detect   |  +----------------+  +-------------------+ |
|  +---------------+                                              |
|                      +-- Permissions --+                        |
|                      | Token-based     |                        |
|                      | access control  |                        |
|                      +----------------+                        |
+-------------------------------------------------------------------+
    |
    v
  bsv-sdk (BRC-42/43 key derivation, transaction primitives)
```

## Quick Start

Add the dependency with `cargo add` (latest version shown in the Crates.io badge above):

```sh
cargo add bsv-wallet-toolbox
cargo add tokio --features rt-multi-thread,macros
```

Build and use a wallet:

```rust,no_run
use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::interfaces::WalletInterface;
use bsv_wallet_toolbox::types::Chain;
use bsv_wallet_toolbox::{WalletBuilder, WalletError};

#[tokio::main]
async fn main() -> Result<(), WalletError> {
    // Build a wallet with SQLite storage and default network services.
    // WalletBuilder provides a fluent API for configuring all subsystems.
    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(PrivateKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        ).unwrap())
        .with_sqlite("wallet.db")
        .with_default_services()
        .with_monitor()
        .build()
        .await?;

    // The wallet is ready. Call any of the 29 WalletInterface methods.
    // For example, check if the wallet is authenticated:
    let auth = setup.wallet.is_authenticated(
        bsv::wallet::interfaces::IsAuthenticatedArgs {},
        None,
    ).await.map_err(|e| WalletError::Internal(e.to_string()))?;

    println!("Authenticated: {}", auth.authenticated);

    // Start the background monitor for proof collection and chain sync.
    if let Some(monitor) = &setup.monitor {
        monitor.start_tasks().await?;
    }

    Ok(())
}
```

## Feature Flags

The crate uses feature flags to select the database backend. SQLite is enabled by default.
MySQL and PostgreSQL are opt-in and require disabling default features.

**SQLite (default)** -- no external server required, data stored in a local file:

```sh
cargo add bsv-wallet-toolbox
```

**MySQL** -- requires a running MySQL server and connection URL:

```sh
cargo add bsv-wallet-toolbox --no-default-features --features mysql
```

**PostgreSQL** -- requires a running PostgreSQL server and connection URL:

```sh
cargo add bsv-wallet-toolbox --no-default-features --features postgres
```

**Chaintracks WebSocket** -- enables the real-time WebSocket live ingestor for local block
header tracking (adds `tokio-tungstenite` and `url` dependencies):

```sh
cargo add bsv-wallet-toolbox --features chaintracks-ws
```

When using MySQL or PostgreSQL, pass the connection URL to the builder:

```rust,no_run
# use bsv_wallet_toolbox::WalletBuilder;
let builder = WalletBuilder::new()
    .with_mysql("mysql://user:pass@localhost/wallet_db");
    // or
    // .with_postgres("postgres://user:pass@localhost/wallet_db");
```

## Subsystems

### Storage

The storage layer is built on a three-level trait hierarchy: `StorageReader` (read-only queries),
`StorageReaderWriter` (reads plus writes and transactions), and `StorageProvider` (full provider
with migrations and sync). `StorageSqlx` implements these traits for all three database backends
via sqlx. `WalletStorageManager` wraps the active storage provider and an optional backup for
redundancy.

Rust tip: `StorageProvider` is a trait, equivalent to a TypeScript abstract class. Implement it
to plug in a custom storage backend.

```rust,no_run
# use bsv_wallet_toolbox::WalletBuilder;
// SQLite in-memory storage (useful for testing)
let builder = WalletBuilder::new()
    .with_sqlite_memory();

// SQLite file-backed storage
let builder = WalletBuilder::new()
    .with_sqlite("wallet.db");
```

For production deployments with multiple replicas (e.g., Railway), tune the connection pool:

```rust,no_run
# use bsv_wallet_toolbox::WalletBuilder;
# use std::time::Duration;
let builder = WalletBuilder::new()
    .with_mysql("mysql://user:pass@host/db")
    .with_max_connections(20)     // default: 50
    .with_min_connections(5)      // default: 2
    .with_pool_idle_timeout(Duration::from_secs(300))   // default: 600s
    .with_pool_connect_timeout(Duration::from_secs(10)); // default: 5s
```

### Chaintracks

The `chaintracks` module provides local block header management with SPV proof validation and
chain reorganization detection. It replaces the sole dependency on the remote Babbage Chaintracks
service with an embedded alternative that can sync headers from CDN bulk downloads, WhatsOnChain
REST/WebSocket feeds, or any custom source via the `BulkIngestor` / `LiveIngestor` traits.

Two storage backends are included: `MemoryStorage` (fast, ephemeral) and `SqliteStorage`
(persistent). The `Chaintracks` orchestrator runs a background sync task, processes incoming
headers, detects reorgs, and notifies subscribers.

By default the wallet still uses the remote Chaintracks service. To opt into local mode:

```rust,no_run
# use bsv_wallet_toolbox::chaintracks::*;
# use bsv_wallet_toolbox::types::Chain;
# async fn example() -> Result<(), bsv_wallet_toolbox::WalletError> {
let storage = Box::new(MemoryStorage::new(Chain::Test));
let options = ChaintracksOptions::default_testnet();
let ct = Chaintracks::new(options, storage);
ct.make_available().await?;
ct.start_background_sync().await?;
# Ok(())
# }
```

### Services

The `WalletServices` trait defines the interface for network service providers. The default
`ServiceCollection` bundles ARC (transaction broadcasting), WhatsOnChain (UTXO and script
history), Bitails (merkle proofs), and Chaintracks (chain headers) with round-robin failover
across multiple provider instances.

### Signer

`DefaultWalletSigner` handles BRC-29 key derivation, UTXO selection, and BEEF (Background
Evaluation Extended Format) transaction construction. It derives protocol-specific keys from the
wallet root key and builds fully signed transactions ready for broadcast.

### Wallet

The `Wallet` struct implements all 29 `WalletInterface` methods defined by the BRC-100 standard:
creating actions, listing outputs, managing certificates, acquiring identities, and more through
a single unified API.

```rust,no_run
use bsv::wallet::interfaces::{ListOutputsArgs, WalletInterface};
# use bsv_wallet_toolbox::WalletError;
# async fn example(wallet: &impl WalletInterface) -> Result<(), WalletError> {
let outputs = wallet.list_outputs(
    ListOutputsArgs {
        basket: "default".to_string(),
        ..Default::default()
    },
    None,
).await.map_err(|e| WalletError::Internal(e.to_string()))?;
println!("Found {} outputs", outputs.outputs.len());
# Ok(())
# }
```

### Monitor

The `Monitor` runs 15 background tasks that handle proof collection, chain synchronization,
ARC SSE event processing, and periodic health checks. Tasks run in a polling loop managed by
tokio, with configurable intervals and automatic retry.

```rust,no_run
# use bsv_wallet_toolbox::{WalletBuilder, WalletError};
# use bsv_wallet_toolbox::types::Chain;
# use bsv::primitives::private_key::PrivateKey;
# async fn example() -> Result<(), WalletError> {
let setup = WalletBuilder::new()
    .chain(Chain::Test)
    .root_key(PrivateKey::from_random()?)
    .with_sqlite_memory()
    .with_default_services()
    .with_monitor()  // enables background proof collection, sync, and chain monitoring
    .build()
    .await?;

// Start all background tasks
if let Some(monitor) = &setup.monitor {
    monitor.start_tasks().await?;
}
# Ok(())
# }
```

### Authentication

`WalletAuthenticationManager` integrates with the Wallet Authentication Backend (WAB) for
identity verification. It supports pluggable auth interactors, Shamir secret sharing for key
recovery, and presentation key management through the `WABClient` HTTP client.

### Permissions

`WalletPermissionsManager` acts as a proxy in front of the wallet, enforcing permission checks
before forwarding calls to the underlying wallet. Permission tokens are stored as PushDrop UTXOs
on-chain, providing a cryptographically verifiable access control layer.

## TypeScript to Rust Mapping

If you are coming from the TypeScript wallet-toolbox, this table maps TS concepts to their
Rust equivalents.

| TypeScript | Rust | Notes |
|------------|------|-------|
| `StorageKnex` | `StorageSqlx` | sqlx replaces knex |
| `StorageProvider` (abstract class) | `StorageProvider` (trait) | Trait hierarchy: Reader / ReaderWriter / Provider |
| `WalletStorageManager` | `WalletStorageManager` | Active + optional backup |
| `Wallet` | `Wallet` | Same 29 methods |
| `WalletSigner` | `DefaultWalletSigner` | BRC-29 key derivation |
| `Services` | `Services` | Same provider pattern |
| `Monitor` | `Monitor` | Same task model |
| `WalletAuthenticationManager` | `WalletAuthenticationManager` | WAB integration |
| `WalletPermissionsManager` | `WalletPermissionsManager` | Proxy pattern |
| `Chaintracks` | `Chaintracks` | Local block header management |
| `ChaintracksServiceClient` | `ChaintracksServiceClient` | Remote HTTP client |
| `Setup` / `SetupClient` | `WalletBuilder` | Fluent builder pattern |
| `WalletError` (class) | `WalletError` (enum) | thiserror-based |
| `WERR_*` error codes | `WalletError::*` variants | Same error semantics |

## Rust Tips for TypeScript Developers

- **Traits vs abstract classes.** Rust traits are similar to TypeScript abstract classes or
  interfaces. When you see `dyn StorageProvider`, think of it as an instance of an abstract
  class. `impl StorageProvider` means "any concrete type implementing this trait."

- **`Result<T, E>` and the `?` operator vs try/catch.** Rust uses `Result` for error handling
  instead of exceptions. The `?` operator propagates errors automatically -- it is roughly
  equivalent to writing `try { ... }` with an implicit `throw` on failure.

- **`async`/`await` with tokio vs the Node.js event loop.** Rust async works similarly to
  TypeScript `async`/`await`, but requires an explicit runtime (tokio). Functions marked `async`
  return a `Future` that must be `.await`ed. The `#[tokio::main]` macro sets up the runtime.

- **Feature flags vs npm optional dependencies.** Cargo feature flags control compile-time
  inclusion of code. `default-features = false` is similar to not installing an optional npm
  package -- it excludes the default database backend so you can pick a different one.

- **`Arc<dyn Trait>` vs dependency injection.** Where TypeScript uses constructor injection with
  interface types, Rust uses `Arc<dyn Trait>` -- a reference-counted pointer to a trait object.
  This allows shared ownership of service implementations across async tasks.

## API Documentation

Full API documentation is available on [docs.rs](https://docs.rs/bsv-wallet-toolbox). All 659+
public items include doc comments with descriptions, parameter explanations, and cross-references.

Generate local documentation with:

```sh
cargo doc --open
```

## Contributing

1. Clone the repository and build:

   ```sh
   git clone https://github.com/bsv-blockchain/wallet-toolbox.git
   cd wallet-toolbox/rust-wallet-toolbox
   cargo build
   ```

2. Run tests:

   ```sh
   cargo test
   ```

3. Pull requests are welcome. Please include tests for new functionality and follow the
   existing code style. Run `cargo clippy` and `cargo fmt --check` before submitting.

## License

This project is licensed under the [Open BSV License](./LICENSE).

Copyright (c) 2023 BSV Blockchain Association. See the LICENSE file for full terms.
