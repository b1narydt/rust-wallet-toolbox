# Phase 3: StorageClient - Research

**Researched:** 2026-03-24
**Domain:** Rust async HTTP client, JSON-RPC 2.0 envelope, BRC-31 authenticated transport, WalletStorageProvider trait impl
**Confidence:** HIGH

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| CLIENT-01 | StorageClient struct with rpc_call helper sends JSON-RPC 2.0 requests (positional params array, auto-incrementing ID) | JSON-RPC 2.0 envelope structure confirmed from TS source; id is a simple counter |
| CLIENT-02 | AuthFetch<W> held behind tokio::sync::Mutex for Send+Sync safety in async context | AuthFetch::new(wallet) API confirmed from bsv-sdk-0.1.75 source; fetch() takes &mut self, needs Mutex |
| CLIENT-03 | All ~25 WalletStorageProvider methods implemented as pass-throughs to rpc_call, matching TS method signatures | All 23 wire method names confirmed from TS StorageClient.ts |
| CLIENT-04 | JSON-RPC error responses map back to typed WalletError variants via WalletErrorObject deserialization | WalletErrorObject struct already exists in error.rs with all WERR fields; just need from-json mapping |
| CLIENT-05 | Constructor accepts WalletInterface impl for BRC-31 auth signing | AuthFetch::new(wallet: W) confirmed; W: WalletInterface + Clone + 'static |
| CLIENT-06 | Wire method names exactly match TS StorageClient | Full method-to-wire-name table documented below |
| CLIENT-07 | updateProvenTxReqWithNewProvenTx as extra method, settings cached after makeAvailable | isAvailable = !!settings; getSettings errors if settings None; makeAvailable caches on first call |
</phase_requirements>

---

## Summary

StorageClient is a thin JSON-RPC 2.0 client that wraps `AuthFetch<W>` behind a `tokio::sync::Mutex`, holds the endpoint URL, and exposes every `WalletStorageProvider` method as a one-liner that serializes arguments into `params: [...]` and deserializes the response. The struct is generic over `W: WalletInterface + Clone + Send + 'static`.

The TypeScript reference implementation (`StorageClient.ts`) has been fully analyzed. All 23 wire method names and their params arrays are confirmed. Settings caching is straightforward: a `settings: Option<Settings>` field on the struct initialized to `None`; `is_available()` returns `settings.is_some()`; `make_available()` calls `rpc_call` once and stores the result; `get_settings()` returns the cached value or errors.

`update_proven_tx_req_with_new_proven_tx` is NOT part of `WalletStorageProvider`. It is an inherent method on `StorageClient` only, matching the TS pattern where `StorageClient` exposes it directly without it appearing on the interface.

**Primary recommendation:** Implement `src/storage/remoting/storage_client.rs` as `StorageClient<W>` with `AuthFetch<W>` behind `Mutex`, a single `rpc_call` private helper, and 23 delegating trait methods plus 1 inherent method. Add `mod remoting` to `storage/mod.rs`.

---

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| bsv-sdk | 0.1.75 | AuthFetch, WalletInterface, AuthError | Already a Cargo.toml dependency with `network` feature |
| tokio::sync::Mutex | (tokio 1.x) | Wrap AuthFetch for async Send+Sync | std::Mutex deadlocks Tokio executor; tokio::sync::Mutex is the correct choice |
| serde_json | 1 | JSON-RPC envelope serialization / response deserialization | Already a dependency |
| async-trait | 0.1 | WalletStorageProvider trait impl | Already used in the trait definition |
| std::sync::atomic::AtomicU64 | stdlib | Auto-incrementing JSON-RPC id | Thread-safe without Mutex; appropriate for concurrent rpc_call |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| serde (Serialize/Deserialize) | 1 | camelCase request/response structs | All params sent to server are serialized structs |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| AtomicU64 for id | Mutex<u64> | AtomicU64 avoids a lock acquisition per call; simpler |
| tokio::sync::Mutex | Arc<std::sync::Mutex> | std::Mutex panics inside .await; tokio Mutex is mandatory |

**Installation:** All dependencies already present in Cargo.toml.

---

## Architecture Patterns

### Recommended Project Structure

```
src/storage/remoting/
├── mod.rs            # pub mod storage_client; pub use storage_client::StorageClient;
└── storage_client.rs # StorageClient<W> struct + impl WalletStorageProvider + inherent methods
```

Add to `src/storage/mod.rs`:
```rust
pub mod remoting;
pub use remoting::StorageClient;
```

### Pattern 1: StorageClient Struct Layout

**What:** Generic struct with a Tokio Mutex-wrapped AuthFetch, endpoint URL, atomic ID counter, and cached settings.

**When to use:** The only pattern that satisfies Send+Sync for async trait objects (`Arc<dyn WalletStorageProvider + Send + Sync>`).

```rust
// Source: bsv-sdk-0.1.75 src/auth/clients/auth_fetch.rs + Cargo.toml Tokio dep
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use bsv::auth::clients::auth_fetch::AuthFetch;
use bsv::wallet::interfaces::WalletInterface;
use crate::tables::Settings;

pub struct StorageClient<W: WalletInterface + Clone + Send + 'static> {
    auth_fetch: Mutex<AuthFetch<W>>,
    endpoint_url: String,
    next_id: AtomicU64,
    settings: tokio::sync::Mutex<Option<Settings>>,
}

impl<W: WalletInterface + Clone + Send + 'static> StorageClient<W> {
    pub fn new(wallet: W, endpoint_url: impl Into<String>) -> Self {
        Self {
            auth_fetch: Mutex::new(AuthFetch::new(wallet)),
            endpoint_url: endpoint_url.into(),
            next_id: AtomicU64::new(1),
            settings: Mutex::new(None),
        }
    }
}
```

### Pattern 2: rpc_call Helper

**What:** Private async method that builds the JSON-RPC 2.0 envelope, POSTs via AuthFetch, and deserializes the response or converts `json.error` into a `WalletError`.

**When to use:** Called by every trait method (23 total) and the one inherent method.

```rust
// Source: TS StorageClient.ts rpcCall pattern
use serde_json::{json, Value};

impl<W: WalletInterface + Clone + Send + 'static> StorageClient<W> {
    async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> crate::error::WalletResult<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id
        });
        let body_bytes = serde_json::to_vec(&body)?;

        let mut headers = std::collections::HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());

        let response = {
            let mut fetch = self.auth_fetch.lock().await;
            fetch.fetch(
                &self.endpoint_url,
                "POST",
                Some(body_bytes),
                Some(headers),
            ).await.map_err(|e| WalletError::Internal(format!("auth fetch: {}", e)))?
        };

        if response.status >= 400 {
            return Err(WalletError::Internal(
                format!("HTTP {} from {}", response.status, self.endpoint_url)
            ));
        }

        let json: Value = serde_json::from_slice(&response.body)?;

        if let Some(err_obj) = json.get("error") {
            let werr: crate::error::WalletErrorObject =
                serde_json::from_value(err_obj.clone())?;
            return Err(wallet_error_from_object(werr));
        }

        let result = json.get("result")
            .ok_or_else(|| WalletError::Internal("JSON-RPC response missing 'result'".into()))?;
        Ok(serde_json::from_value(result.clone())?)
    }
}
```

### Pattern 3: WalletStorageProvider impl — one-liner delegates

**What:** Each trait method serializes its arguments as `serde_json::to_value(...)` and calls `rpc_call`.

**When to use:** All 23 WalletStorageProvider methods.

```rust
// Source: TS StorageClient.ts — all methods are one-liner rpcCall pass-throughs
#[async_trait]
impl<W: WalletInterface + Clone + Send + Sync + 'static> WalletStorageProvider for StorageClient<W> {
    fn is_storage_provider(&self) -> bool {
        false  // Remote client, not a local StorageProvider
    }

    fn is_available(&self) -> bool {
        // Sync: must try_lock or use blocking_lock.
        // Store settings in std::sync::Mutex (or use once_cell) for sync is_available.
        // See Pitfall 1 for details.
        self.settings_available.load(Ordering::Relaxed)
    }

    async fn get_settings(&self) -> WalletResult<Settings> {
        let guard = self.settings.lock().await;
        guard.clone().ok_or_else(|| WalletError::InvalidOperation(
            "call makeAvailable at least once before getSettings".into()
        ))
    }

    async fn make_available(&self) -> WalletResult<Settings> {
        let mut guard = self.settings.lock().await;
        if let Some(ref s) = *guard {
            return Ok(s.clone());
        }
        let s: Settings = self.rpc_call("makeAvailable", vec![]).await?;
        *guard = Some(s.clone());
        Ok(s)
    }
    // ... remaining 20 methods below
}
```

### Pattern 4: is_available sync/async mismatch resolution

**What:** `is_available()` is a `fn` (sync) on the trait but settings are stored behind a `tokio::sync::Mutex`. Resolution: add a separate `AtomicBool` field `settings_cached: AtomicBool` that is set to `true` after `make_available()` succeeds.

```rust
pub struct StorageClient<W: ...> {
    auth_fetch: Mutex<AuthFetch<W>>,
    endpoint_url: String,
    next_id: AtomicU64,
    settings: Mutex<Option<Settings>>,
    settings_cached: AtomicBool,  // for sync is_available()
}

// In make_available:
self.settings_cached.store(true, Ordering::Release);

// In is_available:
fn is_available(&self) -> bool {
    self.settings_cached.load(Ordering::Acquire)
}
```

### Anti-Patterns to Avoid

- **Using std::sync::Mutex for AuthFetch:** `fetch()` is async and takes `&mut self`. Holding a std Mutex across an `.await` point will deadlock the Tokio executor. Always use `tokio::sync::Mutex`.
- **Calling blocking_lock() on Tokio Mutex:** Will panic inside async context. Only use `.lock().await`.
- **Serializing params as a struct:** JSON-RPC 2.0 positional params MUST be a JSON array `[...]`, not an object. Use `Vec<serde_json::Value>`.
- **Wire method names with "Auth" suffix where TS drops it:** `find_output_baskets_auth` Rust method → `"findOutputBaskets"` wire name (no Auth). See table below.
- **Trying to use try_lock for is_available:** Use a dedicated AtomicBool field instead.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Authenticated HTTP with BRC-31 mutual auth | Custom HTTP client with auth signing | `AuthFetch<W>` from bsv-sdk | AuthFetch handles the full BRC-31 handshake, session reuse, certificate exchange |
| JSON serialization of camelCase params | Custom serialization | `serde_json::to_value` with `#[serde(rename_all = "camelCase")]` on arg types | All arg types already have camelCase serde attributes from Phase 1/2 |
| Error code mapping from wire format | Custom WERR string parser | `WalletErrorObject` struct (already exists in error.rs) + a `wallet_error_from_object()` fn | WalletErrorObject already covers all WERR variants; just need a conversion function |
| Thread-safe ID counter | Mutex<u64> | `AtomicU64` | Lock-free, no contention on concurrent calls |

**Key insight:** Nearly all infrastructure already exists. StorageClient is a thin orchestration layer — the hard work (AuthFetch, WalletErrorObject, serde, all arg types) is already in place.

---

## Wire Method Name Table (CRITICAL)

This table maps every Rust trait method to its TS wire name. Mismatches cause runtime failures.

| Rust Trait Method | Wire Method Name (params) | Notes |
|---|---|---|
| `make_available` | `"makeAvailable"` `[]` | Caches result in settings field |
| `migrate` | `"migrate"` `[storageName, storageIdentityKey]` | TS ignores for remote |
| `destroy` | `"destroy"` `[]` | |
| `find_or_insert_user` | `"findOrInsertUser"` `[identityKey]` | |
| `abort_action` | `"abortAction"` `[auth, args]` | |
| `create_action` | `"createAction"` `[auth, args]` | |
| `process_action` | `"processAction"` `[auth, args]` | |
| `internalize_action` | `"internalizeAction"` `[auth, args]` | Rust signature has extra `services` param — serialize only auth+args, ignore services |
| `insert_certificate_auth` | `"insertCertificateAuth"` `[auth, certificate]` | |
| `relinquish_certificate` | `"relinquishCertificate"` `[auth, args]` | |
| `relinquish_output` | `"relinquishOutput"` `[auth, args]` | |
| `find_certificates_auth` | `"findCertificatesAuth"` `[auth, args]` | |
| `find_output_baskets_auth` | `"findOutputBaskets"` `[auth, args]` | **No "Auth" on wire** |
| `find_outputs_auth` | `"findOutputsAuth"` `[auth, args]` | |
| `find_proven_tx_reqs` | `"findProvenTxReqs"` `[args]` | No auth param |
| `list_actions` | `"listActions"` `[auth, args]` | |
| `list_certificates` | `"listCertificates"` `[auth, args]` | |
| `list_outputs` | `"listOutputs"` `[auth, args]` | |
| `find_or_insert_sync_state_auth` | `"findOrInsertSyncStateAuth"` `[auth, storageIdentityKey, storageName]` | |
| `set_active` | `"setActive"` `[auth, newActiveStorageIdentityKey]` | |
| `get_sync_chunk` | `"getSyncChunk"` `[args]` | |
| `process_sync_chunk` | `"processSyncChunk"` `[args, chunk]` | |
| `get_settings` | (sync-only local cache, no rpc) | Returns cached or errors |
| `is_available` | (sync-only local AtomicBool) | Returns settings_cached |
| `is_storage_provider` | (local, returns false) | |
| `get_services` | (local, returns NotImplemented) | |
| `set_services` | (local no-op, returns Ok) | |
| **inherent only:** `update_proven_tx_req_with_new_proven_tx` | `"updateProvenTxReqWithNewProvenTx"` `[args]` | NOT on WalletStorageProvider trait |

**Confidence:** HIGH — confirmed from `StorageClient.ts` source fetched directly from GitHub.

---

## Common Pitfalls

### Pitfall 1: is_available() sync/async conflict
**What goes wrong:** `is_available()` is a `fn` (not async) on `WalletStorageProvider` but settings are stored behind `tokio::sync::Mutex`. Calling `.try_lock()` may spuriously return None; calling `.blocking_lock()` panics inside Tokio.
**Why it happens:** The TS `isAvailable()` is synchronous (reads `!!this.settings`) but Rust async doesn't have sync shortcuts.
**How to avoid:** Add an `AtomicBool settings_cached` field. Set it to `true` in `make_available()` after the RPC call succeeds. Read it atomically in `is_available()`.
**Warning signs:** Tests that check `is_available()` after `make_available()` failing, or runtime panics mentioning `blocking_lock`.

### Pitfall 2: std::sync::Mutex + AuthFetch + async = deadlock
**What goes wrong:** `AuthFetch::fetch()` is async and requires `&mut self`. If wrapped in `std::sync::Mutex`, any `.await` inside the lock guard will block the thread, deadlocking the Tokio runtime.
**Why it happens:** Tokio is cooperative — blocking a thread prevents other tasks from running.
**How to avoid:** Always use `tokio::sync::Mutex<AuthFetch<W>>`. This is also documented in STATE.md as a locked decision.
**Warning signs:** Tests hang indefinitely rather than failing.

### Pitfall 3: findOutputBaskets wire name (Auth suffix mismatch)
**What goes wrong:** The Rust trait method is `find_output_baskets_auth` but the TS wire name is `"findOutputBaskets"` (no Auth suffix). Using `"findOutputBasketsAuth"` will result in a server-side "method not found" JSON-RPC error.
**Why it happens:** TS made an inconsistency — most auth methods keep the Auth suffix on the wire, but `findOutputBaskets` does not.
**How to avoid:** Use the wire name table above. Only `findOutputBaskets` drops the Auth suffix.
**Warning signs:** Server returns `WERR_INVALID_OPERATION` or "unknown method" for basket queries.

### Pitfall 4: internalize_action services parameter
**What goes wrong:** The Rust `WalletStorageProvider::internalize_action` takes an extra `services: &dyn WalletServices` parameter that has no TS equivalent. Attempting to serialize it will fail (it is a trait object, not serializable).
**Why it happens:** The Rust trait was extended with `services` to support local implementations; the remote client doesn't use it.
**How to avoid:** In `StorageClient::internalize_action`, accept the `services` parameter and silently ignore it. Only serialize `auth` and `args` into the params array.
**Warning signs:** Compilation errors trying to serialize `&dyn WalletServices`.

### Pitfall 5: JSON-RPC error vs HTTP error
**What goes wrong:** `AuthFetchResponse.status` encodes HTTP status. A 200 HTTP response can still contain a `json.error` JSON-RPC error. Must check both.
**Why it happens:** JSON-RPC 2.0 protocol — the server returns HTTP 200 but embeds application-level errors in the response body.
**How to avoid:** In `rpc_call`: (1) check `response.status >= 400` first for transport errors; (2) then check `json.get("error")` for application errors.

### Pitfall 6: migrate params
**What goes wrong:** `migrate` takes both `storage_name` and `storage_identity_key` but the params array order matters. TS signature is `[storageName, storageIdentityKey]`.
**Why it happens:** Positional JSON-RPC params — order is the only contract.
**How to avoid:** Follow the table above. Rust trait method args map to `[storage_name, storage_identity_key]` in that order.

---

## Code Examples

Verified patterns from TS source:

### JSON-RPC 2.0 envelope structure
```json
// Source: StorageClient.ts rpcCall method
{
  "jsonrpc": "2.0",
  "method": "findOutputBaskets",
  "params": [{ "identityKey": "...", "userId": 1 }, { "basket": "default" }],
  "id": 42
}
```

### Error response structure
```json
// Source: StorageClient.ts error branch + WalletErrorObject in error.rs
{
  "jsonrpc": "2.0",
  "error": {
    "isError": true,
    "name": "WERR_INVALID_PARAMETER",
    "message": "WERR_INVALID_PARAMETER: The basket parameter must be a string",
    "parameter": "basket"
  },
  "id": 42
}
```

### wallet_error_from_object conversion (needs to be written)
```rust
// Pattern: convert WalletErrorObject back to WalletError
fn wallet_error_from_object(obj: WalletErrorObject) -> WalletError {
    match obj.name.as_str() {
        "WERR_INVALID_PARAMETER" => WalletError::InvalidParameter {
            parameter: obj.parameter.unwrap_or_default(),
            must_be: obj.message,
        },
        "WERR_NOT_IMPLEMENTED" => WalletError::NotImplemented(obj.message),
        "WERR_BAD_REQUEST" => WalletError::BadRequest(obj.message),
        "WERR_UNAUTHORIZED" => WalletError::Unauthorized(obj.message),
        "WERR_NOT_ACTIVE" => WalletError::NotActive(obj.message),
        "WERR_INVALID_OPERATION" => WalletError::InvalidOperation(obj.message),
        "WERR_MISSING_PARAMETER" => WalletError::MissingParameter(
            obj.parameter.unwrap_or_else(|| obj.message.clone())
        ),
        "WERR_INSUFFICIENT_FUNDS" => WalletError::InsufficientFunds {
            message: obj.message,
            total_satoshis_needed: obj.total_satoshis_needed.unwrap_or(0),
            more_satoshis_needed: obj.more_satoshis_needed.unwrap_or(0),
        },
        "WERR_BROADCAST_UNAVAILABLE" => WalletError::BroadcastUnavailable,
        "WERR_NETWORK_CHAIN" => WalletError::NetworkChain(obj.message),
        "WERR_INVALID_PUBLIC_KEY" => WalletError::InvalidPublicKey {
            message: obj.message,
            key: obj.parameter.unwrap_or_default(),
        },
        _ => WalletError::Internal(obj.message),
    }
}
```

### updateProvenTxReqWithNewProvenTx args/result types (need to be defined)
```rust
// Source: WalletStorage.interfaces.ts UpdateProvenTxReqWithNewProvenTxArgs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProvenTxReqWithNewProvenTxArgs {
    pub proven_tx_req_id: i64,
    pub txid: String,
    pub attempts: i64,
    pub status: crate::status::SyncStatus, // or ProvenTxReqStatus - verify type
    pub history: String,
    pub height: i64,
    pub index: i64,
    pub block_hash: String,
    pub merkle_root: String,
    pub merkle_path: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProvenTxReqWithNewProvenTxResult {
    pub status: crate::status::SyncStatus, // verify
    pub history: String,
    pub proven_tx_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| StorageProvider blanket impl for remote clients | WalletStorageProvider implemented directly on StorageClient | Phase 2 decision | StorageClient does NOT have to implement StorageProvider |
| std::sync::Mutex for shared state | tokio::sync::Mutex for async-held locks | Pre-phase decision (STATE.md) | Avoids deadlock under Tokio |

---

## Open Questions

1. **UpdateProvenTxReqWithNewProvenTxArgs status field type**
   - What we know: TS uses `ProvenTxReqStatus` enum (not `SyncStatus`)
   - What's unclear: Whether the Rust codebase has a `ProvenTxReqStatus` type or uses `SyncStatus` for this
   - Recommendation: Grep for `ProvenTxReqStatus` in the codebase before defining the args struct. If absent, define it as a new `sqlx_string_enum`-style type in `src/status.rs` or inline in the remoting module.

2. **reset_session path**
   - What we know: STATE.md flags that `AuthFetch.peers` field may not be `pub`; resetting session may require recreating the entire `AuthFetch<W>` instance
   - What's unclear: Whether this is needed for Phase 3 (basic client) vs later phases
   - Recommendation: Phase 3 does not need session reset. Mark as a Phase 6 concern. If a re-auth is needed, recreate the AuthFetch by locking and replacing it. Confirmed that `peers` is private from bsv-sdk source.

3. **ProvenTx type for updateProvenTxReqWithNewProvenTx**
   - What we know: TS `UpdateProvenTxReqWithNewProvenTxArgs` has `height, index, blockHash, merkleRoot, merklePath` fields inline (not a nested ProvenTx)
   - What's unclear: Whether the existing Rust `update_proven_tx_req_with_new_proven_tx` method in `StorageReaderWriter` uses the same shape
   - Recommendation: Check `src/storage/traits/reader_writer.rs` lines 298-307 — the Rust method takes `(req_id: i64, proven_tx: &ProvenTx, trx)`. The StorageClient inherent method will take the flat TS-compatible args struct and call `rpc_call` directly without needing to match the internal Rust StorageReaderWriter signature.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[cfg(test)]` + `#[tokio::test]` (tokio 1.x) |
| Config file | none (standard Cargo test runner) |
| Quick run command | `cargo test --lib storage::remoting` |
| Full suite command | `cargo test` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CLIENT-01 | rpc_call produces correct JSON-RPC 2.0 envelope | unit | `cargo test --lib storage::remoting::tests::rpc_envelope` | Wave 0 |
| CLIENT-02 | StorageClient::new compiles and constructs | unit | `cargo test --lib storage::remoting::tests::new_constructs` | Wave 0 |
| CLIENT-03 | All 23 method stubs compile (blanket coverage by compilation) | compile-check | `cargo build` | Wave 0 |
| CLIENT-04 | JSON-RPC error response maps to correct WalletError variant | unit | `cargo test --lib storage::remoting::tests::error_mapping` | Wave 0 |
| CLIENT-05 | Constructor accepts WalletInterface impl | compile-check | `cargo build` | Wave 0 |
| CLIENT-06 | Wire method names match TS exactly | unit | `cargo test --lib storage::remoting::tests::wire_names` | Wave 0 |
| CLIENT-07 | is_available=false before makeAvailable; caches after | unit | `cargo test --lib storage::remoting::tests::settings_cache` | Wave 0 |

**Note:** Tests for CLIENT-03 and CLIENT-05 are effectively compile-time — if the struct compiles with all trait methods implemented and the generic bound satisfied, the requirements are met. Explicit unit tests for CLIENT-01, CLIENT-04, CLIENT-06, CLIENT-07 should use mock responses (no live network needed).

### Sampling Rate

- **Per task commit:** `cargo test --lib storage::remoting`
- **Per wave merge:** `cargo test`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps

- [ ] `src/storage/remoting/mod.rs` — module entry point
- [ ] `src/storage/remoting/storage_client.rs` — struct + impl, with `#[cfg(test)]` block
- [ ] `wallet_error_from_object` function (can live in `error.rs` or in `storage_client.rs`)
- [ ] `UpdateProvenTxReqWithNewProvenTxArgs` and `UpdateProvenTxReqWithNewProvenTxResult` structs — define in `src/storage/action_types.rs` or a new `src/storage/remoting/types.rs`
- [ ] Confirm `ProvenTxReqStatus` type: `cargo grep ProvenTxReqStatus src/` before writing the args struct

---

## Sources

### Primary (HIGH confidence)

- bsv-sdk-0.1.75 source at `/Users/elisjackson/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` — AuthFetch struct, fetch() signature, new() signature
- `src/storage/traits/wallet_provider.rs` — all 27 WalletStorageProvider method signatures confirmed
- `src/error.rs` — WalletError variants, WalletErrorObject struct confirmed
- Cargo.toml — tokio, serde_json, bsv-sdk versions confirmed

### Secondary (MEDIUM confidence)

- https://raw.githubusercontent.com/bsv-blockchain/wallet-toolbox/master/src/storage/remoting/StorageClient.ts — all wire method names, params arrays, rpcCall structure, settings caching pattern (fetched directly, HIGH for wire names)
- https://raw.githubusercontent.com/bsv-blockchain/wallet-toolbox/master/src/sdk/WalletStorage.interfaces.ts — UpdateProvenTxReqWithNewProvenTxArgs/Result field shapes

### Tertiary (LOW confidence)

- ProvenTxReqStatus enum fields — inferred from TS type name; exact Rust mapping needs local grep

---

## Metadata

**Confidence breakdown:**

- Standard stack: HIGH — all dependencies confirmed in Cargo.toml and bsv-sdk source
- Architecture: HIGH — AuthFetch API, WalletStorageProvider trait, WalletErrorObject all confirmed from codebase
- Wire names: HIGH — confirmed directly from TS StorageClient.ts source
- UpdateProvenTxReqWithNewProvenTx arg types: MEDIUM — field names confirmed from TS interfaces, Rust status type mapping unverified
- Pitfalls: HIGH — std vs tokio Mutex pitfall confirmed by bsv-sdk source (fetch takes &mut self, is async)

**Research date:** 2026-03-24
**Valid until:** 2026-06-24 (stable domain — bsv-sdk 0.1.75 is pinned, TS protocol is frozen for this milestone)
