# Phase 4: Manager Rewrite - Research

**Researched:** 2026-03-24
**Domain:** Rust async concurrency, multi-provider storage management, chunked sync loops
**Confidence:** HIGH

## Summary

Phase 4 replaces the existing `WalletStorageManager` (single active + optional single backup via `Arc<dyn StorageProvider>`) with a TS-parity manager that uses `Arc<dyn WalletStorageProvider>` — the narrower interface from Phase 2. The new manager wraps each provider in a `ManagedStorage` struct with cached metadata and implements a four-level hierarchical lock system to serialize reader/writer/sync/storage_provider access.

The core challenge is translating the TS queue-based lock pattern (`readerLocks`, `writerLocks`, `syncLocks`, `spLocks` arrays of resolve callbacks) into idiomatic Rust using `tokio::sync` primitives. A semaphore-based approach per level (with ordered acquisition) provides equivalent semantics without the JavaScript promise-queue idiom.

The sync loop in `syncToWriter` / `syncFromReader` iterates `getSyncChunk` → `processSyncChunk` until `done`, driven by `EntitySyncState::fromStorage` which creates `RequestSyncChunkArgs` from the stored sync state. The Rust equivalent is a helper function that queries `find_or_insert_sync_state_auth` and builds `RequestSyncChunkArgs` from the result.

**Primary recommendation:** Implement the hierarchical lock with four `tokio::sync::Mutex<()>` acquired in strict order (reader → writer → sync → storage_provider). Use a `ManagedStorage` inner struct with `RwLock`-protected cache fields. Keep `WalletStorageManager` as a complete replacement for the old manager in `src/storage/manager.rs`.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| MGR-01 | WalletStorageManager constructor accepts identity_key + optional active + Vec of backup WalletStorageProviders | TS constructor: `new(identityKey, active?, backups?)` — first entry becomes `_stores[0]`, rest are backups. Rust: `new(identity_key: String, active: Option<Arc<dyn WalletStorageProvider>>, backups: Vec<Arc<dyn WalletStorageProvider>>)` |
| MGR-02 | ManagedStorage wrapper per provider caching settings, user, is_available, is_storage_provider | TS `class ManagedStorage { isAvailable, isStorageProvider, settings?, user? }`. Rust: struct with `Arc<dyn WalletStorageProvider>`, `Mutex<Option<Settings>>`, `Mutex<Option<User>>`, `AtomicBool is_available`, `bool is_storage_provider` (set at construction) |
| MGR-03 | makeAvailable() partitions stores into active/backups/conflicting_actives | TS logic: stores[0] is default active; swap if a later store's `user.active_storage == settings.storage_identity_key` and current active is not enabled; backups whose `user.active_storage != active.settings.storage_identity_key` are `conflicting_actives` |
| MGR-04 | Four-level hierarchical lock (reader < writer < sync < storage_provider) with ordered acquisition | TS uses queue arrays of resolve-callbacks. Rust equivalent: four `tokio::sync::Mutex<()>` acquired in order reader→writer→sync→sp. reader only acquires reader lock; writer acquires reader+writer; sync acquires reader+writer+sync; sp acquires all four |
| MGR-05 | syncToWriter() and syncFromReader() chunked sync loops using EntitySyncState + RequestSyncChunkArgs iteration | TS loop: `EntitySyncState::fromStorage(writer, identityKey, readerSettings)` → `ss.makeRequestSyncChunkArgs(...)` → `reader.getSyncChunk(args)` → `writer.processSyncChunk(args, chunk)` until `r.done`. Rust helper replaces EntitySyncState class |
| MGR-06 | CRUD write propagation replaced with chunk-based sync — writes to active only, backups sync via updateBackups() | Old manager propagated individual writes to backup. New manager: writes go to active via `run_as_writer`; `update_backups()` calls `sync_to_writer` for each backup under sync lock |
| MGR-07 | All manager-level delegation methods with appropriate lock acquisition and auth checks | TS: readers use `runAsReader`, writers use `runAsWriter`, sync ops use `runAsSync`; auth checked via `getAuth(mustBeActive)`. Rust: same pattern with `run_as_reader/writer/sync` closures |
| MGR-08 | addWalletStorageProvider() runtime addition with re-partition | TS: `makeAvailable()` on provider, push to `_stores`, reset `_isAvailable = false`, call `makeAvailable()`. Rust: equivalent async method |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio::sync::Mutex | tokio 1.x | Hierarchical lock guards | Already in project; async-aware; no std::Mutex deadlock risk across .await |
| async-trait | 0.1 | Async trait methods | Already in project; required for dyn WalletStorageProvider |
| Arc<dyn WalletStorageProvider> | std | Type-erased provider storage | Phase 2 established this as the canonical multi-provider interface |
| std::sync::atomic::AtomicBool | std | is_available flag | Phase 3 pattern: AtomicBool for sync bool that doesn't need blocking |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tokio::sync::RwLock | tokio 1.x | Shared read access for cached settings/user | If hot-path reads of cached metadata need concurrency; Mutex is simpler if cache is only written during makeAvailable |
| tracing | 0.1 | Structured logging for sync progress | Already used in current manager |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Four separate Mutex<()> locks | Single Mutex<LockLevel> with condition vars | TS model is four independent queues; four Mutexes maps cleanly; condition vars add complexity |
| tokio::sync::Mutex for cache | std::sync::RwLock for cache | Cache is written during makeAvailable (which is async); tokio Mutex safer; std RwLock risks deadlock if held across .await |

**Installation:** No new dependencies — all primitives are already in Cargo.toml.

## Architecture Patterns

### Recommended Project Structure
```
src/storage/
├── manager.rs              # WalletStorageManager (REPLACE entire file)
│   ├── ManagedStorage      # Inner struct (private)
│   ├── WalletStorageManager # Public struct
│   └── helpers             # make_request_sync_chunk_args, run_as_* closures
└── (no new files needed)
```

### Pattern 1: ManagedStorage Inner Struct
**What:** Wraps a `WalletStorageProvider` with cached metadata and runtime flags.
**When to use:** One per provider added to the manager.

```rust
struct ManagedStorage {
    storage: Arc<dyn WalletStorageProvider>,
    is_storage_provider: bool,           // set at construction from storage.is_storage_provider()
    is_available: AtomicBool,            // becomes true after makeAvailable() succeeds
    settings: tokio::sync::Mutex<Option<Settings>>,
    user: tokio::sync::Mutex<Option<User>>,
}

impl ManagedStorage {
    fn new(storage: Arc<dyn WalletStorageProvider>) -> Self {
        let is_sp = storage.is_storage_provider();
        Self {
            storage,
            is_storage_provider: is_sp,
            is_available: AtomicBool::new(false),
            settings: tokio::sync::Mutex::new(None),
            user: tokio::sync::Mutex::new(None),
        }
    }
}
```

### Pattern 2: Four-Level Hierarchical Lock
**What:** Four ordered mutexes that enforce reader < writer < sync < storage_provider access hierarchy.
**When to use:** All public async delegation methods acquire the appropriate level.

```rust
pub struct WalletStorageManager {
    stores: Mutex<Vec<ManagedStorage>>,    // guards the partition state
    auth_id: Mutex<AuthId>,
    // Four-level lock — must be acquired in this strict order
    reader_lock: tokio::sync::Mutex<()>,
    writer_lock: tokio::sync::Mutex<()>,
    sync_lock: tokio::sync::Mutex<()>,
    sp_lock: tokio::sync::Mutex<()>,
    is_available: AtomicBool,
    services: Mutex<Option<Arc<dyn WalletServices>>>,
}

// Lock acquisition helpers (private):
// run_as_reader: acquires reader_lock only
// run_as_writer: acquires reader_lock + writer_lock
// run_as_sync: acquires reader_lock + writer_lock + sync_lock
// run_as_storage_provider: acquires all four locks
```

**Critical:** RAII guards must be held for the duration of the closure. In Rust, acquire reader guard first, then writer guard (inside the reader guard's scope), etc. Release in reverse order automatically via Drop.

**TS parity note:** TS uses `getActiveLock(queue)` which auto-resolves if the queue was empty (first waiter). The Rust `tokio::sync::Mutex` has exactly this behaviour: `lock().await` either returns immediately (unlocked) or waits (locked). Semantics match.

### Pattern 3: Chunked Sync Loop
**What:** Iterates getSyncChunk → processSyncChunk until `done`, driven by stored sync state.
**When to use:** `sync_to_writer` and `sync_from_reader`.

```rust
// Rust equivalent of EntitySyncState::fromStorage + makeRequestSyncChunkArgs
async fn load_sync_state_args(
    writer: &dyn WalletStorageProvider,
    identity_key: &str,
    reader_settings: &Settings,
    writer_settings_identity_key: &str,
) -> WalletResult<RequestSyncChunkArgs> {
    let (sync_state, _) = writer
        .find_or_insert_sync_state_auth(
            &AuthId { identity_key: identity_key.to_string(), user_id: None, is_active: false },
            &reader_settings.storage_identity_key,
            &reader_settings.storage_name,
        )
        .await?;
    make_request_sync_chunk_args(&sync_state, identity_key, writer_settings_identity_key)
}

// sync loop body (called from inside run_as_sync closure):
loop {
    let args = load_sync_state_args(writer, identity_key, &reader_settings, &writer_sik).await?;
    let chunk = reader.get_sync_chunk(&args).await?;
    let r = writer.process_sync_chunk(&args, &chunk).await?;
    inserts += r.inserts;
    updates += r.updates;
    if r.done { break; }
}
```

### Pattern 4: makeAvailable Partition Logic
**What:** Iterates all stores, calls `storage.make_available()` + `find_or_insert_user()`, then partitions active/backups/conflicting_actives.
**When to use:** Called on construction (lazily) and after `add_wallet_storage_provider`.

Partition algorithm (translated directly from TS):
1. Iterate stores; first store is default `active`.
2. If a later store's `user.active_storage == store.settings.storage_identity_key` AND current active is not enabled → swap: push old active to backups, new store becomes active.
3. Otherwise push to backups.
4. Second pass: partition backups — if `backup.user.active_storage != active.settings.storage_identity_key` → move to `conflicting_actives`.
5. Set `is_available = true`, populate `auth_id.user_id` and `auth_id.is_active`.

**is_active_enabled logic:**
```
active.settings.storage_identity_key == active.user.active_storage
    AND conflicting_actives is empty
```

### Anti-Patterns to Avoid

- **Holding std::sync::MutexGuard across .await:** Deadlocks the Tokio executor. Use `tokio::sync::Mutex` for all async-context guards.
- **Acquiring locks out of order:** Must always be reader → writer → sync → sp. Out-of-order acquisition risks deadlock when two tasks contend.
- **Cloning Arc<dyn WalletStorageProvider> for lock access:** The closure pattern passes the provider by reference from the locked guard. Don't clone to escape the lock — keep work inside the closure.
- **Re-implementing EntitySyncState class:** The TS class is a thin wrapper over SyncState table + SyncMap JSON. In Rust, use a free function `make_request_sync_chunk_args(sync_state: &SyncState, ...) -> RequestSyncChunkArgs` instead. The `SyncState` table struct already exists in `src/tables/sync_state.rs`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Async mutual exclusion | Custom future-based lock queue | `tokio::sync::Mutex<()>` | Tokio Mutex IS the TS promise-queue pattern, implemented correctly with fairness |
| Sync state management | Custom per-provider sync cursor | `SyncState` table + `find_or_insert_sync_state_auth` | Already in WalletStorageProvider trait; TS uses same pattern |
| Chunk pagination | Custom offset tracking | `SyncChunkOffset` in `RequestSyncChunkArgs` | Already defined in `src/storage/sync/request_args.rs` |
| Manager-level auth | Custom identity resolution | `AuthId` + `find_or_insert_user` in makeAvailable | User/identity pattern already established in Phase 3 |

**Key insight:** Every building block for Phase 4 already exists — `WalletStorageProvider`, `RequestSyncChunkArgs`, `SyncState`, `SyncChunk`, `ProcessSyncChunkResult`, `AuthId`. Phase 4 is assembly, not invention.

## Common Pitfalls

### Pitfall 1: Lock Order Inversion
**What goes wrong:** Two concurrent tasks: Task A acquires writer_lock then tries sync_lock; Task B holds sync_lock and tries writer_lock. Deadlock.
**Why it happens:** Lock helpers acquired in different orders in different code paths.
**How to avoid:** All `run_as_*` helpers must acquire locks in strict ascending order: reader < writer < sync < sp. Never acquire a lower-level lock after a higher-level one. The TS code enforces this by always chaining `getActiveLock` calls in the same order.
**Warning signs:** Tests hang indefinitely; tokio deadlock detector (if enabled) fires.

### Pitfall 2: Stores Vec Mutation During Async Operations
**What goes wrong:** `add_wallet_storage_provider` pushes to the stores vec while `makeAvailable` is iterating it. Data race or incorrect partition.
**Why it happens:** The stores vec is shared state; concurrent access without proper guarding.
**How to avoid:** Wrap `stores`, `active`, `backups`, `conflicting_actives` partition state in a single `tokio::sync::Mutex<ManagerState>` inner struct. The sync/writer locks gate async operations; the ManagerState lock gates structural mutation.
**Warning signs:** makeAvailable sees inconsistent store counts after runtime add.

### Pitfall 3: makeAvailable Idempotency
**What goes wrong:** `makeAvailable` called concurrently from two tasks — both see `!is_available`, both run the expensive initialization, race on setting partition state.
**Why it happens:** The `is_available` check and the initialization are not atomic.
**How to avoid:** Wrap the entire `makeAvailable` body inside a `Mutex<()>` guard (the reader_lock is sufficient — makeAvailable needs write access). TS checks `if (this._isAvailable) return this._active!.settings!` at top, inside the queued lock.
**Warning signs:** Duplicate `find_or_insert_user` calls during concurrent startup.

### Pitfall 4: Sync Loop Termination Condition
**What goes wrong:** `processSyncChunk` returns `done: false` forever because `RequestSyncChunkArgs.since` is never updated from the sync_state.
**Why it happens:** TS updates the `EntitySyncState.when` from `ProcessSyncChunkResult.maxUpdated_at`. The Rust `make_request_sync_chunk_args` helper must use `sync_state.when` as `args.since`, and `update_backups` must persist the sync state after each chunk (or at minimum after done).
**Warning signs:** Sync loop runs indefinitely, same entities returned every iteration.

### Pitfall 5: activeStorage Field in Incoming Chunk User Record
**What goes wrong:** Syncing from a reader clobbers the writer's `user.active_storage` with the reader's value.
**Why it happens:** `processSyncChunk` merges the user record including `active_storage`. TS explicitly overrides: `chunk.user.activeStorage = this._active!.user!.activeStorage` before calling `processSyncChunk`.
**How to avoid:** In `sync_from_reader`, after getting the chunk and before calling `process_sync_chunk`, override `chunk.user.active_storage` with the writer's cached `user.active_storage`.
**Warning signs:** After sync, the active storage selection is wrong; makeAvailable re-partitions incorrectly.

### Pitfall 6: run_as_* and Trait Object Restriction
**What goes wrong:** Trying to use `run_as_writer(|writer| async move { ... })` where the closure returns a future that borrows `writer` — conflicts with the `async_trait` / dyn dispatch lifetimes.
**Why it happens:** Rust closures returning futures that borrow captured references have complex lifetime requirements. `Box<dyn Future>` is needed.
**How to avoid:** Use `run_as_writer` with a function-pointer-style helper or accept `Box<dyn Future>`. The simplest pattern is to not use a closure-based API: acquire the lock guard, call the method, release the guard explicitly. This is more verbose than TS but avoids lifetime complexity.
**Warning signs:** Compiler error "borrowed value does not live long enough" inside run_as_* closures.

## Code Examples

Verified patterns from existing codebase:

### AuthId Construction (from Phase 3 pattern)
```rust
// Source: src/wallet/types.rs + Phase 3 established pattern
use crate::wallet::types::AuthId;

let auth = AuthId {
    identity_key: identity_key.to_string(),
    user_id: Some(user_id),
    is_active: true,
};
```

### RequestSyncChunkArgs from SyncState (Rust translation of EntitySyncState.makeRequestSyncChunkArgs)
```rust
// Source: EntitySyncState.ts + src/storage/sync/request_args.rs
use crate::storage::sync::request_args::{RequestSyncChunkArgs, SyncChunkOffset};
use crate::tables::SyncState;

fn make_request_sync_chunk_args(
    ss: &SyncState,
    for_identity_key: &str,
    to_storage_identity_key: &str,
) -> RequestSyncChunkArgs {
    // Parse sync_map JSON to get per-entity counts for offsets
    // sync_state.sync_map is a JSON string matching SyncMap structure
    let offsets = parse_sync_map_offsets(&ss.sync_map);
    RequestSyncChunkArgs {
        identity_key: for_identity_key.to_string(),
        max_rough_size: 10_000_000,
        max_items: 1000,
        offsets,
        since: ss.when,
        from_storage_identity_key: ss.storage_identity_key.clone(),
        to_storage_identity_key: to_storage_identity_key.to_string(),
    }
}
```

### Hierarchical Lock Acquisition (RAII pattern)
```rust
// Writer acquires reader + writer locks in order
async fn run_as_writer<F, Fut, R>(&self, f: F) -> WalletResult<R>
where
    F: FnOnce(/* active ref */) -> Fut,
    Fut: Future<Output = WalletResult<R>>,
{
    let _reader_guard = self.reader_lock.lock().await;
    let _writer_guard = self.writer_lock.lock().await;
    // Both guards held; released in reverse order when function returns
    let active = self.active_storage()?;
    f(active).await
}
```

### Sync Loop Pattern
```rust
// Inside run_as_sync closure:
let reader_settings = reader.get_settings().await?;
let mut inserts = 0i64;
let mut updates = 0i64;
loop {
    let ss = writer
        .find_or_insert_sync_state_auth(&auth, &reader_settings.storage_identity_key, &reader_settings.storage_name)
        .await?;
    let args = make_request_sync_chunk_args(&ss.0, &identity_key, &writer_settings.storage_identity_key);
    let chunk = reader.get_sync_chunk(&args).await?;
    let r = writer.process_sync_chunk(&args, &chunk).await?;
    inserts += r.inserts;
    updates += r.updates;
    if r.done { break; }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Single Arc<dyn StorageProvider> active + Option backup | Vec<ManagedStorage> wrapping Arc<dyn WalletStorageProvider> | Phase 4 | N backup providers, not just one |
| Single tokio::sync::Mutex<()> for all writes | Four-level hierarchical Mutex (reader/writer/sync/sp) | Phase 4 | Sync operations block writers; readers don't block each other |
| Write propagation (per-op backup writes) | Chunk-based sync via updateBackups() | Phase 4 | Backup sync is eventual, not immediate; supports remote StorageClient backups |
| No makeAvailable partitioning | Explicit active/backup/conflicting partition | Phase 4 | Enables setActive and conflict resolution |

**Deprecated/outdated patterns in manager.rs:**
- `Arc<dyn StorageProvider>` fields — replaced with `Arc<dyn WalletStorageProvider>`
- `backup: Option<...>` field — replaced with `Vec<ManagedStorage>`
- Per-operation backup propagation (inline `if let Some(ref backup)` patterns) — removed

## Open Questions

1. **SyncMap JSON round-trip in make_request_sync_chunk_args**
   - What we know: `SyncState.sync_map` is a JSON string; TS `EntitySyncState` parses it as `SyncMap` and uses per-entity `count` as offsets
   - What's unclear: The Rust `SyncMap` struct in `sync_map.rs` has per-entity `EntitySyncMap` entries with `max_updated_at` and `id_map`. The offset field (`count`) mapping to `SyncChunkOffset.offset` needs to be confirmed.
   - Recommendation: Read `src/storage/sync/sync_map.rs` during planning to verify which field in `EntitySyncMap` maps to the `SyncChunkOffset.offset` value. The TS uses `ess.count` — confirm Rust equivalent.

2. **AuthId struct fields: user_id nullability**
   - What we know: `AuthId` in `src/wallet/types.rs` is used in Phase 3; `is_active` and `user_id` are populated after makeAvailable
   - What's unclear: Whether `user_id` is `Option<i64>` or `i64` in the Rust struct — needed for constructing auth in manager methods before makeAvailable has set it
   - Recommendation: Read `src/wallet/types.rs` at plan time; adjust `getAuth(mustBeActive)` equivalent accordingly.

3. **WalletStorageManager trait implementation**
   - What we know: TS `WalletStorageManager` implements `sdk.WalletStorage` — the full wallet storage interface. Rust `WalletStorageManager` currently has inherent methods only.
   - What's unclear: Whether Phase 4 should have `WalletStorageManager` implement `WalletStorageProvider` or remain inherent-method only (Wallet calls manager methods directly)
   - Recommendation: Check how `Wallet` struct uses `WalletStorageManager` in the codebase. If Wallet holds `Arc<dyn WalletStorageProvider>` and expects a single interface, implement the trait on manager. If Wallet holds `WalletStorageManager` directly, inherent methods suffice. Phase 4 goal is matching TS architecture; the manager likely stays as inherent methods consumed by the Wallet struct.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + tokio test (tokio 1.x) |
| Config file | Cargo.toml `[dev-dependencies]` |
| Quick run command | `cargo test --features sqlite manager_tests 2>&1` |
| Full suite command | `cargo test --features sqlite 2>&1` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MGR-01 | Constructor accepts identity_key + optional active + Vec backups | unit | `cargo test --features sqlite manager_tests::test_constructor` | ❌ Wave 0 |
| MGR-02 | ManagedStorage wraps provider with cached settings, user, is_available | unit | `cargo test --features sqlite manager_tests::test_managed_storage_cache` | ❌ Wave 0 |
| MGR-03 | makeAvailable() partitions stores correctly | unit | `cargo test --features sqlite manager_tests::test_make_available_partition` | ❌ Wave 0 |
| MGR-04 | Four-level lock: writers block readers of lower level | unit | `cargo test --features sqlite manager_tests::test_hierarchical_locks` | ❌ Wave 0 |
| MGR-05 | syncToWriter/syncFromReader loop until done | unit | `cargo test --features sqlite manager_tests::test_sync_to_writer` | ❌ Wave 0 |
| MGR-06 | Writes go to active only; updateBackups syncs | unit | `cargo test --features sqlite manager_tests::test_update_backups` | ❌ Wave 0 |
| MGR-07 | Delegation methods: createAction, findCertificates, etc. | unit | `cargo test --features sqlite manager_tests::test_delegation_methods` | ❌ Wave 0 |
| MGR-08 | addWalletStorageProvider adds at runtime, re-partitions | unit | `cargo test --features sqlite manager_tests::test_add_provider_runtime` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --features sqlite manager_tests 2>&1`
- **Per wave merge:** `cargo test --features sqlite 2>&1`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `tests/manager_tests.rs` — existing file uses OLD manager API; needs full rewrite to match new WalletStorageProvider-based interface; covers MGR-01 through MGR-08
- [ ] Helper `tests/common/` may need a `make_wallet_provider` helper returning `Arc<dyn WalletStorageProvider>` from SqliteStorage

*(The existing `tests/manager_tests.rs` exists but tests the OLD manager — it will need to be replaced entirely as part of Phase 4 implementation.)*

## Sources

### Primary (HIGH confidence)
- TS source: `/Users/elisjackson/Projects/zed/music-streaming/key-server/node_modules/@bsv/wallet-toolbox/src/storage/WalletStorageManager.ts` — full manager implementation read lines 1-900
- TS source: `EntitySyncState.ts` — `fromStorage`, `makeRequestSyncChunkArgs` methods read
- Rust source: `src/storage/manager.rs` — existing manager read fully
- Rust source: `src/storage/traits/wallet_provider.rs` — WalletStorageProvider trait + blanket impl read fully
- Rust source: `src/storage/sync/request_args.rs`, `sync_map.rs`, `process_sync_chunk.rs` — sync infrastructure read
- Rust source: `src/tables/settings.rs`, `user.rs`, `sync_state.rs` — table structs read

### Secondary (MEDIUM confidence)
- Tokio docs (well-known): `tokio::sync::Mutex` semantics match JS promise queues; acquire order = fifo fairness

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all dependencies already in project; no new crates needed
- Architecture: HIGH — TS source read directly; translation to Rust idiomatic patterns verified against existing codebase patterns
- Pitfalls: HIGH — lock ordering pitfalls verified against tokio docs; sync loop termination verified against TS source; activeStorage override pitfall identified from explicit TS code comment

**Research date:** 2026-03-24
**Valid until:** 2026-04-24 (stable domain; tokio API changes are rare)
