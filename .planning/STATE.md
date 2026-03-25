---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Completed 04-manager-rewrite/04-02-PLAN.md
last_updated: "2026-03-25T00:41:42.395Z"
last_activity: 2026-03-24 — Completed Phase 04 Plan 01 (WalletStorageManager foundation rewrite)
progress:
  total_phases: 7
  completed_phases: 4
  total_plans: 7
  completed_plans: 7
  percent: 25
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-03-24)

**Core value:** Wire-compatible remote storage that lets a Rust wallet sync with TypeScript storage servers
**Current focus:** Phase 1 — Wire Format

## Current Position

Phase: 4 of 7 (Manager Rewrite)
Plan: 1 of ? in current phase — Plan 01 complete
Status: In progress
Last activity: 2026-03-24 — Completed Phase 04 Plan 01 (WalletStorageManager foundation rewrite)

Progress: [██░░░░░░░░] 25%

## Performance Metrics

**Velocity:**
- Total plans completed: 0
- Average duration: —
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| - | - | - | - |

*Updated after each plan completion*
| Phase 01-wire-format P01 | 3 | 2 tasks | 3 files |
| Phase 02-trait-definition P01 | 9 | 1 tasks | 10 files |
| Phase 03-storageclient P01 | 200 | 1 tasks | 4 files |
| Phase 03-storageclient P02 | 2min | 1 tasks | 1 files |
| Phase 04-manager-rewrite P01 | ~4h | 2 tasks | 26 files |
| Phase 04-manager-rewrite P03 | 20min | 2 tasks | 5 files |
| Phase 04-manager-rewrite P02 | 52min | 2 tasks | 5 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [Pre-phase]: StorageClient does NOT implement StorageProvider — uses narrower WalletStorageProvider trait (matches TS pattern)
- [Pre-phase]: AuthFetch<W> held behind tokio::sync::Mutex, not std::sync::Mutex (std deadlocks Tokio executor)
- [Pre-phase]: Test against live TS servers first — production interop is the primary goal
- [Phase 01-wire-format]: Split serde_datetime FORMAT into PARSE_FORMAT (tolerant %.f) and SERIALIZE_FORMAT (strict %.3f) — deserialization accepts variable-precision TS timestamps, serialization always produces 3ms+Z
- [Phase 01-wire-format]: skip_serializing_if only on SyncChunk optional entity lists, NOT on table struct Option<T> fields — table fields serialize as null to match TS wire format for table records
- [Phase 02-trait-definition]: WalletStorageProvider is standalone (not supertrait of StorageProvider) — StorageClient implements it directly without StorageProvider
- [Phase 02-trait-definition]: get_settings declared async in WalletStorageProvider — blanket impl delegates to StorageProvider::get_settings(self, None).await
- [Phase 02-trait-definition]: abort_action extracted as standalone function in storage/methods/abort_action.rs for blanket impl use
- [Phase 03-storageclient]: tokio::sync::Mutex for AuthFetch and settings cache — std::sync::Mutex held across .await deadlocks Tokio
- [Phase 03-storageclient]: wallet_error_from_object placed in error.rs alongside to_wallet_error_object for symmetric WERR conversion
- [Phase 03-storageclient]: AtomicBool with Acquire/Release for sync is_available() — avoids blocking lock in sync trait fn
- [Phase 03-storageclient]: Wire helper structs (FindOrInsertUserWire, FindOrInsertSyncStateWire) for tuple-returning RPCs placed inline in storage_client.rs
- [Phase 03-storageclient]: internalize_action ignores services param — accepted in sig for trait compliance, never serialized (no TS equivalent)
- [Phase 04-manager-rewrite]: WalletStorageManager takes Arc<dyn WalletStorageProvider> (not StorageProvider) — allows StorageClient to be used as a provider
- [Phase 04-manager-rewrite]: Signer methods accept &WalletStorageManager directly (not &dyn StorageActionProvider) — StorageActionProvider supertrait incompatibility
- [Phase 04-manager-rewrite]: list_outputs blanket impl calls list_outputs_rw to handle specOp basket names (SPEC_OP_WALLET_BALANCE etc.)
- [Phase 04-manager-rewrite]: setup.rs calls make_available() on each WalletStorageManager after construction — manager state is independent of provider state
- [Phase 04-manager-rewrite]: reprove_proven acquires no lock itself — caller (reprove_header) holds StorageProvider lock
- [Phase 04-manager-rewrite]: get_endpoint_url default returns None — StorageClient overrides to return Some(endpoint_url)
- [Phase 04-manager-rewrite]: find_or_insert_user consistency check uses cached auth to prevent silent identity collision
- [Phase 04-manager-rewrite]: done=true detected at WalletStorageProvider blanket impl level, not free process_sync_chunk function — blanket impl has DB access for SyncState updates
- [Phase 04-manager-rewrite]: SyncState.when advanced to now() after each chunk to prevent re-fetching same data on subsequent sync iterations
- [Phase 04-manager-rewrite]: Empty/initial sync_map JSON falls back to SyncMap::new() in make_request_sync_chunk_args to handle first-sync case

### Codebase Context (from pre-roadmap research)

- `src/storage/remoting/` directory needs to be created
- `SyncChunk`, `ProcessSyncChunkResult`, `SyncMap` already defined in `src/storage/sync/`
- `Settings` table struct has Serialize/Deserialize with serde_datetime handling
- `AuthId` struct exists at `src/wallet/types.rs`
- AuthFetch in bsv-sdk 0.1.75 is generic over `W: WalletInterface + Clone + 'static`, takes `&mut self`
- TS StorageClient has ~25 methods, all one-liner pass-throughs to rpcCall

### Blockers/Concerns

- [Phase 3]: `reset_session()` path uncertain — AuthFetch peers field may not be pub; may need to recreate entire AuthFetch instance. Verify bsv-sdk-0.1.75 source before implementing.
- [Phase 2]: Blanket impl feasibility — if any WalletStorageProvider method signature differs from StorageProvider, per-type impls will be needed. Audit overlap before writing.

### Pending Todos

None yet.

## Session Continuity

Last session: 2026-03-25T00:41:42.393Z
Stopped at: Completed 04-manager-rewrite/04-02-PLAN.md
Resume file: None
