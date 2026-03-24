---
phase: 01-wire-format
plan: 01
subsystem: serialization
tags: [serde, chrono, naivdatetime, json, wire-format, sync]

# Dependency graph
requires: []
provides:
  - "serde_datetime: fixed timestamp serialization with Z suffix and exactly 3 ms digits"
  - "SyncChunk: optional entity list fields omit keys from JSON when None"
  - "10 wire-format tests proving TS-compatible serialization and round-trip"
affects: [02-storage-client, 03-auth-fetch, 04-integration, 05-validation]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "PARSE_FORMAT / SERIALIZE_FORMAT split: tolerant parse (%.f) vs strict serialize (%.3f) in serde helpers"
    - "format!({}Z, ...) pattern for TS-compatible timestamp serialization"
    - "skip_serializing_if = Option::is_none on optional entity lists to omit absent keys from wire JSON"

key-files:
  created: []
  modified:
    - src/serde_datetime.rs
    - src/storage/sync/sync_map.rs
    - tests/table_tests.rs

key-decisions:
  - "Split single FORMAT constant into PARSE_FORMAT (%.f, tolerant) and SERIALIZE_FORMAT (%.3f, strict 3ms digits) -- deserialization must remain tolerant to accept variable-precision TS timestamps"
  - "Z suffix appended with format!({}Z, ...) not baked into format string -- chrono %.f strips trailing zeros so we need the explicit Z after formatting"
  - "skip_serializing_if only on SyncChunk optional entity lists, NOT on table struct Option<T> fields (e.g. Output.spent_by) -- table fields must serialize as null to match TS wire format"

patterns-established:
  - "Wire format parity: all NaiveDateTime serialize as YYYY-MM-DDTHH:MM:SS.mmmZ, matching TS Date.toISOString() byte-for-byte"
  - "TDD RED-GREEN for wire format: write assertions against TS fixture JSON strings first, then fix production code"

requirements-completed: [WIRE-01, WIRE-02, WIRE-03, WIRE-04]

# Metrics
duration: 3min
completed: 2026-03-24
---

# Phase 1 Plan 01: Wire Format Serialization Fixes Summary

**Timestamps now serialize as YYYY-MM-DDTHH:MM:SS.mmmZ (3ms + Z) matching TS Date.toISOString(), and SyncChunk absent entity lists are omitted from JSON instead of serializing as null**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-24T14:25:01Z
- **Completed:** 2026-03-24T14:28:00Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Fixed `serde_datetime.rs` to serialize NaiveDateTime (and Option<NaiveDateTime>) with exactly 3 millisecond digits and a trailing Z suffix, matching TS `Date.toISOString()` output byte-for-byte
- Added `skip_serializing_if = "Option::is_none"` to all 13 optional entity list fields in `SyncChunk`, so absent fields are omitted from JSON (not null) as the TS StorageServer wire format requires
- Added 10 wire-format tests covering: Z suffix, 3ms digits, Option<NaiveDateTime>, TS-format deserialization, variable-precision deserialization regression, SyncChunk absent/present fields, and three end-to-end TS-fixture round-trip tests

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix serde_datetime and SyncChunk serialization** - `a96a3d1` (feat)
2. **Task 2: TS-fixture round-trip tests** - `10c4bdf` (test)

_Note: TDD tasks — Task 1 included RED then GREEN in the same commit since fixes were made before commit; Task 2 tests passed immediately since production code was already fixed._

## Files Created/Modified
- `src/serde_datetime.rs` - Split FORMAT into PARSE_FORMAT/SERIALIZE_FORMAT; serialize functions now append Z and use %.3f
- `src/storage/sync/sync_map.rs` - Added skip_serializing_if to all 13 optional entity list fields in SyncChunk
- `tests/table_tests.rs` - Added 10 new wire-format tests (7 in Task 1, 3 in Task 2)

## Decisions Made
- Split `FORMAT` into `PARSE_FORMAT` (`%.f`, tolerant) and `SERIALIZE_FORMAT` (`%.3f`, strict): deserialize must accept 1-9 digit fractional seconds from TS, but serialize must always produce exactly 3. Using a single format for both would break either parsing or output.
- Applied `skip_serializing_if` only to `SyncChunk`'s optional entity list fields, explicitly NOT to table struct `Option<T>` fields like `Output.spent_by` — those must serialize as `null` to match TS wire format for table records.
- Z appended via `format!("{}Z", date.format(SERIALIZE_FORMAT))` rather than in the format string: chrono's `%.f` strips trailing zeros (`.000` becomes empty), so the Z cannot be embedded — it must be appended after formatting.

## Deviations from Plan

None — plan executed exactly as written. Pre-existing `sync_tests.rs` compile error (`missing field offsets in GetSyncChunkArgs`) was discovered during full `cargo test` verification; confirmed pre-existing, logged for deferred resolution.

## Issues Encountered
- Pre-existing: `tests/sync_tests.rs` fails to compile with `missing field offsets in initializer of GetSyncChunkArgs`. This is out-of-scope for this plan — present before any changes in this plan were made. Logged to deferred items.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness
- Wire format is now correct: all timestamps serialize as `YYYY-MM-DDTHH:MM:SS.mmmZ`, SyncChunk omits absent fields
- Phase 2 (StorageClient struct) can now build HTTP round-trips with confidence that request/response JSON matches the TS server wire format
- No blockers from this plan; pre-existing `sync_tests.rs` compile error should be resolved before Phase 4

---
*Phase: 01-wire-format*
*Completed: 2026-03-24*

## Self-Check: PASSED

- src/serde_datetime.rs: FOUND
- src/storage/sync/sync_map.rs: FOUND
- tests/table_tests.rs: FOUND
- .planning/phases/01-wire-format/01-01-SUMMARY.md: FOUND
- Commit a96a3d1 (Task 1): FOUND
- Commit 10c4bdf (Task 2): FOUND
