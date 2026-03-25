---
phase: 05-manager-orchestration
plan: 01
subsystem: storage
tags: [rust, tokio, sqlite, async, sync, conflict-resolution]

# Dependency graph
requires:
  - phase: 04-manager-rewrite
    provides: WalletStorageManager with sync infrastructure (sync_to_writer, do_make_available, acquire_sync, update_backups, reprove_header, reprove_proven, get_stores)
provides:
  - pub async fn set_active on WalletStorageManager with 8-step TS-parity conflict resolution
  - Fix for find_or_insert_sync_state_auth: random ref_num satisfies UNIQUE constraint
  - Tests for all 5 ORCH requirements
affects: [06-integration-testing, 07-e2e]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - Clone Arc refs and settings from state lock before releasing, then do async I/O outside lock
    - Use do_make_available() inside acquire_sync guards to avoid reader_lock deadlock
    - Generate random ref_num (base64 of 12 random bytes) matching TS randomBytesBase64(12)

key-files:
  created: []
  modified:
    - src/storage/manager.rs
    - src/storage/traits/wallet_provider.rs
    - tests/manager_tests.rs

key-decisions:
  - "set_active uses do_make_available() NOT make_available() at re-partition step: acquire_sync already holds reader_lock, calling make_available() would deadlock"
  - "find_or_insert_sync_state_auth generates random ref_num using rand+base64 to satisfy UNIQUE constraint on sync_states.refNum (TS parity: randomBytesBase64(12))"
  - "test_set_active_noop requires first calling set_active to establish user.active_storage: fresh user has active_storage='' which does not equal store sik"

requirements-completed: [ORCH-01, ORCH-02, ORCH-03, ORCH-04, ORCH-05]

# Metrics
duration: 8min
completed: 2026-03-25
---

# Phase 5 Plan 01: Manager Orchestration Summary

**WalletStorageManager.set_active() with 8-step TS-parity conflict merge, plus fix for sync_states.refNum UNIQUE constraint bug**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-03-25T11:27:48Z
- **Completed:** 2026-03-25T11:35:55Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Implemented `pub async fn set_active()` on `WalletStorageManager` matching the TypeScript 8-step algorithm: auto-init, validate sik, early-return noop, acquire sync lock, merge conflicts via `sync_to_writer`, determine backup_source, provider-level `set_active`, propagate to all stores, reset and re-partition via `do_make_available`
- Fixed pre-existing bug in `find_or_insert_sync_state_auth`: `ref_num` was initialized to empty string, causing UNIQUE constraint failures on second sync state insert per provider; now generates 12 random bytes encoded as base64
- Added 4 `set_active` tests covering all cases, plus `test_reprove_proven_no_services` for ORCH-04 coverage; all 5 ORCH requirements now have passing tests

## Task Commits

1. **Task 1: Implement set_active with conflict resolution and tests** - `172bdbd` (feat)
2. **Task 2: Verify ORCH-02 through ORCH-05 tests are green** - `940bfb6` (test)

## Files Created/Modified
- `src/storage/manager.rs` - Added `set_active` method (~180 lines) after `update_backups`
- `src/storage/traits/wallet_provider.rs` - Fixed `ref_num` generation in `find_or_insert_sync_state_auth`; added `use base64::Engine as _` import
- `tests/manager_tests.rs` - Added 4 `test_set_active_*` tests and `test_reprove_proven_no_services`

## Decisions Made
- Used `do_make_available()` (not `make_available()`) at re-partition step: `acquire_sync` already holds `reader_lock`, calling `make_available()` would deadlock
- Generated `ref_num` as base64(rand 12 bytes) matching TS `randomBytesBase64(12)` — this is a correctness fix, not an arbitrary choice
- `test_set_active_noop` first calls `set_active` once to establish `user.active_storage`, then verifies the second call returns "unchanged" — this correctly models the TS noop condition

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed empty ref_num causing UNIQUE constraint violation in sync_states**
- **Found during:** Task 1 (test_set_active_with_conflicts)
- **Issue:** `find_or_insert_sync_state_auth` initialized `ref_num` to `String::new()`. The `sync_states` table has `refNum TEXT NOT NULL UNIQUE`. When a single provider needs two sync state records (syncing from two different readers), the second insert fails with `UNIQUE constraint failed: sync_states.refNum`
- **Fix:** Generate 12 random bytes and encode as base64 standard (`rand::thread_rng().fill_bytes` + `base64::engine::general_purpose::STANDARD.encode`), matching TS `randomBytesBase64(12)` exactly
- **Files modified:** `src/storage/traits/wallet_provider.rs`
- **Verification:** `test_set_active_with_conflicts` passes; full test suite green
- **Committed in:** `172bdbd` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - bug)
**Impact on plan:** Essential correctness fix. Without it, any multi-provider scenario with `set_active` (or any scenario syncing from more than one reader into the same writer) would fail with a UNIQUE constraint error.

## Issues Encountered
- `test_set_active_noop` precondition was wrong: `is_active_enabled()` returns false for fresh stores because `user.active_storage` is `""` by default, not the store's sik. Fixed the test to call `set_active` once first to establish the active_storage, then verify the noop.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All 5 ORCH requirements have passing tests
- `set_active` completes the WalletStorageManager orchestration API (TS parity)
- Phase 6 integration testing can proceed: full manager API is available

---
*Phase: 05-manager-orchestration*
*Completed: 2026-03-25*
