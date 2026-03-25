# Phase 5: Manager Orchestration - Research

**Researched:** 2026-03-25
**Domain:** Rust async orchestration, multi-provider conflict resolution, WalletStorageManager setActive
**Confidence:** HIGH

## Summary

Phase 5 is a focused implementation phase. ORCH-02 through ORCH-05 were built ahead of schedule during Phase 4 execution and need only verification. The only net-new implementation is ORCH-01: `set_active()` manager-level orchestration.

The TS `setActive(storageIdentityKey, progLog?)` method does four things in sequence, inside a sync-level lock: (1) validates the target store exists, (2) if conflicting actives are present, merges state from each conflicting active into the new active via `syncToWriter`, (3) calls provider-level `storage.setActive()` on the "backup source" (either `newActive` if conflicts existed, or `_active` otherwise) to persist `user.activeStorage = storageIdentityKey` in the database, then propagates that state to all other stores via `syncToWriter`, and (4) resets `_isAvailable = false` and re-runs `makeAvailable()` to re-partition.

The Rust implementation already has all the infrastructure needed: `acquire_sync()`, `sync_to_writer()`, `sync_from_reader()`, `make_available()` / `do_make_available()`, provider-level `set_active()` on `WalletStorageProvider`, `ManagerState` with `conflicting_active_indices`, and cached `user`/`settings` per store. The task is assembling these into one new `pub async fn set_active()` method on `WalletStorageManager`.

**Primary recommendation:** Implement `set_active` as a single method that acquires the sync lock, performs conflict merging if needed, calls provider-level `set_active` on the backup source, propagates state to all stores, then resets `is_available_flag` to false and re-runs `do_make_available()`.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| ORCH-01 | setActive() full conflict resolution — detect conflicts, merge via syncToWriter, update user.activeStorage, re-partition | TS source read directly (WalletStorageManager.ts:797-887); Rust infrastructure verified in manager.rs — all building blocks present |
| ORCH-02 | updateBackups() fan-out sync to all backup stores with per-backup error handling | **Already implemented** at manager.rs:1522. Verify against TS parity. |
| ORCH-03 | reproveHeader() re-validates proofs against orphaned headers using ChainTracker | **Already implemented** at manager.rs:1297. Verify against TS parity. |
| ORCH-04 | reproveProven() re-validates individual ProvenTx proofs against current chain | **Already implemented** at manager.rs:1213. Verify against TS parity. |
| ORCH-05 | getStores() returns WalletStorageInfo for all providers with full status metadata | **Already implemented** at manager.rs:1116. Verify against TS parity. |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio::sync::Mutex | tokio 1.x | Hierarchical lock guards | Already in use; `acquire_sync()` already exists |
| Arc<dyn WalletStorageProvider> | std | Type-erased provider access | Phase 2/4 established pattern |
| WalletError / WalletResult | project | Error propagation | Consistent throughout manager |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tracing::warn / info | 0.1 | Progress logging | Mirror TS progLog pattern for setActive steps |

**Installation:** No new dependencies — all primitives already in Cargo.toml.

## Architecture Patterns

### ORCH-01: set_active Algorithm (TS-Parity Translation)

The TS `setActive` translates to Rust as follows:

```
pub async fn set_active(
    &self,
    storage_identity_key: &str,
    prog_log: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
) -> WalletResult<String>
```

**Step-by-step (matching TS lines 797-887):**

**Step 0: Auto-init**
Ensure `make_available()` has been called. TS does `if (!this.isAvailable()) await this.makeAvailable()`.
Rust: call `self.make_available().await?` if `!self.is_available()`.

**Step 1: Validate target store**
Find the store index whose `settings.storage_identity_key == storage_identity_key`.
TS: `this._stores.findIndex(...)`, throws `WERR_INVALID_PARAMETER` if not found.
Rust: iterate `state.stores`, lock each `settings` mutex, compare. Return `WalletError::InvalidParameter` if no match.

**Step 2: Early return if already active and enabled**
TS: `if (storageIdentityKey === this.getActiveStore() && this.isActiveEnabled) return log + ' unchanged'`.
Rust: compare `storage_identity_key` with active store's sik AND check `is_active_enabled()`.

**Step 3: Acquire sync lock**
TS: `this.runAsSync(async sync => { ... })`.
Rust: `let _guards = self.acquire_sync().await?;`

**Step 4: Conflict resolution (if conflicting actives exist)**
TS logic inside the sync lock:
```typescript
if (this._conflictingActives!.length > 0) {
    // Push current active into conflicting actives temporarily
    this._conflictingActives!.push(this._active!)
    // Remove newActive from conflicting actives list
    this._conflictingActives = this._conflictingActives!.filter(ca =>
        ca.settings!.storageIdentityKey !== storageIdentityKey
    )
    // Sync each conflicting active into newActive
    for (const conflict of this._conflictingActives) {
        await this.syncToWriter(auth, newActive.storage, conflict.storage, ...)
    }
}
```

Rust translation:
1. Lock `state` mutex.
2. If `!state.conflicting_active_indices.is_empty()`:
   a. Build a temporary list of "conflict sources": all `conflicting_active_indices` + `active_index` - `new_active_index`.
   b. For each conflict source: call `self.sync_to_writer(new_active_storage, conflict_storage, new_active_settings, conflict_settings, prog_log).await?` (note: `sync_to_writer` does NOT hold state lock — release state before calling).
3. Collect Arc clones of relevant stores BEFORE releasing `state` lock, then release and call sync methods.

**Step 5: Determine backup_source**
TS: `const backupSource = this._conflictingActives!.length > 0 ? newActive : this._active!`
Rust: if conflicts existed → backup_source = new_active; else → backup_source = current_active.

**Step 6: Provider-level setActive on backup_source**
TS: `await backupSource.storage.setActive({ identityKey, userId: backupSource.user!.userId }, storageIdentityKey)`
Rust:
```rust
let auth = AuthId {
    identity_key: self.identity_key.clone(),
    user_id: Some(backup_source_user_id),
    is_active: false,
};
backup_source_storage.set_active(&auth, storage_identity_key).await?;
```
This writes `user.active_storage = storage_identity_key` to the backup_source's database.

**Step 7: Propagate to all other stores**
TS:
```typescript
for (const store of this._stores) {
    store.user!.activeStorage = storageIdentityKey  // update cache
    if (store.settings!.storageIdentityKey !== backupSource.settings!.storageIdentityKey) {
        await this.syncToWriter(auth, store.storage, backupSource.storage, ...)
    }
}
```
Rust translation:
1. Lock state, update every `store.user.lock().await.as_mut().map(|u| u.active_storage = storage_identity_key.to_string())` for all stores.
2. For each store whose `settings.sik != backup_source_sik`: call `self.sync_to_writer(store_arc, backup_source_arc, store_settings, backup_source_settings, prog_log).await?` (release state lock first).

**Step 8: Re-partition**
TS: `this._isAvailable = false; await this.makeAvailable()`
Rust:
```rust
self.is_available_flag.store(false, Ordering::Release);
// do_make_available() is the internal version that doesn't re-acquire reader_lock
// but we're already inside acquire_sync which holds reader+writer+sync.
// Re-use do_make_available() which operates directly on state.
self.do_make_available().await?;
```

**CRITICAL LOCK CONCERN:** `do_make_available()` locks `self.state`. The `set_active` method will need to release the state mutex before calling any `sync_to_writer` operations (which internally call storage methods), and re-acquire it for cache updates and re-partition. The pattern established in `update_backups` is: lock state → clone Arc refs and settings → drop state lock → do async work → done.

### sync_to_writer vs sync_from_reader in setActive

The TS uses `syncToWriter(auth, writer, reader, ...)`. The Rust `sync_to_writer(writer, reader, writer_settings, reader_settings, prog_log)` maps directly.

In Step 4 (conflict merging): `sync_to_writer(new_active, conflict, new_active_settings, conflict_settings, ...)` — merging conflict INTO new_active (new_active is the writer receiving state).

In Step 7 (propagation): `sync_to_writer(store, backup_source, store_settings, backup_source_settings, ...)` — pushing backup_source state INTO each other store.

Note: `sync_from_reader` (which has the `active_storage` override protection) is NOT used in `setActive`. The TS `syncToWriter` used in `setActive` does NOT include the chunk.user.activeStorage override — that override is only in `syncFromReader`. The Rust `sync_to_writer` (without override) is the correct method to call here.

### Lock Interaction: acquire_sync + do_make_available

`acquire_sync()` acquires reader + writer + sync locks. `do_make_available()` does NOT acquire any of the four named locks — it only acquires `self.state` (the ManagerState mutex). This means it is safe to call `do_make_available()` from inside an `acquire_sync()` guard — no deadlock risk.

However: `make_available()` (public) acquires reader_lock internally. Calling `make_available()` from inside `acquire_sync()` WILL deadlock because acquire_sync already holds reader_lock. Use `do_make_available()` directly in the re-partition step.

### Anti-Patterns to Avoid

- **Holding state mutex across sync_to_writer calls:** `sync_to_writer` calls provider methods that are async. Holding `self.state.lock()` while awaiting storage I/O will block all other manager operations. Clone Arc refs and settings, release state lock, then do I/O.
- **Calling make_available() inside acquire_sync:** Deadlocks — reader_lock is already held. Use `do_make_available()` directly.
- **Forgetting to update cached user.active_storage:** After provider-level `set_active` succeeds, all `ManagedStorage.user.active_storage` fields must be updated in-memory (the cache). Otherwise `is_active_enabled()` returns stale results.
- **Calling sync_to_writer with prog_log inside state lock:** Same deadlock risk as above. Extract prog_log before locking state, do all I/O outside the state lock.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Conflict detection | Manual iteration | `state.conflicting_active_indices` | Already maintained by `do_make_available()` |
| State re-partition | Re-implement partition logic | `do_make_available()` | Already correctly handles the 3-step partition |
| Sync loop | Custom chunk iteration | `sync_to_writer()` | Already implemented with full loop and progLog |
| User record update | Direct DB write | `set_active()` on `WalletStorageProvider` | Provider-level method handles DB persistence |

## Common Pitfalls

### Pitfall 1: State Lock Held Across Async I/O
**What goes wrong:** `self.state.lock().await` is held while calling `sync_to_writer()` or `set_active()` — other manager operations stall indefinitely.
**Why it happens:** Natural pattern of locking then doing work.
**How to avoid:** Collect all needed `Arc<dyn WalletStorageProvider>` clones and `Settings` clones while holding state lock, then drop the lock before any `.await` on storage operations.
**Warning signs:** Tests that run two concurrent operations on the manager hang indefinitely.

### Pitfall 2: make_available() vs do_make_available() Inside Sync Lock
**What goes wrong:** `set_active` holds the sync lock (reader+writer+sync), then calls `make_available()` which tries to acquire reader_lock again — deadlock.
**Why it happens:** `make_available()` uses reader_lock as its idempotency guard. Calling it while reader_lock is held re-acquires the same lock.
**How to avoid:** At the re-partition step, call `do_make_available()` directly (internal method), NOT `make_available()`.
**Warning signs:** `set_active` test hangs after the sync step; never returns.

### Pitfall 3: Wrong sync_to_writer Direction
**What goes wrong:** In the conflict merge step, calling `sync_to_writer(conflict, new_active, ...)` instead of `sync_to_writer(new_active, conflict, ...)` — pushes new_active state into the conflict store instead of the reverse.
**Why it happens:** Writer/reader argument order is easy to swap.
**How to avoid:** The writer is the DESTINATION (receives data), the reader is the SOURCE. In conflict merge: new_active is the writer (receives merged state from conflicts). In propagation: each other store is the writer (receives state from backup_source).
**Warning signs:** After `set_active`, the new active store has stale data rather than merged state.

### Pitfall 4: Cached user.active_storage Not Updated Before Re-partition
**What goes wrong:** After calling provider-level `set_active` and syncing, `do_make_available()` re-reads DB values and re-partitions. If the in-memory cache is not updated first, the partition may see the old `active_storage` and re-classify stores incorrectly.
**Why it happens:** TS explicitly updates `store.user!.activeStorage = storageIdentityKey` for all stores before re-partition. Rust must do the same for the `ManagedStorage.user` Mutex cache.
**How to avoid:** After the propagation loop (Step 7), before calling `do_make_available()`, update each `store.user.lock().await` to set `active_storage = storage_identity_key`. Actually, since `do_make_available()` re-queries `find_or_insert_user()` from each store's database, the cache will be updated from DB anyway. TS updates the in-memory cache as an optimization between the loop and re-partition. Rust can rely on `do_make_available()` re-fetching — but must ensure the propagation syncs have completed so the DB state is correct.
**Warning signs:** After `set_active`, `get_stores()` still shows old active or incorrect `is_enabled`.

### Pitfall 5: WERR_INVALID_PARAMETER for Unknown storageIdentityKey
**What goes wrong:** `setActive` receives a `storage_identity_key` not registered with any store — should return `WalletError::InvalidParameter`, but silently no-ops.
**Why it happens:** Missing validation step.
**How to avoid:** Always validate the target store exists (Step 1) before doing any work. Match TS `WERR_INVALID_PARAMETER('storageIdentityKey', ...)`.

## Code Examples

Verified patterns from existing codebase:

### Pattern: Clone Arcs from State, Then Release Lock
```rust
// Source: manager.rs:1536 (update_backups pattern)
let (active_arc, active_settings, backup_arcs_and_settings) = {
    let state = self.state.lock().await;
    let idx = state.require_active()?;
    let active_arc = state.stores[idx].storage.clone();
    let active_settings = state.stores[idx]
        .get_settings_cached()
        .await
        .ok_or_else(|| WalletError::InvalidOperation("Active settings not cached".to_string()))?;
    // ... collect other arcs/settings
    (active_arc, active_settings, ...)
    // state lock drops here
};
// Now do async I/O with cloned arcs, outside the state lock
```

### Pattern: Provider-level set_active Call
```rust
// Source: WalletStorageProvider trait, wallet_provider.rs:299
// set_active takes an AuthId and the new storage_identity_key
let auth = AuthId {
    identity_key: self.identity_key.clone(),
    user_id: Some(backup_source_user_id),
    is_active: false,
};
backup_source_storage.set_active(&auth, storage_identity_key).await?;
```

### Pattern: sync_to_writer with prog_log
```rust
// Source: manager.rs:1375 — sync_to_writer signature
// writer = destination (receives data); reader = source (provides data)
self.sync_to_writer(
    writer_arc.as_ref(),
    reader_arc.as_ref(),
    &writer_settings,
    &reader_settings,
    prog_log,
).await?;
```

### Pattern: do_make_available for Re-partition Inside Lock
```rust
// Re-partition after setActive: use do_make_available(), NOT make_available()
// make_available() would try to re-acquire reader_lock (already held by acquire_sync)
self.is_available_flag.store(false, Ordering::Release);
self.do_make_available().await?;
```

### Pattern: Update Cached user.active_storage
```rust
// Update all store user caches before re-partition
let mut state = self.state.lock().await;
for store in &state.stores {
    let mut user_guard = store.user.lock().await;
    if let Some(ref mut u) = *user_guard {
        u.active_storage = storage_identity_key.to_string();
    }
}
// drop state — do_make_available will re-fetch from DB anyway but cache update
// prevents stale data during the window between update and re-partition
```

### Pattern: WERR_INVALID_PARAMETER for Unknown Store
```rust
// Source: TS WalletStorageManager.ts:804
// Find the index of the target store
let new_active_idx = {
    let state = self.state.lock().await;
    let mut found = None;
    for (i, store) in state.stores.iter().enumerate() {
        let sik = store.get_settings_cached().await
            .map(|s| s.storage_identity_key)
            .unwrap_or_default();
        if sik == storage_identity_key {
            found = Some(i);
            break;
        }
    }
    found
};
let new_active_idx = new_active_idx.ok_or_else(|| WalletError::InvalidParameter {
    parameter: "storage_identity_key".to_string(),
    must_be: format!(
        "registered with this WalletStorageManager. {} does not match any managed store.",
        storage_identity_key
    ),
})?;
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Single active, no conflict concept | conflicting_active_indices tracked in ManagerState | Phase 4 | setActive can detect and resolve conflicts |
| Per-op backup write propagation | sync_to_writer chunked sync | Phase 4 | setActive uses the same sync infrastructure |
| No manager-level setActive | Need to implement set_active() | Phase 5 | Completes the TS-parity manager API |

## Open Questions

1. **do_make_available() accessibility from set_active**
   - What we know: `do_make_available()` is a private method on `WalletStorageManager` (no `pub`). `set_active()` is also on `WalletStorageManager`, so it has access.
   - What's unclear: Nothing — same impl block, private access is fine.
   - Recommendation: No change needed.

2. **Conflict-merge direction when new active IS the current active**
   - What we know: TS handles this in lines 828-834: when conflicts exist, it pushes `_active` into `_conflictingActives` and removes `newActive` from the list. This means if current active == new active, the current active gets pushed into conflicts and immediately removed (net: current active stays as new active, conflicts are drained into it).
   - What's unclear: Whether this edge case needs explicit special-casing in Rust.
   - Recommendation: Follow TS logic exactly — build the full conflict source list (conflicting_active_indices + active_index, minus new_active_index) and iterate it. This handles the edge case naturally.

3. **Early return when already active and enabled**
   - What we know: TS early-returns `log + ' unchanged\n'` if `storageIdentityKey === this.getActiveStore() && this.isActiveEnabled`.
   - What's unclear: Whether `is_active_enabled()` is already accessible without a lock in the right call sequence.
   - Recommendation: `is_active_enabled()` is `pub async fn` on manager (manager.rs:433) — call it directly before acquiring sync lock.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + tokio test (tokio 1.x) |
| Config file | Cargo.toml `[dev-dependencies]` |
| Quick run command | `cargo test --features sqlite set_active 2>&1` |
| Full suite command | `cargo test --features sqlite 2>&1` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| ORCH-01 | set_active switches active store, merges conflicts, re-partitions | unit | `cargo test --features sqlite set_active` | ❌ Wave 0 |
| ORCH-01 | set_active returns WERR_INVALID_PARAMETER for unknown sik | unit | `cargo test --features sqlite set_active_invalid` | ❌ Wave 0 |
| ORCH-01 | set_active no-ops when already active and enabled | unit | `cargo test --features sqlite set_active_noop` | ❌ Wave 0 |
| ORCH-02 | update_backups syncs to all backups, per-backup error isolation | unit | `cargo test --features sqlite update_backups` | ✅ (verify existing) |
| ORCH-03 | reprove_header re-validates proofs | unit | `cargo test --features sqlite reprove_header` | ✅ (verify existing) |
| ORCH-04 | reprove_proven re-validates individual proven tx | unit | `cargo test --features sqlite reprove_proven` | ✅ (verify existing) |
| ORCH-05 | get_stores returns correct WalletStorageInfo for all providers | unit | `cargo test --features sqlite get_stores` | ✅ (verify existing) |

### Sampling Rate
- **Per task commit:** `cargo test --features sqlite 2>&1`
- **Per wave merge:** `cargo test --features sqlite 2>&1`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `tests/manager_tests.rs` — add `test_set_active_*` tests covering ORCH-01 (basic switch, conflict resolution, no-op, invalid sik)
- [ ] Shared test helper: mock `WalletStorageProvider` that tracks `set_active` calls (or use existing SqliteStorage via in-memory SQLite)

## Sources

### Primary (HIGH confidence)
- TS source: `/Users/elisjackson/Projects/zed/music-streaming/key-server/node_modules/@bsv/wallet-toolbox/src/storage/WalletStorageManager.ts` lines 793-887 — full `setActive` method read
- Rust source: `src/storage/manager.rs` — full file structure, `sync_to_writer` (line 1375), `update_backups` (line 1522), `do_make_available` (line 550), `acquire_sync` (line 681), `ManagerState` struct (line 158)
- Rust source: `src/storage/traits/wallet_provider.rs` lines 296-303 — `set_active` trait method signature

### Secondary (MEDIUM confidence)
- Phase 4 RESEARCH.md — lock hierarchy semantics, pitfall catalogue (verified against codebase)

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- ORCH-01 algorithm: HIGH — TS source read directly, line-by-line translation verified against existing Rust infrastructure
- ORCH-02 through ORCH-05: HIGH — already implemented in Phase 4 (manager.rs:1116, 1213, 1297, 1522); confirmed by grep
- Lock interaction (acquire_sync + do_make_available): HIGH — code read directly; do_make_available does not acquire named locks, only state Mutex
- Pitfalls: HIGH — derive from TS algorithm analysis and existing lock patterns documented in Phase 4

**Research date:** 2026-03-25
**Valid until:** 2026-04-25 (stable Rust async patterns; TS source is a local node_modules snapshot)
