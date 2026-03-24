# Architecture Research

**Domain:** HTTP/JSON-RPC remote storage client for BSV wallet-toolbox
**Researched:** 2026-03-24
**Confidence:** HIGH — based on direct inspection of the Rust codebase, TS reference implementation, and AuthFetch SDK source

## Standard Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Wallet (BRC-100)                           │
│   holds WalletStorageManager                                        │
├─────────────────────────────────────────────────────────────────────┤
│                    WalletStorageManager                             │
│   active: Arc<dyn StorageProvider>                                  │
│   backup: Option<Arc<dyn WalletStorageProvider>>  ← NEW FIELD TYPE │
│   sync_backup: fn(active, backup, auth_id)         ← NEW          │
├───────────────────────────────────┬─────────────────────────────────┤
│          SqlxProvider             │        StorageClient            │
│  (implements StorageProvider)     │  (implements WalletStorageProvider)│
│  60+ CRUD methods                 │  25 JSON-RPC pass-through methods│
│  SQLite / MySQL / Postgres        │  AuthFetch<W> behind Mutex      │
│                                   │  endpoint_url: String           │
└───────────────────────────────────┴────────────┬────────────────────┘
                                                 │ BRC-31 auth
                                                 ▼
                                    ┌─────────────────────────┐
                                    │  TS StorageServer        │
                                    │  storage.babbage.systems │
                                    │  storage.b1nary.tech     │
                                    └─────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Location |
|-----------|----------------|----------|
| `StorageProvider` trait | 60+ method CRUD interface: reads, writes, transactions, lifecycle | `src/storage/traits/` |
| `WalletStorageProvider` trait | NEW: ~25-method interface for remote-capable providers | `src/storage/traits/` (new file) |
| `StorageClient` struct | Implements `WalletStorageProvider` via JSON-RPC over BRC-31 auth | `src/storage/remoting/` (new) |
| `WalletStorageManager` | Coordinates active (local) + backup (local or remote) providers | `src/storage/manager.rs` (modified) |
| `AuthFetch<W>` | BRC-31 mutual auth HTTP client, requires `&mut self` for `fetch()` | `bsv-sdk` dependency |

## Recommended Project Structure

```
src/
├── storage/
│   ├── traits/
│   │   ├── mod.rs              # add WalletStorageProvider to pub exports
│   │   ├── provider.rs         # StorageProvider — unchanged
│   │   ├── reader.rs           # StorageReader — unchanged
│   │   ├── reader_writer.rs    # StorageReaderWriter — unchanged
│   │   └── wallet_storage.rs   # NEW: WalletStorageProvider trait (~25 methods)
│   ├── remoting/
│   │   ├── mod.rs              # pub use storage_client::StorageClient
│   │   └── storage_client.rs   # NEW: StorageClient<W: WalletInterface>
│   ├── manager.rs              # MODIFIED: backup field type change + sync helpers
│   └── ...
```

### Structure Rationale

- **`traits/wallet_storage.rs`:** Mirrors the TS `WalletStorageProvider` interface exactly. This is what both `WalletStorageManager` (via its local providers) and `StorageClient` implement. Keeps the 25-method surface separate from the 60+ method `StorageProvider` internals.
- **`remoting/storage_client.rs`:** Follows the TS naming and file location pattern. Generic over `W: WalletInterface` to remain testable (can inject a mock wallet).
- **`manager.rs` modification:** The backup field type must change from `Option<Arc<dyn StorageProvider>>` to `Option<Arc<dyn WalletStorageProvider>>`. Local providers implement both traits (blanket impl); remote `StorageClient` implements only `WalletStorageProvider`.

## Architectural Patterns

### Pattern 1: WalletStorageProvider as the Backup Slot Type

**What:** Define a new `WalletStorageProvider` trait with only the ~25 methods that `StorageClient` can implement as JSON-RPC pass-throughs. Change `WalletStorageManager.backup` from `Arc<dyn StorageProvider>` to `Arc<dyn WalletStorageProvider>`. Add a blanket impl so all existing `StorageProvider` implementors also implement `WalletStorageProvider`.

**When to use:** Required. The existing backup slot type forces full `StorageProvider` implementation, which cannot work for a remote client.

**Trade-offs:** Requires changing manager's backup type and updating all backup-propagation write paths to use only `WalletStorageProvider` methods. The write propagation in manager currently calls low-level CRUD methods (`update_output`, `update_transaction`) directly on backup — this must change to use sync-chunk-based replication instead.

```rust
// src/storage/traits/wallet_storage.rs
#[async_trait]
pub trait WalletStorageProvider: Send + Sync {
    fn is_available(&self) -> bool;
    async fn make_available(&self) -> WalletResult<Settings>;
    async fn migrate(&self, storage_name: &str, storage_identity_key: &str) -> WalletResult<String>;
    async fn destroy(&self) -> WalletResult<()>;

    async fn create_action(&self, auth: &AuthId, args: &StorageCreateActionArgs) -> WalletResult<StorageCreateActionResult>;
    async fn process_action(&self, auth: &AuthId, args: &StorageProcessActionArgs) -> WalletResult<StorageProcessActionResult>;
    async fn internalize_action(&self, auth: &AuthId, args: &StorageInternalizeActionArgs) -> WalletResult<StorageInternalizeActionResult>;
    async fn abort_action(&self, auth: &AuthId, args: &AbortActionArgs) -> WalletResult<AbortActionResult>;

    async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)>;
    async fn find_or_insert_sync_state_auth(&self, auth: &AuthId, storage_identity_key: &str, storage_name: &str) -> WalletResult<(SyncState, bool)>;
    async fn insert_certificate_auth(&self, auth: &AuthId, certificate: &CertificateX) -> WalletResult<i64>;

    async fn find_certificates_auth(&self, auth: &AuthId, args: &FindCertificatesArgs) -> WalletResult<Vec<CertificateX>>;
    async fn find_output_baskets_auth(&self, auth: &AuthId, args: &FindOutputBasketsArgs) -> WalletResult<Vec<OutputBasket>>;
    async fn find_outputs_auth(&self, auth: &AuthId, args: &FindOutputsArgs) -> WalletResult<Vec<Output>>;
    async fn find_proven_tx_reqs(&self, args: &FindProvenTxReqsArgs) -> WalletResult<Vec<ProvenTxReq>>;

    async fn list_actions(&self, auth: &AuthId, args: &ValidListActionsArgs) -> WalletResult<ListActionsResult>;
    async fn list_outputs(&self, auth: &AuthId, args: &ValidListOutputsArgs) -> WalletResult<ListOutputsResult>;
    async fn list_certificates(&self, auth: &AuthId, args: &ValidListCertificatesArgs) -> WalletResult<ListCertificatesResult>;

    async fn relinquish_certificate(&self, auth: &AuthId, args: &RelinquishCertificateArgs) -> WalletResult<i64>;
    async fn relinquish_output(&self, auth: &AuthId, args: &RelinquishOutputArgs) -> WalletResult<i64>;
    async fn set_active(&self, auth: &AuthId, new_active_storage_identity_key: &str) -> WalletResult<i64>;

    async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk>;
    async fn process_sync_chunk(&self, args: &RequestSyncChunkArgs, chunk: &SyncChunk) -> WalletResult<ProcessSyncChunkResult>;
    async fn update_proven_tx_req_with_new_proven_tx(&self, args: &UpdateProvenTxReqWithNewProvenTxArgs) -> WalletResult<UpdateProvenTxReqWithNewProvenTxResult>;
}
```

### Pattern 2: AuthFetch Behind Mutex for Send + Sync

**What:** `AuthFetch<W>` requires `&mut self` to call `fetch()` (it mutates peer state for the BRC-31 handshake). `StorageClient` must be `Arc`-safe (`Send + Sync`). Wrap `AuthFetch<W>` in `tokio::sync::Mutex`.

**When to use:** Required. `StorageClient` needs to implement the `WalletStorageProvider` trait which requires `Send + Sync`, and it will be held as `Arc<dyn WalletStorageProvider>`.

**Trade-offs:** Mutex serializes all outbound HTTP calls through a single gate. For a wallet making 2-3 concurrent requests this is acceptable. It mirrors the pattern already used by `WalletStorageManager` for write serialization.

```rust
pub struct StorageClient<W: WalletInterface + Clone + 'static> {
    auth_fetch: Mutex<AuthFetch<W>>,
    endpoint_url: String,
    next_id: std::sync::atomic::AtomicU64,
    settings: Mutex<Option<Settings>>,
}

impl<W: WalletInterface + Clone + Send + Sync + 'static> StorageClient<W> {
    pub fn new(wallet: W, endpoint_url: String) -> Self {
        Self {
            auth_fetch: Mutex::new(AuthFetch::new(wallet)),
            endpoint_url,
            next_id: std::sync::atomic::AtomicU64::new(1),
            settings: Mutex::new(None),
        }
    }

    async fn rpc_call<T: DeserializeOwned>(&self, method: &str, params: serde_json::Value) -> WalletResult<T> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let body = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": id });
        let body_bytes = serde_json::to_vec(&body)?;
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        let mut fetch = self.auth_fetch.lock().await;
        let resp = fetch.fetch(&self.endpoint_url, "POST", Some(body_bytes), Some(headers)).await?;
        // parse resp.body as JSON-RPC response, extract result or propagate error
        ...
    }
}
```

### Pattern 3: Sync-Chunk Replication Replaces Direct CRUD Propagation

**What:** The current `WalletStorageManager` write propagation calls raw CRUD methods directly on the backup (`backup.update_output(...)`, `backup.update_transaction(...)`, etc.). This is incompatible with a remote `StorageClient` which does not expose CRUD. Replace the synchronous per-write backup propagation with periodic `getSyncChunk` / `processSyncChunk` replication driven by the existing sync module.

**When to use:** Required when the backup is a `StorageClient`. Optional (but cleaner) for local-to-local backup.

**Trade-offs:** Sync is eventually consistent rather than immediately consistent. Acceptable because: (a) the TS implementation uses the same model — the backup receives sync chunks, not live writes, and (b) the sync module already implements the full merge and ID-remapping protocol.

**Implication for WalletStorageManager:** The backup field should hold `Arc<dyn WalletStorageProvider>` not `Arc<dyn StorageProvider>`. The existing write-propagation code in manager should be conditionally removed or wrapped behind a `is_storage_provider()` check.

## Data Flow

### Action Flow (local active + remote backup)

```
Wallet::create_action(args)
    ↓
WalletStorageManager
    ↓ active.create_action(auth, args)
SqlxProvider (local SQLite)
    ↓ returns StorageCreateActionResult
    ↓ [no immediate backup write — backup is remote, sync is async]
WalletSigner builds & signs tx
    ↓
WalletStorageManager
    ↓ active.process_action(auth, args)
SqlxProvider (local SQLite)
    ↓ returns StorageProcessActionResult
    ↓ [sync scheduled async or on-demand via sync_backup]
```

### Sync Replication Flow (active → remote backup)

```
sync_backup(active: &Arc<dyn StorageProvider>, backup: &Arc<dyn WalletStorageProvider>, auth: &AuthId)
    ↓
active.get_sync_chunk(args)       ← local SQL query for changed entities
    ↓ SyncChunk
backup.get_sync_chunk(args)       ← remote RPC: "what do you have?"
    ↓ SyncChunk (backup's state)
merge/compare
    ↓
backup.process_sync_chunk(args, chunk)  ← remote RPC: "accept these entities"
    ↓ ProcessSyncChunkResult
loop until done=true
```

### JSON-RPC Wire Format (per TS reference)

```
POST endpoint_url
Content-Type: application/json
[BRC-31 auth headers injected by AuthFetch]

Body:
{
  "jsonrpc": "2.0",
  "method": "createAction",
  "params": [auth_id_object, create_action_args_object],
  "id": 42
}

Response:
{
  "jsonrpc": "2.0",
  "result": { ... },
  "id": 42
}
```

**Critical:** Parameter ordering in `params` array must exactly match the TS `StorageClient` method signatures. Method names use camelCase as in TS (e.g. `"createAction"`, `"getSyncChunk"`, `"findOrInsertUser"`).

### DateTime Handling

The TS server serializes timestamps as ISO-8601 strings. The Rust codebase already has `serde_datetime` helpers for this. `StorageClient::rpc_call` deserialization must route through the same `serde_datetime` mechanisms used for SQLx results to ensure `chrono::DateTime` fields round-trip correctly. This is the same "validateEntity/validateEntities" pattern in the TS `StorageClient`.

## Scaling Considerations

| Scale | Architecture Adjustments |
|-------|--------------------------|
| Single wallet | StorageClient with Mutex<AuthFetch> is sufficient — serialized HTTP calls are fine |
| Multiple wallets same process | Each Wallet instance owns its own WalletStorageManager and StorageClient — no sharing needed |
| High-frequency sync | Increase chunk size in RequestSyncChunkArgs; BRC-31 session reuse across calls (already handled by AuthFetch peers HashMap) |

## Anti-Patterns

### Anti-Pattern 1: Implementing StorageProvider on StorageClient

**What people do:** Try to make `StorageClient` implement the full `StorageProvider` (60+ method) trait to fit directly into the existing backup slot type.

**Why it's wrong:** The remote server does not expose 60+ CRUD endpoints. The `WalletStorageProvider` interface (~25 methods) is the correct level of abstraction. Implementing `StorageProvider` would require either (a) leaving most methods as `unimplemented!()` panics or (b) adding unsupported RPC endpoints to the server — both violate the TS contract.

**Do this instead:** Define `WalletStorageProvider` as a new trait, change the backup slot to `Arc<dyn WalletStorageProvider>`, and use a blanket impl so local providers implement both.

### Anti-Pattern 2: Sharing AuthFetch Across Wallet Instances

**What people do:** Put `AuthFetch<W>` in an `Arc<Mutex<...>>` shared between multiple `StorageClient` instances or wallets.

**Why it's wrong:** `AuthFetch` tracks per-base-URL peer sessions keyed to a specific wallet's identity. Sharing across wallets would cross-contaminate BRC-31 sessions.

**Do this instead:** Each `StorageClient` owns its own `AuthFetch` instance (in a `Mutex` for internal `&mut self` access).

### Anti-Pattern 3: Removing Direct Write Propagation Before Sync Is Wired

**What people do:** Remove the backup write propagation code in `WalletStorageManager` immediately when adding the remote backup type, expecting sync to cover it.

**Why it's wrong:** If sync is not wired yet, the backup will silently fall behind with no data. Local-to-local backup still benefits from synchronous propagation.

**Do this instead:** Keep the existing propagation for local `StorageProvider` backups. Add a runtime check: if `backup.is_storage_provider()` returns false (i.e., it is a `StorageClient`), skip direct CRUD propagation and rely on sync-chunk replication.

### Anti-Pattern 4: Panicking on DateTime Parse in RPC Responses

**What people do:** Deserialize RPC JSON responses directly into Rust structs without applying the `serde_datetime` validation pass.

**Why it's wrong:** The TS server may return dates as strings, numbers, or ISO-8601 depending on the database engine. The TS `StorageClient` runs `validateEntity` on all returned records. Without this, datetime fields will fail to deserialize or produce wrong values.

**Do this instead:** After `rpc_call` deserializes a response, normalize all `DateTime` fields using the same `serde_datetime` helpers used in the SQLx implementation.

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| TS StorageServer (storage.babbage.systems) | JSON-RPC POST over BRC-31 HTTPS | Primary production target; method names are camelCase strings |
| TS StorageServer (storage.b1nary.tech) | Same | Secondary production target |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| `StorageClient` ↔ `WalletStorageManager` | `Arc<dyn WalletStorageProvider>` in backup slot | New trait; blanket impl covers local providers |
| `StorageClient` ↔ `AuthFetch<W>` | `Mutex<AuthFetch<W>>` owned by `StorageClient` | Not shared; `W` is a concrete type bound |
| `sync module` ↔ `StorageClient` | `WalletStorageProvider::get_sync_chunk` / `process_sync_chunk` | Same trait methods work for local and remote |
| `WalletStorageManager` ↔ backup write propagation | Changed: CRUD propagation only when `is_storage_provider() == true` | Remote backup uses sync-chunk path |

## Build Order

The dependency chain requires this ordering to avoid circular compilation failures:

1. **`WalletStorageProvider` trait** (`src/storage/traits/wallet_storage.rs`)
   — Depends on: existing type structs (`AuthId`, `SyncChunk`, `Settings`, `FindCertificatesArgs`, etc.)
   — Must exist before `StorageClient` or manager changes can compile.

2. **Blanket impl of `WalletStorageProvider` for `StorageProvider` impls**
   — Add to `src/storage/traits/wallet_storage.rs` or `src/storage/traits/mod.rs`
   — `impl<T: StorageProvider> WalletStorageProvider for T` with delegation
   — Allows `SqlxProvider` and `WalletStorageManager` to satisfy the new trait without code changes.

3. **`StorageClient<W>` struct** (`src/storage/remoting/storage_client.rs`)
   — Depends on: `WalletStorageProvider` trait, `AuthFetch<W>` from bsv-sdk
   — Implements `WalletStorageProvider` via `rpc_call` helper
   — Generic over `W: WalletInterface + Clone + Send + Sync + 'static`

4. **`WalletStorageManager` backup field type change**
   — Change `backup: Option<Arc<dyn StorageProvider>>` to `Option<Arc<dyn WalletStorageProvider>>`
   — Update `add_wallet_storage_provider` signature
   — Add `is_storage_provider()` guard around CRUD propagation paths

5. **Integration tests against live TS servers**
   — `StorageClient::new(wallet, "https://storage.babbage.systems")` round-trip test
   — `make_available()` → `create_action()` → `process_action()` flow

## Sources

- Direct source: `/Users/elisjackson/Projects/rust-wallet-toolbox/src/storage/manager.rs` (lines 57-110) — backup field type
- Direct source: `/Users/elisjackson/Projects/rust-wallet-toolbox/src/storage/traits/provider.rs` — StorageProvider trait (11 methods)
- Direct source: `node_modules/@bsv/wallet-toolbox/src/storage/remoting/StorageClient.ts` — TS reference implementation (530 lines)
- Direct source: `node_modules/@bsv/wallet-toolbox/src/sdk/WalletStorage.interfaces.ts` — WalletStorageProvider, WalletStorageWriter, WalletStorageReader hierarchy
- Direct source: `~/.cargo/registry/.../bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` — `fetch(&mut self, ...)` signature confirmed
- Confirmed: `reqwest = "0.12"` already in Cargo.toml; no new HTTP dependency needed

---
*Architecture research for: rust-wallet-toolbox StorageClient integration*
*Researched: 2026-03-24*
