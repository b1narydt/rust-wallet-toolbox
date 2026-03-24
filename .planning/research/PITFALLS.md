# Pitfalls Research

**Domain:** Cross-language JSON-RPC client — Rust StorageClient interoperating with TypeScript StorageServer
**Researched:** 2026-03-24
**Confidence:** HIGH (verified against live TS source at wallet-toolbox/src/storage/remoting/StorageClient.ts and bsv-sdk-0.1.75 AuthFetch source)

---

## Critical Pitfalls

### Pitfall 1: Binary Fields Sent as Number Arrays, Not Base64

**What goes wrong:**
The TypeScript StorageServer sends binary fields (`raw_tx`, `merkle_path`, `input_beef`) as plain JSON number arrays (`[1, 2, 3, ...]`), not base64 strings. Serde's default `Vec<u8>` deserializer expects a JSON array of numbers and will work correctly in this direction — but Rust's default `Vec<u8>` serializer also emits a number array. This means round-trip is consistent, but it is easy to accidentally introduce a base64 serializer (via `serde_with::base64` or similar) thinking it is "more standard", which will break the wire format immediately.

**Why it happens:**
Developers expect binary data over HTTP to be base64. The TS SDK explicitly converts `Buffer` and `Uint8Array` fields using `Array.from(val)` on both the client and server side. The round-trip works, but the wire encoding is a raw JSON number array, which is unusual and easy to "fix" incorrectly.

**How to avoid:**
Do not add `#[serde(with = "...")]` base64 helpers to `raw_tx`, `merkle_path`, `input_beef`, or any other `Vec<u8>` field on structs used over the wire. The existing `Serialize/Deserialize` derive on `ProvenTx`, `Transaction`, and similar structs already emits number arrays. Verify this explicitly in a wire-format unit test before connecting to a live server.

**Warning signs:**
- Deserialization errors containing "expected a sequence" or "expected a string" on binary fields.
- The TS server returns `[1,2,3]` but Rust rejects it because someone added `serde_with::base64`.
- `serde_json::to_value(&proven_tx)` shows `"merkle_path": "AAEC..."` (base64 string) instead of `"merkle_path": [0, 1, 2, ...]` (array).

**Phase to address:**
Wire-format definition phase — add a `#[test]` that roundtrips a known `ProvenTx` JSON fixture from the live TS server through serde before writing any StorageClient method.

---

### Pitfall 2: `AuthFetch<W>` Requires `W: WalletInterface + Clone + 'static` — Kills Trait Object Use

**What goes wrong:**
`AuthFetch` is generic over `W: WalletInterface + Clone + 'static`. `StorageClient` wraps `AuthFetch<W>`, making `StorageClient<W>` also generic. When `StorageClient` is stored as a backup provider in `WalletStorageManager` — which holds `Arc<dyn StorageProvider>` — the compiler rejects it because `dyn WalletInterface` is not `Clone`. The error appears far from the root cause, typically as "the trait `Clone` is not implemented for `dyn WalletInterface`" on an `Arc<dyn StorageProvider>` coercion.

**Why it happens:**
`WalletStorageManager` (and everything above it) uses `Arc<dyn StorageProvider>` for type erasure. To put `StorageClient<W>` behind that `dyn`, `StorageClient<W>` must implement `StorageProvider` with `W` erased. But `AuthFetch<W>` caches its generic wallet instance internally, so the only way to satisfy `dyn`-compatibility is to store `AuthFetch<W>` inside a `Mutex` and make the whole `StorageClient` concrete over a specific `W` type, which propagates `W` into every caller.

**How to avoid:**
Accept that `StorageClient` will remain a concrete generic type `StorageClient<W>`. At the integration point (constructing `WalletStorageManager`), require the concrete wallet type `W` rather than a `dyn WalletInterface`. This is the same constraint the TS side avoids because TypeScript uses structural typing. Alternatively, introduce a thin `BoxedStorageClient` newtype that holds `Arc<Mutex<dyn ErasedStorageClient>>` — but this adds complexity that is probably not worth it for this milestone.

**Warning signs:**
- Attempt to coerce `Arc<StorageClient<W>>` to `Arc<dyn StorageProvider>` fails at compile time.
- Attempting to implement `StorageProvider` for `StorageClient<dyn WalletInterface>` fails because `dyn WalletInterface: !Clone`.
- `WalletStorageManager::new` starts requiring a generic parameter it previously did not.

**Phase to address:**
Architecture phase — decide before writing any trait impl whether `StorageClient` will be type-erased or concrete. Document the decision in code. The PROJECT.md entry "AuthFetch behind Mutex for Send+Sync" is the right instinct; verify it compiles as `Arc<Mutex<AuthFetch<W>>>` where `W` is a specific concrete wallet type, not `dyn`.

---

### Pitfall 3: `Mutex<AuthFetch>` Deadlock from Re-entrant Lock in Async Context

**What goes wrong:**
`AuthFetch::fetch()` takes `&mut self`, so `StorageClient` wraps it in `Mutex<AuthFetch<W>>`. Each storage call acquires the mutex, calls `auth_fetch.fetch()` (which is async), and holds the lock across the `.await`. If any code path attempts to acquire the same `Mutex` while the lock is held — including indirect callers or error paths that retry — a deadlock occurs on a single-threaded Tokio executor. Using `std::sync::Mutex` instead of `tokio::sync::Mutex` makes this worse: `std::sync::Mutex` will block the Tokio worker thread, potentially starving the executor.

**Why it happens:**
`fetch()` itself internally does multiple `.await` points (handshake, peer message send, response receive with 30-second timeout loop). Holding any mutex across those awaits violates Tokio's single-lock-per-task-at-a-time guidance. An accidental nested call — e.g., a retry helper or a `drop` implementation that calls back into the client — deadlocks immediately.

**How to avoid:**
Use `tokio::sync::Mutex`, not `std::sync::Mutex`. Never call any `StorageClient` method from within a context that already holds the inner `AuthFetch` lock. In practice this means: do not implement `Drop` on `StorageClient` in a way that calls `auth_fetch`. Do not pass a `StorageClient` reference to code that runs concurrently under the same lock. Guard the happy path with `tokio::sync::MutexGuard` and ensure the guard is dropped before any branch that could re-acquire.

**Warning signs:**
- Tests that run concurrently hang indefinitely with no panic.
- Tokio's `--cfg tokio_unstable` `tracing` shows a task stuck waiting on a mutex that another task holds, but the other task is also waiting on an `.await`.
- `cargo test -- --nocapture` shows "acquiring lock..." never followed by "acquired".

**Phase to address:**
StorageClient implementation phase — wrap `AuthFetch` in `Arc<tokio::sync::Mutex<AuthFetch<W>>>` from the start, not `std::sync::Mutex`. Add a compile-time comment explaining why.

---

### Pitfall 4: Timestamp Serialization: Missing Milliseconds Truncation

**What goes wrong:**
The existing `serde_datetime` module correctly strips the trailing `"Z"` from TS ISO timestamps. However, `NaiveDateTime::format("%Y-%m-%dT%H:%M:%S%.f")` emits fractional seconds with up to 9 digits (nanoseconds), while TS `Date.toISOString()` emits exactly 3 digits (milliseconds, e.g. `"2026-01-11T23:13:12.718Z"`). When Rust serializes a timestamp and sends it to a TS server that re-parses it for a filter (e.g., `getSyncChunk since` parameter), the server may reject or silently truncate the value.

**Why it happens:**
Chrono defaults to nanosecond precision. JavaScript Date objects have millisecond precision. The format string `%.f` in Chrono prints as many significant fractional digits as the value contains, which can be 3, 6, or 9 depending on how the timestamp was created.

**How to avoid:**
Change the serialize format in `serde_datetime` to `"%Y-%m-%dT%H:%M:%S%.3f"` (three decimal places). Alternatively, truncate the `NaiveDateTime` to milliseconds before formatting: `dt.with_nanosecond((dt.nanosecond() / 1_000_000) * 1_000_000)`. Also append `"Z"` to the serialized output — the TS server expects the trailing `Z` and the current `serde_datetime::serialize` omits it, which will cause the TS side to parse the timestamp as local time rather than UTC.

**Warning signs:**
- TS server returns `400 Bad Request` or `422 Unprocessable Entity` on `getSyncChunk` calls with a `since` parameter.
- Comparing round-tripped timestamps in tests: Rust emits `"2026-01-11T23:13:12.718000000"`, TS expects `"2026-01-11T23:13:12.718Z"`.
- `serde_json::to_string(&dt)` for a timestamp created from SQLite (`NaiveDateTime::from_timestamp_millis`) produces correct output, but one created from `Utc::now().naive_utc()` produces nanoseconds.

**Phase to address:**
Wire-format definition phase — add a round-trip test: serialize a known `NaiveDateTime` with millisecond precision and verify the JSON output matches the format the TS server sends.

---

### Pitfall 5: JSON-RPC Parameters Sent as Positional Arrays, Not Named Objects

**What goes wrong:**
The TS `StorageClient.rpcCall(method, params)` sends parameters as a JSON array: `{"jsonrpc":"2.0","method":"findOutputs","params":[arg0, arg1],"id":1}`. The TS `StorageServer` dispatches via `await storage[method](...params)` — spreading the array as positional arguments. If the Rust client sends named parameters (`{"params":{"args":...}}`) instead of a positional array, every call will fail with a silent wrong-argument error on the TS server, which typically returns `null` or a vague error.

**Why it happens:**
Most JSON-RPC 2.0 tutorials and Rust crates default to named parameters (object params). The TS storage server uses `...params` spread, which only works with array params. This is the live production wire format and there is no fallback.

**How to avoid:**
Build the JSON-RPC body manually using `serde_json::json!`:
```rust
let body = serde_json::json!({
    "jsonrpc": "2.0",
    "method": method_name,
    "params": [arg0, arg1],  // always an array, never an object
    "id": request_id,
});
```
Verify the method name and parameter count against the TS `StorageServer` dispatch table. There is no runtime error for wrong parameter count — the TS method simply receives `undefined` for missing positional arguments.

**Warning signs:**
- Server returns `{"result": null}` for calls that should return data.
- Server returns `{"error": {"message": "Cannot read property 'X' of undefined"}}` — a JavaScript null-dereference from a missing positional arg.
- A call that should return an array returns `[]` or `{}`.

**Phase to address:**
StorageClient implementation phase — define a shared `rpc_call` helper as the first function written in the new module. Test it against a real server (or a mock that validates the body shape) before implementing individual methods.

---

### Pitfall 6: BRC-31 Handshake Must Complete Before Sending the First JSON-RPC Request

**What goes wrong:**
The Rust `AuthFetch::fetch()` calls `peer.get_authenticated_session("")` before sending the request payload. If the handshake itself fails (network error, certificate mismatch, wrong identity key format), the error surfaces as an `AuthError` unrelated to the RPC call. Callers that catch generic errors and retry the same call will loop-retry the handshake against a server that has already advanced its session state, causing the second attempt to fail with a nonce-replay error.

**Why it happens:**
BRC-31 requires a one-time nonce exchange. The server stores the client's nonce to prevent replay. If the client retries after a partial handshake, the server rejects the stale nonce. `AuthFetch` maintains per-base-URL peer state in its `HashMap<String, AuthPeer<W>>`, so a failed handshake may leave a partially-initialized `AuthPeer` that subsequent calls attempt to reuse.

**How to avoid:**
On `AuthError` from `fetch()`, remove the affected base URL from the `AuthFetch` peers map before retrying. Since `AuthFetch` is behind `Mutex`, this requires calling a method that does `peers.remove(&base_url)` — or simply drop and recreate the entire `AuthFetch` instance. Do not silently retry; distinguish handshake errors (`AuthError::TransportNotConnected`) from payload errors (HTTP 4xx) and only reset the session on the former.

**Warning signs:**
- Second request to the same server always fails after the first fails.
- Server log shows "nonce already used" or "invalid initial request" errors.
- `AuthError::Timeout` on the second call when the first timed out during handshake.

**Phase to address:**
StorageClient implementation phase — implement `reset_session(&mut self)` on `StorageClient` alongside the core `rpc_call` method.

---

### Pitfall 7: `serde_datetime::serialize` Does Not Emit Trailing `"Z"` — TS Server Parses as Local Time

**What goes wrong:**
The current `serde_datetime::serialize` formats `NaiveDateTime` as `"2026-01-11T23:13:12.718"` without a trailing `"Z"`. The TS `StorageServer.validateDate()` calls `new Date(value)` on incoming parameters. In Node.js, `new Date("2026-01-11T23:13:12.718")` is parsed as **local time** in most environments (behavior is implementation-defined before ES2015; Node.js historically treats ISO strings without timezone as local). This silently shifts timestamps by the server's UTC offset when the Rust client sends filter parameters to the TS server.

**Why it happens:**
`NaiveDateTime` has no timezone concept, so it is natural to omit the `"Z"`. But the TS ecosystem treats the trailing `"Z"` as the canonical marker for UTC ISO strings. The asymmetry — Rust strips `"Z"` on deserialize but does not add it on serialize — creates a one-way mismatch.

**How to avoid:**
Change `serde_datetime::serialize` to always append `"Z"`:
```rust
let s = format!("{}Z", date.format("%Y-%m-%dT%H:%M:%S%.3f"));
```
This is a safe change because the `serde_datetime::deserialize` already strips trailing `"Z"`.

**Warning signs:**
- Integration tests pass in UTC environments (CI) but fail in local development with non-zero UTC offset.
- `getSyncChunk` returns records from the wrong time window when a `since` parameter is provided.
- Timestamps stored in the local SQLite database and timestamps returned from the remote server differ by the server's UTC offset.

**Phase to address:**
Wire-format definition phase — alongside the millisecond precision fix (Pitfall 4), add `"Z"` suffix to `serde_datetime::serialize`. Add a test asserting `serialize(naive_datetime) == "2026-01-11T23:13:12.718Z"`.

---

### Pitfall 8: `i64` Primary Keys Silently Lose Precision in JavaScript

**What goes wrong:**
All Rust table structs use `i64` for primary keys (`transaction_id`, `proven_tx_id`, etc.). JavaScript's `Number` type is IEEE 754 double-precision, which can only safely represent integers up to 2^53 - 1 (9,007,199,254,740,991). SQLite auto-increment IDs stay well below this limit in practice, but if the TS server ever returns an ID near or above 2^53, Rust's `i64` deserialization will accept it while JavaScript silently rounds it — causing a foreign key mismatch when Rust later sends the rounded value back.

**Why it happens:**
The TS wallet toolbox uses JavaScript `number` for all IDs. No BigInt is used. IDs are assigned sequentially by SQLite, so this limit is not hit in practice. But it is a latent bug if data is migrated from a large existing database.

**How to avoid:**
This is acceptable for this milestone (IDs will be small). Document it as a known limitation. If future-proofing is needed, use string-encoded IDs in the wire format (not in scope here).

**Warning signs:**
- IDs above 9 quadrillion in the database (not realistic for a wallet).
- JSON deserialization of `id` fields returns `9007199254740992` regardless of actual value.

**Phase to address:**
Architecture decision phase — document as accepted risk, no action required for this milestone.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Skip session-reset on auth error | Simpler error handling | Second request always fails after first network hiccup | Never — implement reset from the start |
| Use `std::sync::Mutex` for AuthFetch | Simpler type | Deadlocks on Tokio multi-thread executor | Never in async context |
| Hardcode `"jsonrpc": "2.0"` without tests | Faster initial build | Silent breakage if format drifts | Only if a wire-format test exists |
| Accept nanosecond timestamps without truncation | No extra code | Intermittent TS server rejection on `since` filters | Never — fix in `serde_datetime` upfront |
| Generic `StorageClient<W>` propagates into all callers | Correct architecture | Complicates `WalletStorageManager` construction | Acceptable — this is the right design |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Live TS server (storage.babbage.systems) | Test with `reqwest` first, add `AuthFetch` after | Add `AuthFetch` from the first HTTP call — the server requires BRC-31 auth and returns 401 without it |
| TS StorageServer `validateEntity` | Assume server returns native types | Server runs `Array.from(Buffer)` on binary fields and `new Date(...)` on timestamps — Rust receives number arrays and ISO strings, not raw bytes or epoch ints |
| `getSyncChunk` params | Serialize `SyncChunkArgs` as a named object | `params` must be a positional array: `[args]` (a single-element array containing the args object) |
| `processSyncChunk` params | Send chunk as a top-level JSON object | `params` must be `[chunk]` — the TS server spreads the array into the function, so `storage.processSyncChunk(chunk)` is called with one positional argument |
| BRC-31 handshake TLS | Connect to HTTP in dev | storage.babbage.systems requires HTTPS; `reqwest` needs TLS feature enabled, which `bsv-sdk 0.1.75` already brings in |

---

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Single `Mutex<AuthFetch>` serializes all RPC calls | All storage calls queue behind one lock | Acceptable for initial milestone; if needed, separate `AuthFetch` instances per endpoint | At sustained concurrent request load (~10+ simultaneous storage calls) |
| BRC-31 handshake on every new server connection | First call to a fresh `StorageClient` instance takes ~200-500ms for handshake | Reuse `StorageClient` across the lifetime of `WalletStorageManager` — do not create per-call | Immediate if `StorageClient` is recreated on each request |
| `getSyncChunk` returns large payloads without pagination | First sync of a large wallet stalls | Implement `paged` parameter in `getSyncChunk` args from the start | At ~10,000+ records per sync |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Log the full JSON-RPC request body (includes raw_tx, merkle_path) | Transaction data leak in logs | Log method name and param count only, not param values |
| Store AuthFetch wallet reference in a type that implements `Debug` without redaction | Private key material visible in log output | Ensure the `W: WalletInterface` concrete type's `Debug` impl does not print private key bytes |
| Skip TLS verification for dev convenience | MITM during development leaks identity keys from BRC-31 handshake | Use proper TLS even in dev; never add `danger_accept_invalid_certs` to reqwest |

---

## "Looks Done But Isn't" Checklist

- [ ] **Binary fields:** `serde_json::to_value(&proven_tx)` shows `"merkle_path": [0, 1, 2]` (array), not `"merkle_path": "AAEC..."` (base64). Verify before first live call.
- [ ] **Timestamp format:** `serde_datetime::serialize` output ends with `.718Z` (3 decimal places, trailing Z). Verify with a unit test.
- [ ] **JSON-RPC body shape:** `rpc_call` emits `{"jsonrpc":"2.0","method":"...","params":[...],"id":N}`. Verify `params` is always an array.
- [ ] **AuthFetch mutex type:** `grep -r "std::sync::Mutex" src/storage/remoting/` returns no results — it must be `tokio::sync::Mutex`.
- [ ] **Session reset on error:** `StorageClient` has a `reset_session()` or equivalent that clears the `AuthFetch` peer state on auth error.
- [ ] **`getSyncChunk` since parameter:** Transmitted as a string with trailing `"Z"`, not as an epoch integer or a local-time string.

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Binary field base64 mismatch discovered after integration | MEDIUM | Add `#[serde(with = "...")]` custom serializer to affected fields; no schema change needed |
| Timestamp UTC offset bug discovered in production | HIGH | Requires a data migration to correct shifted timestamps in the remote DB; fix `serde_datetime` and resync |
| Deadlock from `std::sync::Mutex` | LOW | Replace `std::sync::Mutex` with `tokio::sync::Mutex`; recompile |
| Positional vs named params causing silent null returns | LOW | Fix `rpc_call` helper; no data corruption |
| AuthFetch session stuck after partial handshake | LOW | Implement `reset_session()`; restart `StorageClient` as a workaround |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Binary fields as number arrays | Wire-format definition (before StorageClient code) | Unit test: `serde_json::to_value(&ProvenTx {...})["merkle_path"]` is a JSON array |
| AuthFetch generic constraint kills dyn trait | Architecture decision (before writing trait impls) | `StorageClient<W>` compiles into `WalletStorageManager` with a concrete `W` |
| Mutex deadlock in async context | StorageClient implementation (day 1) | `tokio::sync::Mutex` in source; integration test with concurrent calls |
| Timestamp millisecond + UTC suffix | Wire-format definition (alongside binary field test) | Round-trip test: Rust serialize → TS parse → Rust deserialize gives same value |
| Positional array params | StorageClient implementation (rpc_call helper) | Mock server test asserting `body["params"]` is `Array`, not `Object` |
| BRC-31 session reset on failure | StorageClient implementation (error handling) | Test: simulate transport error → verify second call succeeds without manual restart |
| Missing `"Z"` suffix on timestamps | Wire-format definition | Unit test asserting serialized output contains trailing `"Z"` |
| i64 precision loss in JavaScript | Architecture decision | Accepted risk — document as known limitation |

---

## Sources

- TypeScript `StorageClient.ts` source: https://github.com/bsv-blockchain/wallet-toolbox/blob/master/src/storage/remoting/StorageClient.ts (fetched 2026-03-24)
- TypeScript `StorageServer.ts` source: https://github.com/bsv-blockchain/wallet-toolbox/blob/master/src/storage/remoting/StorageServer.ts (fetched 2026-03-24)
- Rust `bsv-sdk 0.1.75` `AuthFetch` source: `/Users/elisjackson/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` (local)
- BRC-31 Authrite specification: https://github.com/bitcoin-sv/BRCs/blob/master/peer-to-peer/0031.md
- Tokio Mutex deadlock analysis: https://turso.tech/blog/how-to-deadlock-tokio-application-in-rust-with-just-a-single-mutex
- Tokio `sync::Mutex` docs: https://docs.rs/tokio/latest/tokio/sync/struct.Mutex.html
- JSON-RPC 2.0 specification (positional vs named params): https://www.jsonrpc.org/specification
- serde `Vec<u8>` as base64 discussion: https://users.rust-lang.org/t/serialize-a-vec-u8-to-json-as-base64/57781
- JavaScript i64 precision: https://github.com/near/NEPs/issues/319

---
*Pitfalls research for: Rust StorageClient HTTP/JSON-RPC transport with TypeScript StorageServer interop*
*Researched: 2026-03-24*
