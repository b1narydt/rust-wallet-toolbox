# Feature Research

**Domain:** JSON-RPC HTTP client implementing WalletStorageProvider for BSV wallet remote storage
**Researched:** 2026-03-24
**Confidence:** HIGH — based on direct source reading of TS StorageClient.ts, WalletStorage.interfaces.ts, bsv-sdk-0.1.75 AuthFetch, existing Rust sync/error modules

---

## Feature Landscape

### Table Stakes (Users Expect These)

Features the WalletStorageManager and sync protocol require. Missing any one = StorageClient cannot be plugged in as a backup provider.

| Feature | Why Expected | Complexity | Dependencies |
|---------|--------------|------------|--------------|
| WalletStorageProvider trait impl (~25 methods) | WalletStorageManager accepts `Arc<dyn WalletStorageProvider>`; every method must forward to the server or the trait is incomplete | HIGH | Requires Rust trait mirroring TS WalletStorageProvider hierarchy (WalletStorageReader + WalletStorageWriter + WalletStorageSync) |
| JSON-RPC dispatch helper (`rpc_call`) | All 25 methods are thin wrappers over a single POST; without a shared helper each method reimplements HTTP error handling, body serialization, and response parsing | MEDIUM | Requires `reqwest` (already in Cargo.toml), `serde_json` for `{"jsonrpc":"2.0","method":"...","params":[...],"id":N}` envelope |
| BRC-31 auth via AuthFetch (`Mutex<AuthFetch<W>>`) | StorageServer rejects unauthenticated requests; AuthFetch.fetch() takes `&mut self`, requiring interior mutability for `Arc`-safe use | MEDIUM | `bsv-sdk 0.1.75` with `network` feature; `tokio::sync::Mutex` wrapping; wallet type parameter `W: WalletInterface + Clone` |
| `make_available` / `is_available` / `get_settings` lifecycle | Sync protocol calls `make_available` first to retrieve remote `Settings` and confirm the server knows this user; `is_available` gates all other calls in WalletStorageManager | LOW | Settings struct already defined in existing tables module |
| Entity datetime validation after deserialization | TS server serializes `NaiveDateTime` as ISO 8601 with trailing "Z"; JSON-RPC responses carry these as strings; Rust must parse them back to `NaiveDateTime` without panicking on the "Z" suffix | LOW | `serde_datetime` module already exists in crate — use it on all returned entity structs; no new code needed beyond applying it consistently |
| WalletError mapping from JSON-RPC error objects | TS server returns `{"error":{"isError":true,"name":"WERR_UNAUTHORIZED","message":"..."}}` on failures; client must reconstruct a `WalletError` variant with matching WERR code instead of returning an opaque HTTP error | MEDIUM | `WalletError` and `WalletErrorObject` already defined in `error.rs`; need `WalletErrorObject -> WalletError` conversion (`From` impl or helper function) |
| `getSyncChunk` / `processSyncChunk` forwarding | WalletStorageManager calls these to push sync chunks to backup storage; StorageClient must forward them verbatim as RPC calls with date validation on all entity arrays in the `SyncChunk` response | MEDIUM | `SyncChunk` and `ProcessSyncChunkResult` types already defined in `storage::sync` module |
| `AuthId` construction from wallet identity | Every write method takes `AuthId { identityKey, userId, isActive }` as first param; StorageClient must know the wallet's identity key to populate this before each call | LOW | Wallet identity key available from the `WalletInterface` passed to construct AuthFetch; store it at construction time |
| `findOrInsertUser` on first call | Server needs to know the user before any operation; this is the bootstrap step that maps identity key to userId on the server side | LOW | Single RPC call; no date validation needed on `TableUser` response beyond `created_at`/`updated_at` |
| `isStorageProvider()` returns false | WalletStorageManager uses this flag to skip migration calls on remote providers; returning true would cause incorrect behavior | LOW | One-line stub |

### Differentiators (Competitive Advantage)

Features that improve reliability, debuggability, or ease of integration beyond the minimum required interface.

| Feature | Value Proposition | Complexity | Dependencies |
|---------|-------------------|------------|--------------|
| Lazy `make_available` on first non-lifecycle call | TS StorageClient only calls `makeAvailable` when explicitly triggered; Rust could auto-call it on first use to reduce caller burden, matching WABClient's pattern of lazy init | LOW | Internal `settings: Option<Settings>` field + check at top of every method; negligible cost |
| Typed RPC call with per-method request/response structs | Generic `rpc_call<T>(method, params)` is the minimum; typed per-method structs (separate request/response types) make future server version mismatch detectable at compile time | MEDIUM | More boilerplate than the TS approach but improves Rust-idiomatic API; only worth it if methods have complex params that benefit from named fields over positional arrays |
| Configurable sequential request ID counter | TS uses `nextId: number` incremented atomically; Rust needs `AtomicU64` for thread safety without locking the whole client on ID generation | LOW | `std::sync::atomic::AtomicU64`; no new dep |
| Connection keep-alive via Peer reuse | AuthFetch internally manages one Peer per base URL; subsequent requests reuse the authenticated session, avoiding re-handshake overhead on every RPC call | LOW | Already handled internally by bsv-sdk AuthFetch — not something StorageClient needs to implement, but worth documenting as a behavior guarantee |
| `WalletStorageProvider` as a Rust trait (not just an impl) | If StorageClient is just a struct, tests and MockStorageProvider require a concrete type; defining `WalletStorageProvider` as an `async_trait` in the crate enables mock injection and local-server testing without an HTTP connection | HIGH | Defines the interface boundary, required anyway for WalletStorageManager to hold it as `Arc<dyn WalletStorageProvider>` |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Implementing StorageProvider (full 60+ method interface) | Would allow StorageClient to be used anywhere a local database provider is accepted | TS explicitly returns `false` from `isStorageProvider()`; the 60-method interface includes migration, transaction tokens, and CRUD methods that make no sense over HTTP — mapping them all would produce a misleading API with silent no-ops | Keep StorageClient implementing only WalletStorageProvider; WalletStorageManager already handles the interface distinction |
| Async connection pooling / retry logic inside StorageClient | Seems like reliability improvement; common in HTTP client libraries | AuthFetch already manages session state per-peer; adding retry at the StorageClient level would re-send mutating calls (createAction, processAction) that are not idempotent, causing duplicate transactions | Surface errors to the caller; let higher-level wallet recovery logic decide whether to retry |
| Logger / tracing passthrough in RPC params | TS StorageClient embeds a `logger` seed object in params[1] to propagate log indentation across the network boundary | Adds protocol coupling: server must understand the logger format; any logger version mismatch silently corrupts params; Rust tracing spans already correlate across await points | Use Rust `tracing` spans locally; omit logger from wire params entirely — the server has its own logging |
| Caching deserialized responses (memoize findCertificatesAuth, etc.) | Avoids repeat network calls for unchanged data | StorageClient is a backup provider; reads are rare and correctness requires fresh data; a stale cache on the backup path could cause sync divergence | Accept the network cost; sync protocol already batches requests into chunks |
| `getServices` / `setServices` implementation | WalletStorageReader requires `getServices` | Remote clients cannot share local service objects (chain trackers, broadcast services) over the wire; TS throws `WERR_INVALID_OPERATION` from `getServices` | Return `WalletError::InvalidOperation` from `get_services`; `set_services` is a no-op |

---

## Feature Dependencies

```
WalletStorageProvider trait (Rust)
    └──requires──> WalletStorageSync trait
                       └──requires──> WalletStorageWriter trait
                                          └──requires──> WalletStorageReader trait

JSON-RPC dispatch helper (rpc_call)
    └──requires──> AuthFetch<W> behind Mutex
                       └──requires──> WalletInterface type parameter on StorageClient

Entity datetime validation
    └──requires──> serde_datetime module (already exists)
    └──applies-to──> all methods returning entities: getSyncChunk, findCertificatesAuth,
                     findOutputBasketsAuth, findOutputsAuth, findProvenTxReqs,
                     findOrInsertSyncStateAuth

WalletError mapping from JSON-RPC error
    └──requires──> WalletErrorObject -> WalletError conversion (new From impl)
    └──used-by──> rpc_call (single conversion point)

AuthId construction
    └──requires──> identity key stored at StorageClient construction time
    └──used-by──> all WalletStorageWriter + WalletStorageSync methods (11 of 25)

make_available lifecycle
    └──required-before──> all other methods (WalletStorageManager contract)
    └──populates──> Settings cache on StorageClient

getSyncChunk / processSyncChunk
    └──requires──> SyncChunk, ProcessSyncChunkResult types (already in storage::sync)
    └──requires──> entity datetime validation on SyncChunk fields
```

### Dependency Notes

- **WalletStorageProvider requires the full sub-trait chain:** The TS interface splits into WalletStorageReader (7 methods), WalletStorageWriter (9 methods), WalletStorageSync (4 methods) = 20 required + 5 lifecycle/meta = ~25 total. All must be implemented before the trait is satisfiable.
- **AuthFetch requires `&mut self`:** This forces `Mutex<AuthFetch<W>>` on StorageClient. Every RPC call must acquire the lock. This is correct — concurrent RPC calls share the same Peer session state, which is not Send without the Mutex.
- **Entity datetime validation is a single-point concern:** The serde_datetime module handles deserialization. The validation needed after RPC response parsing is the same `null -> None` / `Uint8Array -> Vec<u8>` coercion the TS client does in `validateEntity`. In Rust this is already handled by serde derives — no runtime fixup step is needed beyond ensuring all entity structs use `#[serde(default)]` for optional fields.
- **WalletError mapping is a prerequisite for rpc_call:** The helper must convert `json.error` to `WalletError` before returning; this is the single place where all error mapping lives.

---

## MVP Definition

### Launch With (v1)

Minimum needed for StorageClient to work as a backup provider in WalletStorageManager and satisfy the end-to-end payment receive flow.

- [ ] `WalletStorageProvider` Rust trait definition (~25 methods, using `async_trait`) — WalletStorageManager cannot hold StorageClient without this
- [ ] `StorageClient<W>` struct with `AuthFetch<W>` behind `Mutex`, endpoint URL, stored identity key, and optional cached Settings — this is the core type
- [ ] `rpc_call<T>` helper: build JSON-RPC envelope, POST via AuthFetch, parse response, map `json.error` to WalletError — single implementation point for all network behavior
- [ ] `WalletErrorObject -> WalletError` conversion: parse `name` field against WERR codes, reconstruct correct variant — required by rpc_call
- [ ] All ~25 WalletStorageProvider methods as thin wrappers over rpc_call with correct method name strings and parameter ordering matching TS wire format — required for trait impl
- [ ] Entity datetime post-processing on methods that return entities (getSyncChunk, findCertificatesAuth, findOutputBasketsAuth, findOutputsAuth, findProvenTxReqs, findOrInsertSyncStateAuth) — prevents panics on date fields from live servers
- [ ] `make_available`, `is_available`, `get_settings` lifecycle methods — WalletStorageManager calls these before any sync operation

### Add After Validation (v1.x)

- [ ] Integration test against live storage.babbage.systems or storage.b1nary.tech — trigger: v1 compiles and passes unit tests; validates wire compatibility assumptions
- [ ] `AtomicU64` request ID counter — trigger: identified as a correctness issue under concurrent use; not needed if StorageClient is only used serially behind WalletStorageManager's write lock
- [ ] Lazy `make_available` on first use — trigger: callers report friction from having to call make_available manually; low effort to add

### Future Consideration (v2+)

- [ ] Typed per-method request/response structs instead of positional `params` arrays — defer until server protocol has stabilized and breaking changes become a real concern
- [ ] Health-check / reconnect logic for long-lived backup providers — defer until production deployments show session expiry as a problem; bsv-sdk AuthFetch handles this at the Peer level already

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| WalletStorageProvider trait definition | HIGH | MEDIUM | P1 |
| StorageClient<W> struct + rpc_call helper | HIGH | MEDIUM | P1 |
| BRC-31 auth via Mutex<AuthFetch<W>> | HIGH | LOW | P1 |
| WalletErrorObject -> WalletError conversion | HIGH | LOW | P1 |
| All ~25 method stubs forwarding to rpc_call | HIGH | MEDIUM | P1 |
| Entity datetime post-processing | HIGH | LOW | P1 |
| make_available / is_available / get_settings | HIGH | LOW | P1 |
| AtomicU64 request ID | LOW | LOW | P2 |
| Lazy make_available on first use | MEDIUM | LOW | P2 |
| Integration test vs live TS server | HIGH | MEDIUM | P2 |
| Typed per-method structs | LOW | HIGH | P3 |
| Health-check / reconnect | LOW | HIGH | P3 |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

---

## Reference Implementation Analysis

TS `StorageClient` (wallet-toolbox/src/storage/remoting/StorageClient.ts) is the authoritative reference. Key observations from direct source reading:

| Aspect | TS Behavior | Rust Implication |
|--------|-------------|-----------------|
| JSON-RPC envelope | `{jsonrpc:"2.0", method, params:[], id}` — params is always a positional array | `serde_json::json!` macro to build; no typed request struct needed |
| Error detection | `if (json.error)` — checks for `error` key in response root | Deserialize response as `{result: Option<T>, error: Option<WalletErrorObject>}` |
| Auth | `AuthFetch` constructed once with wallet; `authClient.fetch()` called per request | `Mutex<AuthFetch<W>>` field; lock per rpc_call invocation |
| Settings caching | `settings?: TableSettings` — cached after first makeAvailable, checked by isAvailable | `Option<Settings>` field; set once |
| Entity validation | `validateEntity` / `validateEntities` called on all returned entity arrays | Not needed in Rust — serde handles type coercion; `serde_datetime` handles the "Z" suffix; `#[serde(default)]` handles null-to-None |
| Logger param | `params[1]?.logger` replaced with seed object before send | Omit entirely in Rust implementation; use tracing spans locally |
| Method names | Exact strings: `makeAvailable`, `destroy`, `migrate`, `internalizeAction`, `createAction`, `processAction`, `abortAction`, `findOrInsertUser`, `findOrInsertSyncStateAuth`, `insertCertificateAuth`, `listActions`, `listOutputs`, `listCertificates`, `findCertificatesAuth`, `findOutputBasketsAuth` (note: method string is `findOutputBaskets` not `findOutputBasketsAuth`), `findOutputsAuth`, `findProvenTxReqs`, `relinquishCertificate`, `relinquishOutput`, `processSyncChunk`, `getSyncChunk`, `updateProvenTxReqWithNewProvenTx`, `setActive` | These exact strings must appear in Rust rpc_call invocations — any mismatch causes 404 or dispatch error at the server |
| `findOutputBasketsAuth` name mismatch | TS method is named `findOutputBasketsAuth` but sends RPC method string `'findOutputBaskets'` | Rust must match the wire string `findOutputBaskets`, not the trait method name |

---

## Sources

- Direct source: `/Users/elisjackson/Projects/metawatt-edge-rs/.ref/wallet-toolbox/src/storage/remoting/StorageClient.ts` (TS reference implementation, HIGH confidence)
- Direct source: `/Users/elisjackson/Projects/metawatt-edge-rs/.ref/wallet-toolbox/src/sdk/WalletStorage.interfaces.ts` (WalletStorageProvider interface hierarchy, HIGH confidence)
- Direct source: `~/.cargo/registry/src/.../bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` (AuthFetch API: `&mut self`, `Mutex` requirement, HIGH confidence)
- Direct source: `src/error.rs` (WalletError + WalletErrorObject already defined, HIGH confidence)
- Direct source: `src/serde_datetime.rs` (datetime handling already solved, HIGH confidence)
- Direct source: `src/storage/sync/` (SyncChunk types already defined, HIGH confidence)
- Direct source: `src/wab_client/mod.rs` (existing reqwest HTTP client pattern in codebase, HIGH confidence)
- Direct source: `Cargo.toml` (available deps: reqwest 0.12, serde_json 1, tokio Mutex, bsv-sdk 0.1.75, HIGH confidence)

---

*Feature research for: StorageClient JSON-RPC remote storage transport, rust-wallet-toolbox*
*Researched: 2026-03-24*
