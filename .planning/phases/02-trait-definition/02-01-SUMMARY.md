---
phase: 02-trait-definition
plan: 01
subsystem: storage
tags: [rust, async-trait, blanket-impl, wallet-storage, rpc, sync]

# Dependency graph
requires:
  - phase: 01-wire-format
    provides: SyncChunk/SyncMap wire types, serde_datetime helpers

provides:
  - WalletStorageProvider async trait with 25 methods matching TS WalletStorageProvider interface hierarchy
  - Blanket impl over StorageProvider so SqliteStorage satisfies WalletStorageProvider with zero source changes
  - RequestSyncChunkArgs / SyncChunkOffset wire-format structs for Phase 3 JSON-RPC
  - AuthId extended with user_id and is_active optional fields for Phase 3 serialization
  - Standalone abort_action storage function (extracted from WalletStorageManager)

affects:
  - 03-storage-client (implements WalletStorageProvider directly)
  - 04-manager-rewrite (accepts Arc<dyn WalletStorageProvider>)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Blanket impl pattern: impl<T: StorageProvider> WalletStorageProvider for T {} (mirrors StorageActionProvider precedent)"
    - "Narrowed trait pattern: WalletStorageProvider is a standalone trait, not supertrait of StorageProvider — allows StorageClient to implement it without implementing StorageProvider"
    - "Auth scoping: blanket impl methods that take &AuthId extract .identity_key for delegation to StorageProvider methods taking &str"

key-files:
  created:
    - src/storage/traits/wallet_provider.rs
    - src/storage/sync/request_args.rs
    - src/storage/methods/abort_action.rs
  modified:
    - src/wallet/types.rs
    - src/storage/traits/mod.rs
    - src/storage/mod.rs
    - src/storage/sync/mod.rs
    - src/storage/methods/mod.rs
    - src/wallet/wallet.rs
    - tests/trait_tests.rs

key-decisions:
  - "WalletStorageProvider is standalone (not supertrait of StorageProvider) — StorageClient can implement it without implementing all table-level CRUD"
  - "get_settings declared async in WalletStorageProvider trait (not sync) — blanket impl delegates to StorageProvider::get_settings(self, None) avoiding need for sync cache"
  - "find_or_insert_sync_state_auth implemented inline in blanket impl using find_sync_states + insert_sync_state — no helper method exists on StorageReaderWriter"
  - "abort_action extracted as standalone function in storage/methods/abort_action.rs — WalletStorageManager version uses manager lock which is not accessible from blanket impl"
  - "get_services returns WalletError::NotImplemented, set_services is no-op — matches TS StorageClient behavior, WalletServices is a separate Rust trait not on StorageProvider"

patterns-established:
  - "Blanket impl auth scoping: blanket impl methods extract auth.identity_key when delegating to StorageProvider methods taking &str"
  - "Wire-format separate structs: RequestSyncChunkArgs is a new owned struct distinct from internal GetSyncChunkArgs<'a> to avoid lifetime object-safety issues"

requirements-completed: [TRAIT-01, TRAIT-02, TRAIT-03]

# Metrics
duration: 9min
completed: 2026-03-24
---

# Phase 2 Plan 01: WalletStorageProvider Trait Definition Summary

**WalletStorageProvider async trait with 25-method TS-parity interface and blanket impl over StorageProvider using async_trait for Arc<dyn WalletStorageProvider> object safety**

## Performance

- **Duration:** 9 min
- **Started:** 2026-03-24T19:33:17Z
- **Completed:** 2026-03-24T19:42:34Z
- **Tasks:** 1
- **Files modified:** 10

## Accomplishments

- Defined `WalletStorageProvider` trait with 25 methods matching the TypeScript `WalletStorageProvider` interface hierarchy (WalletStorageReader → WalletStorageWriter → WalletStorageSync → WalletStorageProvider)
- Blanket impl `impl<T: StorageProvider> WalletStorageProvider for T` delegates 22+ methods to existing code — only `get_services` and `set_services` are NotImplemented (matching TS)
- `Arc<dyn WalletStorageProvider>` is object-safe and compiles; `SqliteStorage` satisfies the trait without any changes to sqlx_impl source files
- All 3 TDD tests pass: object safety, blanket impl satisfaction, `is_storage_provider()` returns true

## Task Commits

1. **Task 1: Define WalletStorageProvider trait with blanket impl** - `32851d0` (feat)

**Plan metadata:** (pending docs commit)

## Files Created/Modified

- `src/storage/traits/wallet_provider.rs` — WalletStorageProvider trait definition (25 methods) + blanket impl (744 lines)
- `src/storage/sync/request_args.rs` — RequestSyncChunkArgs and SyncChunkOffset wire-format structs
- `src/storage/methods/abort_action.rs` — Standalone abort_action function for blanket impl delegation
- `src/wallet/types.rs` — AuthId extended with user_id: Option<i64> and is_active: Option<bool> + serde camelCase derives
- `src/storage/traits/mod.rs` — Added pub mod wallet_provider; pub use wallet_provider::WalletStorageProvider
- `src/storage/mod.rs` — Added WalletStorageProvider to re-exports
- `src/storage/sync/mod.rs` — Added pub mod request_args; pub use request_args::*
- `src/storage/methods/mod.rs` — Added pub mod abort_action
- `src/wallet/wallet.rs` — Updated AuthId constructor to include user_id: None, is_active: None
- `tests/trait_tests.rs` — Added 3 new WalletStorageProvider tests

## Decisions Made

- `get_settings` declared `async fn` in `WalletStorageProvider` (matching the plan's recommendation to start async) — blanket impl delegates to `StorageProvider::get_settings(self, None).await`
- `find_or_insert_sync_state_auth` implemented inline in blanket impl using low-level `find_sync_states` + `insert_sync_state` since no `find_or_insert_sync_state` helper exists on `StorageReaderWriter`
- `abort_action` extracted as standalone function (deviation: plan specified using existing `WalletStorageManager::abort_action` logic but that requires manager lock — extracted the core logic as a standalone function instead)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] find_or_insert_sync_state method does not exist on StorageReaderWriter**
- **Found during:** Task 1 (blanket impl implementation)
- **Issue:** Plan specified delegating to `StorageReaderWriter::find_or_insert_sync_state` but this method does not exist on the trait — only `find_sync_states` (reader) and `insert_sync_state` (reader_writer) exist
- **Fix:** Implemented the find-or-insert logic inline in the blanket impl using `find_sync_states` + `insert_sync_state`
- **Files modified:** src/storage/traits/wallet_provider.rs
- **Verification:** Compiles, full test suite passes
- **Committed in:** 32851d0

---

**Total deviations:** 1 auto-fixed (Rule 1 - bug/missing method)
**Impact on plan:** Auto-fix necessary for compilation. Inline logic is semantically equivalent to what the plan specified. No scope creep.

## Issues Encountered

- `make_available` test had method ambiguity between `WalletStorageProvider::make_available` (blanket impl) and `StorageProvider::make_available` — resolved by disambiguating to `StorageProvider::make_available(&storage)` in test

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `WalletStorageProvider` trait ready for Phase 3 `StorageClient` to implement directly
- `RequestSyncChunkArgs` wire-format struct ready for Phase 3 JSON-RPC calls
- `AuthId` with optional `user_id` and `is_active` fields ready for Phase 3 JSON serialization
- All local storage tests pass, no regressions

## Self-Check: PASSED

All required files exist and commits are present in git history.

---
*Phase: 02-trait-definition*
*Completed: 2026-03-24*
