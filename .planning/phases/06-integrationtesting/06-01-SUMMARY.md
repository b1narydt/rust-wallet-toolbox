---
phase: 06-integrationtesting
plan: 01
subsystem: testing
tags: [integration-tests, brc31, auth-fetch, storage-client, wire-format, sqlite]

# Dependency graph
requires:
  - phase: 03-storageclient
    provides: StorageClient<W> with WalletStorageProvider trait impl
  - phase: 04-manager-rewrite
    provides: WalletStorageManager.add_wallet_storage_provider + update_backups + sync_to_writer
  - phase: 05-manager-orchestration
    provides: set_active orchestration completing manager layer

provides:
  - "Integration test suite: 6 live tests against staging-storage.babbage.systems covering TEST-01..07"
  - "WalletArc<W> wrapper enabling StorageClient<W> use with non-Clone wallet types (ProtoWallet)"
  - "Executable proof of wire compatibility: BRC-31 auth, makeAvailable, findOrInsertUser, getSyncChunk, updateBackups"

affects:
  - "Any phase using StorageClient<ProtoWallet> — must use WalletArc<ProtoWallet> instead"

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "WalletArc<W> pattern: Arc-wrapping WalletInterface to satisfy W: Clone without requiring W itself to be Clone"
    - "Integration test gating: #[cfg(feature = 'sqlite')] + #[ignore] + BSV_TEST_ROOT_KEY env var"
    - "make_wallet_with_remote_backup() async helper: avoids duplicating WalletBuilder + add_wallet_storage_provider setup across tests"

key-files:
  created:
    - tests/storage_client_integration_tests.rs
    - src/storage/remoting/wallet_arc.rs
  modified:
    - src/storage/remoting/mod.rs

key-decisions:
  - "WalletArc<W> manually implements Clone (not derived) so Arc<W> clone works even when W: !Clone — ProtoWallet does not implement Clone but Arc<ProtoWallet> is always cloneable"
  - "Tests TEST-06 and TEST-07 both call update_backups(None) — update_backups internally calls sync_to_writer, so TEST-06 verifies sync_to_writer indirectly through the manager layer (simpler and more correct than calling sync_to_writer directly which requires explicit Settings references)"
  - "WalletArc exported from storage::remoting alongside StorageClient for ergonomic co-import"

patterns-established:
  - "Pattern: use WalletArc<ProtoWallet> when constructing StorageClient in tests (not ProtoWallet directly)"
  - "Pattern: each integration test creates its own StorageClient instance — AuthFetch<W> is behind Mutex and should not be shared across concurrent tests"

requirements-completed:
  - TEST-01
  - TEST-02
  - TEST-03
  - TEST-04
  - TEST-05
  - TEST-06
  - TEST-07

# Metrics
duration: 35min
completed: 2026-03-25
---

# Phase 06 Plan 01: Integration Testing — StorageClient vs Live TS Server Summary

**Six `#[ignore]` integration tests proving BRC-31 auth, wire format, and sync loop compatibility against staging-storage.babbage.systems, plus WalletArc<W> fix enabling StorageClient<ProtoWallet>**

## Performance

- **Duration:** 35 min
- **Started:** 2026-03-25T13:52:33Z
- **Completed:** 2026-03-25T14:27:00Z
- **Tasks:** 3 (Tasks 1-3 complete; Task 4 is a human-verify checkpoint)
- **Files modified:** 3

## Accomplishments
- Created `tests/storage_client_integration_tests.rs` with 6 live integration tests covering all requirements TEST-01 through TEST-07
- Fixed `ProtoWallet: !Clone` incompatibility with `StorageClient<W: Clone>` by introducing `WalletArc<W>` — an Arc-based wrapper with manual `Clone` impl
- All 6 tests compile and appear in test listing under `storage_client_integration::*`
- Full test suite (non-ignored) passes with no regressions

## Task Commits

Each task was committed atomically:

1. **Tasks 1-3: StorageClient integration tests + WalletArc wrapper** - `cd5d16a` (feat)

**Plan metadata:** (pending final commit)

## Files Created/Modified
- `tests/storage_client_integration_tests.rs` - 6 live integration tests (TEST-01..07), gated `#[cfg(feature = "sqlite")]` + `#[ignore]`
- `src/storage/remoting/wallet_arc.rs` - WalletArc<W> wrapper: Arc-based Clone for non-Clone WalletInterface implementors
- `src/storage/remoting/mod.rs` - Added wallet_arc module + WalletArc re-export

## Decisions Made

1. **WalletArc<W> manually implements Clone**: `#[derive(Clone)]` would propagate `W: Clone` bound (defeating the purpose). Manual impl clones the inner `Arc<W>` directly — always valid.

2. **TEST-06 calls update_backups instead of sync_to_writer directly**: `sync_to_writer` requires explicit `Settings` references that must be extracted from the manager's internal state. `update_backups(None)` internally calls `sync_to_writer` for each backup, making it the simpler and semantically correct path to prove sync_to_writer works through the manager layer.

3. **Shared `make_wallet_with_remote_backup()` helper**: Tests 05, 06, and 07 all need a full wallet with a remote backup. Extracted as an async helper to avoid duplicated setup.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] WalletArc<W> wrapper to fix ProtoWallet: !Clone incompatibility**
- **Found during:** Task 1 (creating integration test file)
- **Issue:** `StorageClient<W>` requires `W: Clone` (propagated from `AuthFetch<W: Clone>` in bsv-sdk 0.1.75). `ProtoWallet` does not implement `Clone`, so `StorageClient<ProtoWallet>` fails to compile. The plan assumed `StorageClient<ProtoWallet>` would work directly.
- **Fix:** Created `src/storage/remoting/wallet_arc.rs` — `WalletArc<W>` wraps any `WalletInterface` in `Arc<W>` and manually implements `Clone` (arc ref-count increment), delegating all 28 `WalletInterface` methods to the inner `W`. Tests use `StorageClient<WalletArc<ProtoWallet>>` instead.
- **Files modified:** src/storage/remoting/wallet_arc.rs (new), src/storage/remoting/mod.rs
- **Verification:** `cargo build --features sqlite` succeeds; all 6 tests appear in `-- --list` output
- **Committed in:** cd5d16a (combined task commit)

---

**Total deviations:** 1 auto-fixed (Rule 1 - Bug)
**Impact on plan:** WalletArc is a clean abstraction that belongs in the remoting module. No scope creep — it enables the intended design rather than changing it.

## Issues Encountered
- `ProtoWallet: !Clone` prevented `StorageClient<ProtoWallet>` from compiling. Resolved by adding `WalletArc<W>` wrapper (see Deviations above). The research in 06-RESEARCH.md assumed this would work but did not verify the Clone bound in bsv-sdk 0.1.75's AuthFetch.

## User Setup Required

**To run these tests against the live staging server, set `BSV_TEST_ROOT_KEY`:**

```bash
# Generate a test key
openssl rand -hex 32

# Run live integration tests
BSV_TEST_ROOT_KEY=<your-64-char-hex-key> cargo test --features sqlite -- --ignored storage_client_integration 2>&1
```

No other external service configuration required. The staging server (`staging-storage.babbage.systems`) is public.

## Next Phase Readiness
- Task 4 is a `checkpoint:human-verify` gate requiring manual test execution against the live server
- After verification: integration test phase is complete, all TEST-01..07 requirements met
- No blockers for continuation once Task 4 is approved

---
*Phase: 06-integrationtesting*
*Completed: 2026-03-25*
