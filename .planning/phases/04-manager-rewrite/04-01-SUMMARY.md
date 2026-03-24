---
phase: 04-manager-rewrite
plan: 01
subsystem: storage-manager
tags: [manager, multi-provider, locks, make-available, wallet-storage-provider]
dependency_graph:
  requires: []
  provides: [WalletStorageManager, ManagedStorage, hierarchical-locks, make-available-partition]
  affects: [signer, monitor, wallet, setup, beef, storage-client]
tech_stack:
  added: []
  patterns:
    - "Arc<dyn WalletStorageProvider> as the narrowed provider interface"
    - "ManagedStorage wrapper caches settings/user per provider after make_available"
    - "Four tokio::sync::Mutex<()> locks in strict ordering (reader < writer < sync < sp)"
    - "auto-init pattern: acquire_reader triggers make_available if not yet available"
    - "Blanket impl: every StorageProvider satisfies WalletStorageProvider"
key_files:
  created:
    - tests/manager_tests.rs
  modified:
    - src/storage/manager.rs
    - src/storage/traits/wallet_provider.rs
    - src/storage/beef.rs
    - src/storage/sqlx_impl/trait_impls.rs
    - src/signer/methods/abort_action.rs
    - src/signer/methods/create_action.rs
    - src/signer/methods/sign_action.rs
    - src/signer/methods/internalize_action.rs
    - src/signer/default_signer.rs
    - src/monitor/helpers.rs
    - src/monitor/mod.rs
    - src/monitor/tasks/*.rs (all monitor tasks)
    - src/wallet/setup.rs
    - src/wallet/wallet.rs
    - tests/beef_tests.rs
    - tests/common/mod.rs
decisions:
  - "WalletStorageManager takes narrowed WalletStorageProvider (not StorageProvider) to support StorageClient in Phase 03"
  - "Signer methods accept &WalletStorageManager directly instead of &dyn StorageActionProvider (StorageActionProvider requires StorageProvider supertrait which manager doesn't satisfy)"
  - "beef.rs gets get_valid_beef_for_storage_reader variant using StorageReader trait, avoiding WalletStorageProvider impedance mismatch in storage methods"
  - "list_outputs blanket impl calls list_outputs_rw (not list_outputs) so specOp basket names like SPEC_OP_WALLET_BALANCE are handled"
  - "setup.rs creates 3 managers (SetupWallet.storage, WalletArgs.storage, monitor storage) each calling make_available() independently"
  - "SqliteStorage generates random 33-byte hex storage_identity_key when none configured (e.g. in-memory instances)"
metrics:
  duration: "~4 hours (continued from previous session)"
  completed_date: "2026-03-24"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 26
---

# Phase 04 Plan 01: WalletStorageManager Foundation Summary

Rewrote WalletStorageManager with multi-provider architecture: ManagedStorage wrapper, hierarchical locks, makeAvailable partition logic, and full WalletStorageProvider delegation — replacing the old single-active/single-backup model.

## Tasks Completed

### Task 1: Rewrite WalletStorageManager (TDD)

Completely replaced `src/storage/manager.rs` with:

- **ManagedStorage** (private): wraps `Arc<dyn WalletStorageProvider>` with cached `Settings`, `User`, `is_available: AtomicBool`, and `is_storage_provider: bool`
- **ManagerState** (private): holds `stores: Vec<ManagedStorage>`, `active_index`, `backup_indices`, `conflicting_active_indices`
- **WalletStorageManager**: 3-arg constructor (`identity_key, active, backups`), four `tokio::sync::Mutex<()>` locks, `is_available_flag: AtomicBool`
- **make_available()**: calls `provider.make_available()` + `find_or_insert_user()` on each store, partitions into active/backups/conflicting using TS-parity enabled-active logic
- **Lock helpers**: `acquire_reader/writer/sync/storage_provider` with auto-init (triggers `make_available` if not yet available)
- **Delegation methods**: all monitor, signer, wallet, and sync operations route through `get_active().await?`

Rewritten `tests/manager_tests.rs` with 11 tests covering constructor, make_available, lock hierarchy, idempotency, getters.

Commit: `9da3c67`

### Task 2: Fix Compilation Across Codebase

Fixed 49 → 0 compilation errors cascading from the manager rewrite:

**Monitor tasks/helpers**: Removed trailing `, None` TrxToken args from all delegation calls.

**Signer methods**: Changed parameter type from `&dyn StorageActionProvider` to `&WalletStorageManager` (StorageActionProvider has StorageProvider supertrait which manager no longer satisfies). Added `AuthId` construction at call sites.

**beef.rs**: Added `get_valid_beef_for_storage_reader<S: StorageReader>` variant and shared `build_beef_from_collected` helper to allow beef construction from within storage method implementations (where only `StorageReader` is available, not `WalletStorageProvider`).

**wallet_provider.rs**:
- Added `find_output_baskets`, `insert_output_basket`, `insert_transaction`, `insert_output` to trait (with NotImplemented defaults and blanket impl)
- Fixed `list_outputs` blanket impl to call `list_outputs_rw` instead of `list_outputs` (handles specOp basket names like `SPEC_OP_WALLET_BALANCE`)

**trait_impls.rs**: Fixed `make_available` to generate a random 33-byte hex `storage_identity_key` for in-memory instances where none is configured.

**setup.rs**: Restructured to call `make_available()` on each manager after construction (wallet, setup, monitor each get their own manager instance, all sharing the same provider Arc).

**wallet.rs**: Fixed `get_storage_identity` (sync, can't await), fixed `relinquish_output/certificate` return type wrapping (`i64` → `RelinquishOutputResult`), changed `&auth.identity_key` → `&auth` for `AuthId`-taking methods.

**Test utilities**: Updated `common/mod.rs` to use manager delegation methods directly (removed `storage.active().method(...)` pattern, now uses `storage.method()` on manager).

Commit: `37fabf4`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] list_outputs specOp not handled in blanket impl**
- **Found during:** Task 2 (balance tests returning 0)
- **Issue:** `list_outputs` blanket impl called the read-only `list_outputs` function which doesn't handle `SPEC_OP_WALLET_BALANCE` basket name. The spec op dispatch only exists in `list_outputs_rw`.
- **Fix:** Changed blanket impl to call `list_outputs_rw`
- **Files modified:** `src/storage/traits/wallet_provider.rs`
- **Commit:** 37fabf4

**2. [Rule 2 - Missing] Empty storage_identity_key for in-memory SQLite**
- **Found during:** Task 2 (manager tests asserting `!storage_identity_key.is_empty()`)
- **Issue:** `SqliteStorage::new_sqlite` always initializes `storage_identity_key: String::new()`. In-memory instances never get an identity key set before `make_available()`.
- **Fix:** Generate a random 33-byte hex key in `make_available()` when `storage_identity_key` is empty
- **Files modified:** `src/storage/sqlx_impl/trait_impls.rs`
- **Commit:** 37fabf4

**3. [Rule 1 - Bug] setup.rs created managers without calling make_available()**
- **Found during:** Task 2 (wallet lifecycle tests panicking with "not yet available")
- **Issue:** `WalletBuilder::build()` created 3 `WalletStorageManager` instances but only called `provider.make_available()` (on the raw provider), not `manager.make_available()`. Manager's internal `state.active_index` was never set.
- **Fix:** Call `manager.make_available().await?` on each created manager in setup.rs
- **Files modified:** `src/wallet/setup.rs`
- **Commit:** 37fabf4

## Self-Check: PASSED

- manager.rs: FOUND
- manager_tests.rs: FOUND
- 04-01-SUMMARY.md: FOUND
- commit 9da3c67: FOUND
- commit 37fabf4: FOUND
- All tests: PASSED (214+ in lib, 11 manager_tests, 7 convenience, etc.)
