# State: rust-wallet-toolbox

## Current Position

Phase: Not started (defining requirements)
Plan: —
Status: Defining requirements
Last activity: 2026-03-24 — Milestone v1.0 started

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-24)

**Core value:** Wire-compatible remote storage that lets a Rust wallet sync with TypeScript storage servers
**Current focus:** StorageClient implementation

## Accumulated Context

- Codebase has full storage trait hierarchy (StorageReader → StorageReaderWriter → StorageProvider)
- AuthFetch in bsv-sdk 0.1.75 is generic over W: WalletInterface + Clone + 'static, takes &mut self
- TS StorageClient has ~25 methods, all one-liner pass-throughs to rpcCall
- TS wire format: JSON-RPC 2.0 with params as ordered arrays, method names in camelCase
- AuthId struct exists at src/wallet/types.rs with just identity_key field
- SyncChunk, ProcessSyncChunkResult, SyncMap already defined in src/storage/sync/
- Settings table struct has Serialize/Deserialize with serde_datetime handling
- src/storage/remoting/ directory needs to be created
- No existing test files for remote storage yet
