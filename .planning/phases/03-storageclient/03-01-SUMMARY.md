---
phase: 03-storageclient
plan: "01"
subsystem: storage/remoting
tags: [storage-client, json-rpc, auth-fetch, error-mapping, settings-cache]
dependency_graph:
  requires:
    - src/error.rs (WalletErrorObject)
    - src/storage/traits/wallet_provider.rs (WalletStorageProvider)
    - src/tables/settings.rs (Settings)
    - src/status.rs (ProvenTxReqStatus)
    - bsv-sdk-0.1.75 (AuthFetch, WalletInterface)
  provides:
    - src/storage/remoting/storage_client.rs (StorageClient<W>)
    - src/storage/remoting/mod.rs (module entry point)
    - src/error.rs (wallet_error_from_object)
  affects:
    - src/storage/mod.rs (added pub mod remoting, re-exports StorageClient)
tech_stack:
  added:
    - tokio::sync::Mutex (for AuthFetch and Settings cache)
    - std::sync::atomic (AtomicBool, AtomicU64)
    - serde_json::json! macro for envelope construction
  patterns:
    - JSON-RPC 2.0 envelope with positional params array and auto-incrementing id
    - Acquire/Release atomic pairing for sync is_available() check
    - Cache-then-fetch pattern in make_available (lock -> check -> fetch -> store)
key_files:
  created:
    - src/storage/remoting/mod.rs
    - src/storage/remoting/storage_client.rs
  modified:
    - src/storage/mod.rs
    - src/error.rs
decisions:
  - tokio::sync::Mutex for AuthFetch and settings cache (std::sync::Mutex deadlocks Tokio executor)
  - AtomicBool with Acquire/Release ordering for sync is_available() without blocking async context
  - wallet_error_from_object placed in error.rs (peer of to_wallet_error_object, symmetric conversion)
  - is_storage_provider() returns false to distinguish StorageClient from local SqliteStorage
  - todo!() stubs for Plan 02 methods compile cleanly giving Plan 02 a working foundation
metrics:
  duration_seconds: 200
  completed_date: "2026-03-24"
  tasks_completed: 1
  tasks_total: 1
  files_created: 2
  files_modified: 2
---

# Phase 03 Plan 01: StorageClient Foundation Summary

**One-liner:** `StorageClient<W>` with JSON-RPC 2.0 `rpc_call`, WERR→WalletError mapping, AtomicBool settings cache, and `UpdateProvenTxReqWithNewProvenTx` types.

## What Was Built

`StorageClient<W>` is the remote JSON-RPC implementation of `WalletStorageProvider`. It holds an `AuthFetch<W>` behind a `tokio::sync::Mutex` for BRC-31 authenticated HTTP calls, builds JSON-RPC 2.0 envelopes with auto-incrementing IDs, and maps WERR error code strings in JSON-RPC error responses back to the local `WalletError` enum via `wallet_error_from_object`.

Settings are fetched once via `makeAvailable` and cached in a `Mutex<Option<Settings>>`. An `AtomicBool` with Acquire/Release ordering gives `is_available()` a lock-free sync path without needing to enter an async context.

The `WalletStorageProvider` impl includes the four core methods (`is_storage_provider`, `is_available`, `get_settings`, `make_available`) fully implemented. All 20 remaining methods have `todo!("Plan 02: ...")` stubs so the file compiles cleanly and Plan 02 has a well-defined foundation.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | StorageClient struct, rpc_call, error mapping, module wiring | 9a20b78 | src/storage/remoting/mod.rs, src/storage/remoting/storage_client.rs, src/storage/mod.rs, src/error.rs |

## Verification Results

- `cargo build` — succeeded, 0 errors
- `cargo test --lib storage::remoting` — 4/4 tests passed
  - `test_rpc_envelope` — JSON-RPC 2.0 envelope structure verified
  - `test_error_mapping` — all 12 WERR codes mapped to correct WalletError variants
  - `test_settings_cache_atomic` — AtomicBool Acquire/Release semantics verified
  - `test_update_proven_tx_req_serialization` — camelCase serde output verified
- `cargo test --features sqlite` — full suite 213 tests passed, 0 failed

## Deviations from Plan

None — plan executed exactly as written.

## Decisions Made

1. **tokio::sync::Mutex throughout** — AuthFetch and settings cache both use tokio's Mutex. std::sync::Mutex held across .await points would deadlock the Tokio executor (confirmed in STATE.md pre-phase decision).

2. **wallet_error_from_object in error.rs** — Placed alongside `to_wallet_error_object` (the inverse direction) for symmetric placement and natural discoverability.

3. **AtomicBool with Acquire/Release** — `is_available()` is a sync fn on the trait. Using an AtomicBool avoids introducing an async version or a blocking lock just for a boolean check.

4. **todo!() stubs compile cleanly** — The plan required stubs for Plan 02 methods. Each `todo!()` carries a `"Plan 02: method_name"` message so errors are self-documenting if hit at runtime before Plan 02 is complete.
