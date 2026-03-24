# rust-wallet-toolbox

## What This Is

A production-ready Rust implementation of the BSV wallet-toolbox, providing persistent wallet storage with SQLite/MySQL/PostgreSQL backends, transaction lifecycle management, incremental sync between storage providers, and background monitoring. It is the Rust equivalent of the TypeScript `@bsv/wallet-toolbox` package and targets the same wire protocols for cross-language interoperability.

## Core Value

Wire-compatible remote storage that lets a Rust wallet sync with TypeScript storage servers (storage.babbage.systems, storage.b1nary.tech) — enabling the full BSV wallet ecosystem to work across language boundaries.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. -->

- StorageReader trait: 16 find + 16 count methods across all table types
- StorageReaderWriter trait: insert/update for all 16 tables, transaction management, find-or-insert helpers
- StorageProvider trait: migration, settings, lifecycle control
- StorageActionProvider trait: create_action, process_action, internalize_action
- SQLx implementation: SQLite backend with dual read/write pool, full trait implementation
- WalletStorageManager: active + optional backup provider coordination, write propagation
- Sync protocol: getSyncChunk / processSyncChunk with incremental entity merge, ID remapping, conflict resolution
- Table structs: all 16 entity types with Serialize/Deserialize, serde_datetime handling
- Signer pipeline: BRC-29 key derivation, transaction building, signing
- Monitor: background tasks for proofs, broadcasting, chain state
- AuthFetch: BRC-31 mutual authentication HTTP client (in bsv-rust-sdk dependency)

### Active

<!-- Current scope. Building toward these. -->

- [ ] StorageClient: HTTP/JSON-RPC transport implementing WalletStorageProvider over BRC-31 auth
- [ ] Wire compatibility with TypeScript StorageServer (storage.babbage.systems)
- [ ] Integration as backup provider in WalletStorageManager
- [ ] End-to-end payment receive flow via remote storage

### Out of Scope

- StorageServer (Rust HTTP server) — exists separately at rust-wallet-storage repo
- MySQL/PostgreSQL backends — SQLite is sufficient for this milestone
- Browser/IndexedDB storage — Rust targets server/CLI environments
- New wallet features beyond what exists in the TS wallet-toolbox

## Context

- **Upstream dependency**: metawatt-edge-rs Phase 08.1 needs StorageClient to complete BRC-100 WalletProvider with remote storage
- **Reference implementations**: TS StorageClient at wallet-toolbox/src/storage/remoting/StorageClient.ts, Rust storage server dispatch at rust-wallet-storage/src/dispatch.rs
- **Auth middleware**: https://github.com/b1narydt/auth-actix-middleware for server-side BRC-31 auth
- **Live servers**: storage.babbage.systems (TS) and storage.b1nary.tech (TS) are the production interop targets
- **Repo relationships**: b1narydt repos are originals, bsv-blockchain org repos are forks (identical)

## Constraints

- **Wire format**: JSON-RPC method names, parameter ordering, and serialization must exactly match TS StorageClient wire format
- **BRC-31 auth**: Must use AuthFetch from bsv-sdk 0.1.75 (already pinned in Cargo.toml)
- **No new deps**: Pin to same bsv-sdk version the toolbox already uses
- **Code style**: Match existing crate patterns — module docs, API docs, no comment spam
- **PR target**: b1narydt/rust-wallet-toolbox, must match their style and conventions

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| StorageClient does NOT implement StorageProvider trait | Matches TS pattern: WalletStorageProvider is a higher-level ~25-method interface, not the 60+ method StorageProvider | — Pending |
| AuthFetch behind Mutex for Send+Sync | AuthFetch requires &mut self for fetch(); StorageClient needs to be Arc-safe | — Pending |
| Test against live TS servers first | Production interop is the primary goal; Rust server testing is secondary | — Pending |

---
*Last updated: 2026-03-24 after milestone v1.0 initialization*
