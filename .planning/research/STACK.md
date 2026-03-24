# Stack Research

**Domain:** Rust HTTP/JSON-RPC client transport for BSV wallet storage (StorageClient milestone)
**Researched:** 2026-03-24
**Confidence:** HIGH

## New Capabilities Required

This research covers only what the StorageClient milestone adds. The baseline stack
(sqlx, bsv-sdk, serde, tokio, reqwest, async-trait, chrono, tracing) is already
validated and pinned. No new crates are needed.

## Recommended Stack

### Core Technologies (existing, confirmed sufficient)

| Technology | Version | Purpose | Why Confirmed Sufficient |
|------------|---------|---------|--------------------------|
| `reqwest` | 0.12 (pinned) | HTTP POST for JSON-RPC calls | Already used by ARC provider and WAB client; `json` feature already enabled |
| `serde_json` | 1 (pinned) | JSON-RPC envelope serialization + response parsing | Already in Cargo.toml; `serde_json::Value` handles `params: Vec<Value>` cleanly |
| `serde` | 1 (pinned) | Struct serialization for request/response types | `rename_all = "camelCase"` already on all table structs; `Vec<u8>` serializes as `number[]` by default — matching TS wire format |
| `tokio::sync::Mutex` | tokio 1 (pinned) | Wrap `AuthFetch<W>` for `Send + Sync` | `AuthFetch::fetch()` takes `&mut self`; tokio Mutex already used in signer and monitor |
| `async-trait` | 0.1 (pinned) | `WalletStorageProvider` trait methods | All storage traits already use it |
| `bsv-sdk` | 0.1.75 (pinned) | `AuthFetch<W>` for BRC-31 mutual auth | `AuthFetch` lives in `bsv::auth::clients::AuthFetch`; network feature already enabled |
| `thiserror` | 2.0 (pinned) | `StorageClientError` variants | Matches `WalletError` pattern used throughout |

### No New Crates Required

**JSON-RPC crate (e.g., `jsonrpc-core`, `jsonrpc-v2`):** NOT needed. The TS
`StorageClient.rpcCall()` constructs a hand-rolled JSON-RPC 2.0 envelope:

```json
{ "jsonrpc": "2.0", "method": "<name>", "params": [arg0, arg1], "id": <n> }
```

This is a 4-field struct trivially constructed with `serde_json`. A full JSON-RPC
crate adds transports, routing, and proc-macros that are all irrelevant — the server
dispatch is the TS `StorageServer`, not a Rust jsonrpc crate. Using one would
couple the client to crate-specific error envelope formats that may not match the TS
server response schema.

**`base64` for bytes:** NOT needed for wire format. TS `TableProvenTx.merklePath`
and `rawTx` are `number[]` — JSON arrays of byte values. Serde's default `Vec<u8>`
serialization is a JSON integer array, which matches. No re-encoding needed for the
outbound direction. For inbound responses from the TS server, the same `Vec<u8>`
deserialization handles `[1, 2, 3]` arrays automatically. (`base64` is already in
Cargo.toml at 0.22 for other uses.)

**`mockito` or HTTP mock crate:** NOT needed. Test strategy targets live TS servers
(`storage.babbage.systems`, `storage.b1nary.tech`) via `#[ignore]` integration tests
plus unit tests for the JSON-RPC envelope construction and response parsing. No local
HTTP mock layer needed.

## Integration Points

### AuthFetch<W> — Wire Into StorageClient

`AuthFetch<W: WalletInterface + Clone + 'static>` requires `&mut self` on `fetch()`.
`StorageClient` must be `Arc`-safe. The correct approach:

```rust
pub struct StorageClient<W: WalletInterface + Clone + Send + 'static> {
    endpoint_url: String,
    auth: tokio::sync::Mutex<AuthFetch<W>>,
    next_id: std::sync::atomic::AtomicU64,
}
```

- `tokio::sync::Mutex` (not `std::sync::Mutex`) because `AuthFetch::fetch()` is async
- `AtomicU64` for the JSON-RPC request counter avoids taking the auth lock for ID allocation
- `W` bound must include `Send` for `Arc<StorageClient<W>>` to be `Send + Sync`

The `next_id` in TS is a simple incrementing integer. `AtomicU64::fetch_add` with
`Ordering::Relaxed` is correct — ordering relative to the auth lock is not needed.

### `rpc_call<T>` helper

The private `rpc_call` method builds the envelope, calls `auth.fetch()`, parses the
response body as JSON, and extracts `result` or returns an error for `error` field.
The return type `T: DeserializeOwned` covers all response shapes.

```rust
async fn rpc_call<T: serde::de::DeserializeOwned>(
    &self,
    method: &str,
    params: Vec<serde_json::Value>,
) -> WalletResult<T>
```

`params` is `Vec<serde_json::Value>` — each argument is serialized to `Value` via
`serde_json::to_value()` at the call site. This is cleaner than boxing types and
matches the TS `params: unknown[]` exactly.

### WalletStorageProvider Trait (new, ~25 methods)

The `StorageClient` implements `WalletStorageProvider`, not `StorageProvider`.
`WalletStorageProvider` does not exist yet in Rust — it needs to be defined at
`src/storage/traits/wallet_storage_provider.rs` before `StorageClient` can be
written. The method set matches the TS `WalletStorageProvider` interface exactly
(mirroring the `StorageClient.ts` method list).

### Bytes on the Wire

TS `TableProvenTx.merklePath` and `rawTx` are `number[]`. Rust `Vec<u8>` serializes
to a JSON integer array by default, which is compatible. No `#[serde(with = ...)]`
annotation is needed for binary fields in the remote transport direction. The
existing `serde_datetime` helper handles timestamp fields exactly as before.

### Testing Against Live TS Servers

`reqwest` tests marked `#[ignore]` run against `storage.babbage.systems`. The
`StorageClient<W>` is generic over `W: WalletInterface`, so a real
`ProtoWallet` / `WalletClient` from `bsv-sdk` can be constructed in integration
tests. No mock server needed for initial validation.

## Alternatives Considered

| Recommended | Alternative | Why Not |
|-------------|-------------|---------|
| Hand-rolled JSON-RPC envelope | `jsonrpc-core` crate | Brings server routing, transports, and proc-macros irrelevant to a pure client; risk of mismatching TS error envelope schema |
| `tokio::sync::Mutex<AuthFetch<W>>` | `Arc<Mutex<AuthFetch<W>>>` (std) | `AuthFetch::fetch()` is async — `std::sync::Mutex` would block the executor; tokio Mutex required |
| `Vec<serde_json::Value>` for params | Generic param tuple | Simpler, directly maps to TS `unknown[]`, avoids lifetime complexity |
| Live server `#[ignore]` tests | `mockito` / `wiremock` | Mock servers cannot validate BRC-31 auth handshake; live interop is the actual goal |
| `AtomicU64` for request ID | mutex-guarded `u64` | No need to hold auth lock for ID increment; atomic is sufficient |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `jsonrpc-core` / `jsonrpc-v2` / `jsonrpc-lite` | All assume symmetric server setup or have their own error envelope schemas that may not match TS `StorageServer` | Hand-rolled 4-field struct via `serde_json` |
| `std::sync::Mutex` wrapping `AuthFetch` | `AuthFetch::fetch()` is async; holding a sync mutex across `.await` will panic in async context | `tokio::sync::Mutex` |
| `serde_bytes` or base64 encoding for `Vec<u8>` | TS wire format is `number[]`, not base64 or hex; serde default matches | Default `Vec<u8>` serialization |
| New HTTP client (`hyper` direct, `ureq`) | `reqwest` 0.12 is already pinned and used; adding a second client wastes compile time | Reuse existing `reqwest` |

## Stack Patterns by Variant

**If running integration tests against live servers:**
- Use `#[cfg(test)] #[ignore]` gates on tests requiring network access
- Pass `STORAGE_ENDPOINT`, `IDENTITY_KEY_HEX` (or WIF) as env vars
- Keep unit tests (envelope construction, response parsing) without `#[ignore]`

**If `StorageClient` needs to be used as a backup in `WalletStorageManager`:**
- `WalletStorageManager.backup` currently holds `Arc<dyn StorageProvider>`
- `StorageClient` intentionally does NOT implement `StorageProvider` (matches TS design)
- A thin adapter or a second optional field for `Arc<dyn WalletStorageProvider>` is
  needed in `WalletStorageManager` — this is an architecture decision for the
  milestone, not a library decision

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| `bsv-sdk 0.1.75` network feature | `reqwest 0.12` | Both are already in Cargo.toml; bsv-sdk re-exports reqwest 0.12 internally |
| `tokio 1` | `async-trait 0.1` | No changes needed |
| `serde 1` + `serde_json 1` | All existing table structs | `camelCase` rename and datetime helpers carry over unchanged |

## Cargo.toml Changes

No new entries required. All dependencies are already present:

```toml
# Already present — no additions needed for StorageClient
bsv-sdk = { version = "0.1.75", features = ["network"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
async-trait = "0.1"
thiserror = "2.0"
```

The only code additions are new source files under `src/storage/remoting/`.

## Sources

- Inspected `bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` — `AuthFetch<W>::fetch()` signature, `&mut self` requirement, `W: WalletInterface + Clone + 'static` bound
- Inspected `src/services/providers/arc.rs` — existing `reqwest::Client` usage pattern, established HTTP provider conventions
- Inspected `@bsv/wallet-toolbox/src/storage/remoting/StorageClient.ts` — exact JSON-RPC 2.0 envelope, `rpcCall<T>` pattern, parameter ordering for all ~25 methods
- Inspected `@bsv/wallet-toolbox/src/storage/schema/tables/TableProvenTx.ts` and `TableTransaction.ts` — binary fields are `number[]` confirming `Vec<u8>` serde default is wire-compatible
- Inspected `src/serde_datetime.rs` — existing datetime helper, no changes needed for remote path
- Inspected `Cargo.toml` — confirmed all needed dependencies already pinned

---
*Stack research for: StorageClient HTTP/JSON-RPC transport with BRC-31 auth*
*Researched: 2026-03-24*
