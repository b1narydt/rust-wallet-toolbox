---
phase: 04-manager-rewrite
plan: 02
subsystem: storage-manager
tags: [manager, sync, multi-provider, locks, process-sync-chunk, delegation]
dependency_graph:
  requires:
    - phase: 04-manager-rewrite/04-01
      provides: WalletStorageManager foundation with ManagedStorage, hierarchical locks, makeAvailable
    - phase: 04-manager-rewrite/04-03
      provides: make_request_sync_chunk_args, sync_to_writer, sync_from_reader, update_backups, add_wallet_storage_provider (pre-implemented by Plan 03 agent)
  provides:
    - chunked sync loop via sync_to_writer and sync_from_reader
    - update_backups fans out sync to all backup stores
    - process_sync_chunk sets done=true and persists SyncState with updated when timestamp
    - make_request_sync_chunk_args handles empty/initial sync_map correctly
    - delegation methods acquire hierarchical reader/writer locks
    - add_wallet_storage_provider adds provider at runtime and re-partitions
  affects: [sync-tests, wallet-lifecycle, storage-client, phase-05-orchestration]
tech_stack:
  added: []
  patterns:
    - "sync loop: loop { find_or_insert_sync_state_auth + get_sync_chunk + process_sync_chunk until done=true }"
    - "done detection: chunk_is_empty check at blanket impl level, not inside free function"
    - "SyncState.when high-watermark: advanced to now() after each chunk to prevent re-fetching"
    - "sync_map persistence: blanket process_sync_chunk serializes updated SyncMap to JSON and calls update_sync_state"
    - "lock acquisition: reader methods use acquire_reader(), writer methods use acquire_writer()"
key_files:
  created: []
  modified:
    - src/storage/manager.rs
    - src/storage/find_args.rs
    - src/storage/sqlx_impl/update.rs
    - src/storage/traits/wallet_provider.rs
    - tests/manager_tests.rs
key-decisions:
  - "done=true detected at blanket WalletStorageProvider impl level (not inside free process_sync_chunk function) — blanket impl has access to DB for SyncState lookup, free function does not"
  - "SyncState.when advanced to chrono::Utc::now() after each non-empty chunk — ensures next iteration's since filter excludes already-processed records"
  - "when field added to SyncStatePartial and update_sync_state SQL impls to support SyncState high-watermark persistence"
  - "Empty sync_map JSON ('{}') falls back to SyncMap::new() in make_request_sync_chunk_args — avoids serde error on initial sync where no JSON has been written yet"
  - "Delegation methods acquire hierarchical locks to match TS runAsReader/Writer semantics"

requirements-completed: [MGR-05, MGR-06, MGR-07, MGR-08]

duration: 52min
completed: 2026-03-25
---

# Phase 04 Plan 02: Sync Loops and Delegation Methods Summary

**Chunked sync replication via getSyncChunk/processSyncChunk loop with SyncState persistence, active_storage override protection, and hierarchical lock acquisition on all delegation methods**

## Performance

- **Duration:** 52 min
- **Started:** 2026-03-24T23:47:36Z
- **Completed:** 2026-03-25T00:40:00Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments

- Fixed infinite loop bug in sync protocol — `process_sync_chunk` now detects empty chunks and returns `done=true`
- SyncState high-watermark (`when`) is now persisted after each chunk so the next iteration skips already-processed records
- All read delegation methods acquire reader lock; all write methods acquire reader + writer locks
- 10 new TDD tests verify sync behavior, prog_log callbacks, update_backups, and add_wallet_storage_provider

## Task Commits

1. **Task 1: Sync loops and make_request_sync_chunk_args** - `de08be4` (feat) — fixes process_sync_chunk, adds when to SyncStatePartial, TDD tests
2. **Task 2: Delegation methods with lock semantics** - `cd9b16d` (feat) — reader/writer lock acquisition on all delegation methods

## Files Created/Modified

- `src/storage/manager.rs` - sync_to_writer, sync_from_reader, update_backups, add_wallet_storage_provider, upgraded delegation methods with lock acquisition (pre-committed by Plan 03 agent; lock upgrade in this plan)
- `src/storage/find_args.rs` - added `when: Option<NaiveDateTime>` to SyncStatePartial for SyncState high-watermark persistence
- `src/storage/sqlx_impl/update.rs` - added `when` column update to update_sync_state_impl (SQLite and macro-generated MySQL/PostgreSQL)
- `src/storage/traits/wallet_provider.rs` - fixed blanket process_sync_chunk: chunk empty detection, done=true, SyncState persistence
- `tests/manager_tests.rs` - 10 new TDD tests (tests 15-21) for sync functionality

## Decisions Made

- `done=true` is determined at the `WalletStorageProvider` blanket impl level, not inside the free `process_sync_chunk` function. The blanket impl has access to the DB connection for SyncState lookup and update; the free function only has the SyncMap in memory.
- SyncState.when is advanced to `chrono::Utc::now()` after each non-empty chunk. This is slightly after the entity data timestamps, ensuring the next `get_sync_chunk` call with `since=when` correctly excludes all already-synced records.
- The `when` field is added to `SyncStatePartial` and both the SQLite and macro-generated (MySQL/PostgreSQL) `update_sync_state_impl` functions.
- Empty/initial sync_map JSON (`"{}"` or `""`) falls back to `SyncMap::new()` to avoid a serde deserialization error on the first sync.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] process_sync_chunk never set done=true — infinite loop**
- **Found during:** Task 1 (sync_to_writer test hung indefinitely)
- **Issue:** The `ProcessSyncChunkResult.done` field defaulted to `false` and was never set. The blanket impl did not detect when a chunk was empty, causing `sync_to_writer` to loop forever.
- **Fix:** Added chunk-empty detection in blanket `process_sync_chunk`: checks all entity lists + chunk.user. Sets `done=true` when chunk is empty.
- **Files modified:** `src/storage/traits/wallet_provider.rs`
- **Verification:** test_sync_to_writer, test_sync_from_reader_matching_identity, test_sync_prog_log all pass
- **Committed in:** de08be4

**2. [Rule 1 - Bug] SyncState.when never updated — re-fetches same data on every iteration**
- **Found during:** Task 1 (test hung even after done=true fix — second iteration returned same data)
- **Issue:** After processing a chunk, the SyncState `when` field in the DB was never updated. The next `make_request_sync_chunk_args` call used `since=None` (null), causing `get_sync_chunk` to return all records again.
- **Fix:** Added SyncState DB persistence in blanket `process_sync_chunk`: serializes updated sync_map to JSON and calls `update_sync_state` with `when=now()` after each non-empty chunk.
- **Files modified:** `src/storage/find_args.rs`, `src/storage/sqlx_impl/update.rs`, `src/storage/traits/wallet_provider.rs`
- **Verification:** Sync loop terminates after one iteration for single-user DB
- **Committed in:** de08be4

**3. [Rule 3 - Blocking] "when" is reserved SQL keyword — syntax error**
- **Found during:** Task 1 (SQL error when running test after initial fix)
- **Issue:** `UPDATE sync_states SET when = ?` fails with SQLite syntax error because `when` is a reserved keyword.
- **Fix:** Quoted the column as `"when"` in the SQLite impl and used `$qc_fn("when")` (the database-appropriate quoting function) in the macro-generated MySQL/PostgreSQL impl.
- **Files modified:** `src/storage/sqlx_impl/update.rs`
- **Verification:** No more SQL syntax errors
- **Committed in:** de08be4

---

**Total deviations:** 3 auto-fixed (2 bugs, 1 blocking)
**Impact on plan:** All three fixes were required for the sync loop to function correctly. No scope creep.

## Issues Encountered

- Plan 03 agent had pre-implemented most of Plan 02's features (sync methods, add_wallet_storage_provider) but documented the infinite loop as a deferred item in `deferred-items.md`. Plan 02 execution resolved this deferred work plus upgraded the delegation methods with proper lock semantics.

## Next Phase Readiness

- Sync protocol fully functional end-to-end: loop iterates until done, SyncState persists watermark, active_storage override prevents cross-contamination
- All 21 manager tests pass, full test suite clean (no failures)
- Phase 05 (orchestration) can now rely on sync_to_writer/update_backups working correctly

---
*Phase: 04-manager-rewrite*
*Completed: 2026-03-25*
