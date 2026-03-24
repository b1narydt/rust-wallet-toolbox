# Project Research Summary

**Project:** rust-wallet-toolbox — StorageClient milestone
**Domain:** Rust HTTP/JSON-RPC client implementing WalletStorageProvider for BSV wallet remote storage
**Researched:** 2026-03-24
**Confidence:** HIGH

## Executive Summary

This milestone adds `StorageClient<W>` to the Rust wallet-toolbox crate — a JSON-RPC client that speaks the same wire protocol as the TypeScript `StorageServer` and implements a new `WalletStorageProvider` trait (~25 methods). The key architectural insight is that the remote client cannot implement the full 60+ method `StorageProvider` interface used by local SQLite providers; instead a narrower `WalletStorageProvider` trait must be introduced and the `WalletStorageManager` backup slot type changed to match. This is the same design decision the TypeScript implementation makes, and the Rust codebase must follow it explicitly because Rust's type system enforces the boundary that TypeScript's structural typing lets slip through.

The implementation is straightforward in terms of dependencies — no new crates are needed. Every required technology (`reqwest` 0.12, `serde_json`, `tokio::sync::Mutex`, `async-trait`, `bsv-sdk 0.1.75` with `AuthFetch`) is already in `Cargo.toml`. The entire implementation is new source files under `src/storage/traits/` and `src/storage/remoting/`, plus a targeted change to `src/storage/manager.rs`. The primary technical complexity is in the cross-language wire format: binary fields must be number arrays (not base64), timestamps must include millisecond precision and a trailing "Z" suffix, and JSON-RPC parameters must be positional arrays. All of these deviate from Rust/serde defaults and require explicit attention.

The critical risks are the async mutex pattern (using `std::sync::Mutex` for `AuthFetch` deadlocks the Tokio executor), timestamp format asymmetry (the existing `serde_datetime::serialize` omits the "Z" suffix, causing TS servers in non-UTC timezones to misparse filter parameters), and method name accuracy (the TS server dispatches via method string; any mismatch returns a silent null or undefined-dereference error). All three can be caught early with targeted unit tests on the wire format before any live server integration.

## Key Findings

### Recommended Stack

No new crates are required for this milestone. The baseline stack already includes every dependency needed for `StorageClient`. The implementation uses `reqwest` 0.12 for HTTP POST, `serde_json` for the 4-field JSON-RPC 2.0 envelope (hand-rolled, not via a jsonrpc crate), `tokio::sync::Mutex` to wrap `AuthFetch<W>` (which requires `&mut self` for `fetch()`), and `bsv-sdk 0.1.75` for BRC-31 mutual authentication. Adding a JSON-RPC framework crate would be actively harmful — it would introduce server-routing machinery irrelevant to a pure client and risk mismatching the TS server's error envelope schema.

**Core technologies:**
- `reqwest` 0.12: HTTP POST transport — already used by ARC provider and WAB client, `json` feature already enabled
- `serde_json` 1: JSON-RPC envelope construction and response parsing — `Value` handles `params: Vec<Value>` cleanly
- `tokio::sync::Mutex`: wraps `AuthFetch<W>` for `Send + Sync` — required because `fetch()` is async and takes `&mut self`
- `bsv-sdk 0.1.75` (`AuthFetch<W>`): BRC-31 mutual auth — `W: WalletInterface + Clone + 'static` constraint propagates into `StorageClient<W>`
- `std::sync::atomic::AtomicU64`: JSON-RPC request ID counter — avoids holding the auth mutex for ID generation
- `async-trait` 0.1: `WalletStorageProvider` trait methods — same pattern used by all existing storage traits
- `thiserror` 2.0: `StorageClientError` variants — matches `WalletError` pattern throughout

### Expected Features

The `StorageClient` must fully implement the `WalletStorageProvider` trait hierarchy (WalletStorageReader + WalletStorageWriter + WalletStorageSync) before it can be plugged into `WalletStorageManager` as a backup provider. All ~25 methods are thin wrappers over the shared `rpc_call<T>` helper. The trait definition itself is a prerequisite that does not yet exist in the Rust codebase.

**Must have (table stakes):**
- `WalletStorageProvider` Rust trait definition (~25 methods with `async_trait`) — `WalletStorageManager` cannot hold `StorageClient` without this
- `StorageClient<W>` struct with `Mutex<AuthFetch<W>>`, endpoint URL, stored identity key, `AtomicU64` ID counter, and `Option<Settings>` cache
- `rpc_call<T>` helper: build JSON-RPC envelope, POST via `AuthFetch`, parse response, map `json.error` to `WalletError`
- `WalletErrorObject -> WalletError` conversion: parse `name` field against WERR codes — required by `rpc_call`
- All ~25 `WalletStorageProvider` methods forwarding to `rpc_call` with correct camelCase method name strings matching TS wire format exactly
- `make_available`, `is_available`, `get_settings` lifecycle methods — `WalletStorageManager` calls these before any sync operation
- Entity datetime handling on methods that return entity arrays (uses existing `serde_datetime` module; no new code beyond applying `#[serde(with)]` consistently)
- `isStorageProvider()` returning false — prevents `WalletStorageManager` from calling CRUD migration methods on a remote client

**Should have (competitive):**
- `AtomicU64` request ID counter — correctness under concurrent use; low cost to add from the start
- Lazy `make_available` on first use — reduces caller burden; matches `WABClient` pattern
- Integration tests against `storage.babbage.systems` with `#[ignore]` — validates wire-compatibility assumptions

**Defer (v2+):**
- Typed per-method request/response structs instead of positional `params` arrays — defer until the server protocol has stabilized
- Health-check / reconnect logic for long-lived backup providers — `AuthFetch` handles session reuse already

### Architecture Approach

The implementation requires four ordered steps: (1) define `WalletStorageProvider` trait at `src/storage/traits/wallet_storage.rs`; (2) add a blanket impl `impl<T: StorageProvider> WalletStorageProvider for T` so existing local providers satisfy the new trait without modification; (3) implement `StorageClient<W>` at `src/storage/remoting/storage_client.rs`; and (4) change `WalletStorageManager.backup` from `Option<Arc<dyn StorageProvider>>` to `Option<Arc<dyn WalletStorageProvider>>` and guard the existing CRUD write-propagation paths with an `is_storage_provider()` check. The sync protocol (getSyncChunk / processSyncChunk) replaces direct CRUD propagation for remote backups — this is eventually consistent replication, which matches the TS design.

**Major components:**
1. `WalletStorageProvider` trait (`src/storage/traits/wallet_storage.rs`) — new; 25-method interface for the backup slot; defines the contract StorageClient fulfills
2. `StorageClient<W>` (`src/storage/remoting/storage_client.rs`) — new; JSON-RPC pass-through client; `Mutex<AuthFetch<W>>` for BRC-31 auth; `rpc_call<T>` as the single network operation point
3. `WalletStorageManager` (`src/storage/manager.rs`) — modified; backup field type changed; CRUD propagation guarded by `is_storage_provider()` check; sync-chunk replication used for remote backups

### Critical Pitfalls

1. **`serde_datetime::serialize` omits trailing "Z"** — change format to `"%Y-%m-%dT%H:%M:%S%.3f"` and append "Z"; without it, TS Node.js parses timestamps as local time, shifting filter windows by the server's UTC offset; add a unit test asserting the serialized output ends in `.718Z`

2. **`std::sync::Mutex` around `AuthFetch` deadlocks Tokio** — use `tokio::sync::Mutex` from day one; `AuthFetch::fetch()` has multiple internal `.await` points (30-second handshake timeout loop); holding a sync mutex across those awaits blocks the executor thread

3. **JSON-RPC params must be a positional array, never a named object** — the TS server dispatches via `storage[method](...params)` spread; named params silently produce `undefined` positional arguments; the `rpc_call` helper must always emit `"params": [...]`

4. **Binary fields must serialize as number arrays, not base64** — serde's default `Vec<u8>` serialization is correct (emits JSON integer array matching TS `number[]`); do not add `serde_with::base64` or any `#[serde(with)]` base64 annotation to `raw_tx`, `merkle_path`, or `input_beef` fields; verify with a wire-format unit test before live server calls

5. **BRC-31 session stuck after partial handshake** — a failed handshake leaves a partially-initialized `AuthPeer` that subsequent calls attempt to reuse; implement `reset_session()` on `StorageClient` that clears the `AuthFetch` peer state on `AuthError`, and distinguish handshake errors from HTTP 4xx payload errors

## Implications for Roadmap

Based on the build-order dependency chain from ARCHITECTURE.md and the phase-to-pitfall mapping from PITFALLS.md, four phases are recommended:

### Phase 1: Wire-Format Foundation
**Rationale:** Binary field serialization and timestamp format are checked before any `StorageClient` code is written. Catching these early prevents "looks done but isn't" integration failures against live servers. These are cross-cutting concerns that affect every subsequent method implementation.
**Delivers:** Fixed `serde_datetime::serialize` with millisecond precision and trailing "Z"; unit tests asserting `Vec<u8>` serializes as integer array and `NaiveDateTime` serializes as `"...Z"`; a `ProvenTx` JSON fixture round-trip test
**Addresses:** Pitfalls 1, 4, 7 (binary fields, timestamp milliseconds, missing "Z" suffix)
**Avoids:** Silent wire-format bugs discovered only after live-server integration

### Phase 2: WalletStorageProvider Trait and Blanket Impl
**Rationale:** `StorageClient` cannot be implemented without the trait it satisfies. The blanket impl must exist before `WalletStorageManager` changes compile. No network code at all in this phase — pure type system work.
**Delivers:** `src/storage/traits/wallet_storage.rs` with all ~25 method signatures; blanket `impl<T: StorageProvider> WalletStorageProvider for T`; updated `storage/traits/mod.rs` exports
**Addresses:** `WalletStorageProvider` trait definition (P1 table-stakes feature); architecture pattern 1
**Avoids:** Pitfall 2 (AuthFetch generic constraint kills dyn trait — decided here: `StorageClient<W>` stays concrete generic)

### Phase 3: StorageClient Implementation
**Rationale:** Depends on Phase 2 for the trait and Phase 1 for correct serializers. This is the core implementation phase. The `rpc_call` helper is written first and tested against correct envelope shape before any of the 25 method stubs are added.
**Delivers:** `StorageClient<W>` struct; `rpc_call<T>` helper with `tokio::sync::Mutex<AuthFetch<W>>`; `WalletErrorObject -> WalletError` conversion; all ~25 `WalletStorageProvider` method stubs with correct camelCase wire method names; `reset_session()` for auth error recovery
**Uses:** `reqwest` 0.12, `serde_json`, `tokio::sync::Mutex`, `bsv-sdk AuthFetch<W>`, `AtomicU64`
**Avoids:** Pitfalls 3, 5, 6 (mutex deadlock, positional params, BRC-31 session reset)

### Phase 4: WalletStorageManager Integration and Live Validation
**Rationale:** Changes to `manager.rs` are isolated to the final phase to avoid churn while the trait and client are being built. Live server tests are `#[ignore]` gated and serve as the final acceptance criteria.
**Delivers:** `WalletStorageManager.backup` field type changed to `Option<Arc<dyn WalletStorageProvider>>`; `is_storage_provider()` guard around CRUD propagation paths; `#[ignore]` integration tests against `storage.babbage.systems`; documentation of `i64` precision limitation as accepted risk
**Implements:** Architecture pattern 3 (sync-chunk replication replaces CRUD propagation for remote backups)
**Avoids:** Anti-pattern 3 (removing direct write propagation before sync is wired)

### Phase Ordering Rationale

- Wire format must be correct before any storage method is written — format bugs are pervasive and expensive to fix after all 25 methods are implemented
- The trait must exist before the struct that implements it — Rust compilation enforces this ordering
- `WalletStorageManager` changes depend on both the trait (Phase 2) and the struct (Phase 3) being stable — touching the manager earlier causes churn
- Integration tests are last because they require a live BRC-31 identity key and network access; they validate everything but cannot drive development

### Research Flags

Phases with well-documented patterns (skip research-phase):
- **Phase 1:** `serde` serialization format changes are standard Rust; `chrono` format strings are well-documented
- **Phase 2:** Rust trait definition and blanket impls are core language features
- **Phase 3:** `reqwest` POST pattern already exists in codebase (`arc.rs` ARC provider); `tokio::sync::Mutex` usage is standard

Phases that may benefit from targeted research during planning:
- **Phase 3 (BRC-31 session management):** The `AuthFetch::fetch()` internals and peer-state reset path should be re-verified against the exact `bsv-sdk 0.1.75` source before implementing `reset_session()`; the API may not expose a direct "clear peer" method
- **Phase 4 (WalletStorageManager backup propagation):** The exact CRUD propagation call sites in `manager.rs` should be audited for completeness before adding the `is_storage_provider()` guard — missed call sites would silently fail to replicate to remote backups

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | All dependencies verified from `Cargo.toml` and `bsv-sdk-0.1.75` source; no new crates required; wire format confirmed from live TS source |
| Features | HIGH | Based on direct reading of `StorageClient.ts` (530 lines) and `WalletStorage.interfaces.ts`; method list and wire names verified |
| Architecture | HIGH | Verified from `src/storage/manager.rs` current backup field type, `StorageProvider` trait source, and TS `WalletStorageProvider` hierarchy |
| Pitfalls | HIGH | Timestamp and binary format pitfalls verified from live TS source and existing `serde_datetime.rs`; mutex deadlock from Tokio documentation |

**Overall confidence:** HIGH

### Gaps to Address

- **`reset_session()` implementation path:** `AuthFetch`'s internal `peers: HashMap<String, AuthPeer<W>>` field may not be `pub`. If `AuthFetch` does not expose a method to evict a peer, the reset strategy becomes "recreate the entire `AuthFetch` instance" — this changes the `StorageClient` API (need to hold the wallet `W` separately, not just pass it to `AuthFetch::new`). Verify field visibility in `bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` before implementing Phase 3.

- **Blanket impl feasibility:** `impl<T: StorageProvider> WalletStorageProvider for T` requires that every method on `StorageProvider` can be forwarded to the corresponding method on `WalletStorageProvider`. If any method signature differs between the TS-derived `WalletStorageProvider` and the existing Rust `StorageProvider`, the blanket impl will not compile and per-type impls will be needed for `SqlxProvider` and any other local provider. Audit the overlap before writing the blanket impl.

- **Method name for `findOutputBasketsAuth`:** FEATURES.md flags that the TS `StorageClient` method is named `findOutputBasketsAuth` but sends the RPC wire string `'findOutputBaskets'`. Verify this mismatch is intentional and that the server expects `findOutputBaskets` (not `findOutputBasketsAuth`) by checking `StorageServer.ts` dispatch table during Phase 3.

## Sources

### Primary (HIGH confidence)
- `/Users/elisjackson/Projects/metawatt-edge-rs/.ref/wallet-toolbox/src/storage/remoting/StorageClient.ts` — JSON-RPC envelope, method names, parameter ordering, error detection pattern
- `/Users/elisjackson/Projects/metawatt-edge-rs/.ref/wallet-toolbox/src/sdk/WalletStorage.interfaces.ts` — `WalletStorageProvider`, `WalletStorageReader`, `WalletStorageWriter`, `WalletStorageSync` hierarchy
- `~/.cargo/registry/src/.../bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` — `AuthFetch<W>::fetch()` signature, `&mut self` requirement, `W: WalletInterface + Clone + 'static` bound
- `src/storage/manager.rs` (lines 57-110) — current backup field type, existing write-propagation call sites
- `src/storage/traits/provider.rs` — `StorageProvider` trait method count and signatures
- `Cargo.toml` — confirmed all required dependencies already pinned

### Secondary (MEDIUM confidence)
- https://github.com/bsv-blockchain/wallet-toolbox/blob/master/src/storage/remoting/StorageServer.ts — TS server dispatch pattern (`storage[method](...params)`) confirming positional array requirement
- https://turso.tech/blog/how-to-deadlock-tokio-application-in-rust-with-just-a-single-mutex — Tokio mutex deadlock analysis
- https://www.jsonrpc.org/specification — positional vs named params in JSON-RPC 2.0

### Tertiary (LOW confidence)
- JavaScript `i64` precision limit (2^53 - 1) — accepted risk documentation; https://github.com/near/NEPs/issues/319

---
*Research completed: 2026-03-24*
*Ready for roadmap: yes*
