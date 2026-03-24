---
phase: 03-storageclient
plan: 02
subsystem: storage
tags: [rpc, json-rpc, remote-storage, brc-31, wallet-storage-provider]

# Dependency graph
requires:
  - phase: 03-01
    provides: StorageClient struct, rpc_call infrastructure, partial WalletStorageProvider stub

provides:
  - Complete WalletStorageProvider impl on StorageClient with all 23 methods
  - Wire helper structs for tuple-returning RPC methods (FindOrInsertUserWire, FindOrInsertSyncStateWire)
  - test_wire_names regression test documenting all 22 method-to-wire-name mappings

affects: [04-walletstoragemanager, integration-tests]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - rpc_call delegate — each method is a one-liner forwarding to self.rpc_call(wire_name, params)
    - Wire helper struct deserialization — TS returns { field, isNew } objects; Rust tuple-returns need intermediate structs with Deserialize
    - services param suppression — internalize_action accepts but ignores &dyn WalletServices (not serializable, no TS equivalent)
    - Positional string params via Value::String — &str params serialized directly, not wrapped in a struct

key-files:
  created: []
  modified:
    - src/storage/remoting/storage_client.rs

key-decisions:
  - "Wire helper structs (FindOrInsertUserWire, FindOrInsertSyncStateWire) placed inline in storage_client.rs — private to the impl, no separate module needed"
  - "internalize_action ignores services param with _services binding — accepted in sig for trait compliance, never serialized"
  - "findOutputBaskets anomaly documented in method doc comment — only method whose wire name differs from Rust name pattern"

patterns-established:
  - "rpc_call one-liner pattern: self.rpc_call(wire_name, vec![serde_json::to_value(param)?]).await"
  - "String params use Value::String(s.to_string()) not serde_json::to_value(&s) to avoid double-quoting"
  - "Tuple-returning RPCs: deserialize to wire struct, then return Ok((struct.field, struct.is_new))"

requirements-completed: [CLIENT-03, CLIENT-06]

# Metrics
duration: 2min
completed: 2026-03-24
---

# Phase 03 Plan 02: StorageClient WalletStorageProvider Methods Summary

**18 todo!() stubs replaced with rpc_call delegates completing StorageClient as a fully wire-compatible WalletStorageProvider usable as Arc<dyn WalletStorageProvider>**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-24T21:43:37Z
- **Completed:** 2026-03-24T21:45:52Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments

- All 23 WalletStorageProvider methods compile on StorageClient (0 todo!() remaining)
- Each method forwards to rpc_call with correct TS wire name from the research table
- findOutputBaskets uses wire name "findOutputBaskets" (no Auth suffix — only anomaly)
- internalize_action accepts but ignores the non-serializable services param
- find_or_insert_user and find_or_insert_sync_state_auth use private wire helper structs
  to convert TS {field, isNew} JSON objects into Rust (T, bool) tuples
- test_wire_names test documents all 22 method-to-wire-name mappings as a regression guard
- cargo build and cargo test --features sqlite both pass clean

## Task Commits

1. **Task 1: Replace all todo!() stubs with rpc_call delegates** - `01abe79` (feat)

**Plan metadata:** (pending)

## Files Created/Modified

- `src/storage/remoting/storage_client.rs` - Replaced 18 todo!() stubs with rpc_call delegates; added FindOrInsertUserWire, FindOrInsertSyncStateWire helper structs; added test_wire_names test

## Decisions Made

- Wire helper structs placed inline in storage_client.rs (private use, no separate module)
- internalize_action ignores services param — accepted in signature for trait compliance, never serialized (services has no TS equivalent)
- String parameters use Value::String(s.to_string()) rather than serde_json::to_value for clarity on the wire encoding

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- StorageClient fully implements WalletStorageProvider and can be used as Arc<dyn WalletStorageProvider + Send + Sync>
- Phase 4 (WalletStorageManager) can now use StorageClient directly as a remote storage backend
- No blockers

---
*Phase: 03-storageclient*
*Completed: 2026-03-24*
