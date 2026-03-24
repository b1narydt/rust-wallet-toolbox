---
phase: 01-wire-format
verified: 2026-03-24T15:00:00Z
status: passed
score: 6/6 must-haves verified
re_verification: false
---

# Phase 1: Wire Format Verification Report

**Phase Goal:** Serialization matches the TypeScript server wire format before any client code is written
**Verified:** 2026-03-24
**Status:** PASSED
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| #  | Truth                                                                                                                            | Status     | Evidence                                                                                                |
|----|----------------------------------------------------------------------------------------------------------------------------------|------------|---------------------------------------------------------------------------------------------------------|
| 1  | `serde_datetime::serialize` emits timestamps with 3-digit ms precision and trailing "Z" (e.g. `"2024-01-15T10:30:00.718Z"`)     | VERIFIED   | `SERIALIZE_FORMAT = "%Y-%m-%dT%H:%M:%S%.3f"` + `format!("{}Z", ...)` in `serde_datetime.rs` lines 20,27. Test `timestamp_serializes_with_z_and_3ms` asserts exact string equality. |
| 2  | Timestamps with zero milliseconds serialize as `.000Z` not bare seconds                                                          | VERIFIED   | `%.3f` in `SERIALIZE_FORMAT` always pads to 3 digits. Test `timestamp_zero_millis_serializes_as_000z` asserts `ends_with(".000Z")`. |
| 3  | `Option<NaiveDateTime>` fields serialize with Z and 3-digit ms                                                                   | VERIFIED   | `option` submodule in `serde_datetime.rs` lines 52-59 uses same `SERIALIZE_FORMAT` + `format!("{}Z", ...)`. Test `option_timestamp_serializes_with_z` uses `EntitySyncMap.max_updated_at`. Wired via `#[serde(with = "crate::serde_datetime::option")]` on `EntitySyncMap.max_updated_at` at `sync_map.rs` line 80. |
| 4  | `Vec<u8>` fields serialize as JSON integer arrays, not base64                                                                    | VERIFIED   | No custom serializer on `Vec<u8>` fields in any table struct (grep confirmed). Serde default for `Vec<u8>` is integer array. Tests `vec_u8_some_serializes_as_array` and `vec_u8_optional_none_serializes_as_null` pass. `proven_tx_ts_fixture_roundtrip` asserts integer array values directly. |
| 5  | `SyncChunk` with `None` entity lists omits those fields from JSON (not null)                                                     | VERIFIED   | All 13 optional fields in `SyncChunk` carry `#[serde(skip_serializing_if = "Option::is_none")]` (lines 28-65 of `sync_map.rs`). Test `sync_chunk_none_fields_absent_not_null` asserts absence of `user`, `provenTxs`, `transactions`, `outputs`, `certificates`. `sync_chunk_ts_fixture_roundtrip` asserts round-trip absent fields stay absent. |
| 6  | TS-format JSON fixtures deserialize into correct Rust structs and round-trip back to identical JSON                              | VERIFIED   | Three round-trip tests pass: `proven_tx_ts_fixture_roundtrip` (timestamps + binary arrays), `transaction_ts_fixture_roundtrip` (optional `Vec<u8>` fields), `sync_chunk_ts_fixture_roundtrip` (sparse entity lists). All assert exact field values and correct re-serialized format. |

**Score:** 6/6 truths verified

### Required Artifacts

| Artifact                              | Expected                                                         | Status   | Details                                                                                            |
|---------------------------------------|------------------------------------------------------------------|----------|----------------------------------------------------------------------------------------------------|
| `src/serde_datetime.rs`               | Fixed timestamp serialization with Z suffix and 3-digit ms       | VERIFIED | Contains `SERIALIZE_FORMAT`, `format!("{}Z", ...)` in both outer module and `option` submodule. Substantive: 79 lines, two complete serialize/deserialize pairs. |
| `src/storage/sync/sync_map.rs`        | SyncChunk with `skip_serializing_if` on all optional entity lists | VERIFIED | Contains `skip_serializing_if` on all 13 optional fields. Substantive: 186 lines, complete struct + EntitySyncMap + SyncMap implementations. |
| `tests/table_tests.rs`                | Wire format tests covering timestamps, binary, SyncChunk, TS fixtures | VERIFIED | Contains `timestamp_serializes_with_z` and all required test functions. 44 tests total pass with 0 failures. |

### Key Link Verification

| From                                     | To                                              | Via                                          | Status   | Details                                                                                             |
|------------------------------------------|-------------------------------------------------|----------------------------------------------|----------|-----------------------------------------------------------------------------------------------------|
| `src/serde_datetime.rs`                  | All 16 table structs with NaiveDateTime fields  | `#[serde(with = "crate::serde_datetime")]`  | VERIFIED | 36 usages across 10 table source files (tx_label_map, output_tag_map, transaction, output_basket, certificate_field, proven_tx_req, certificate, commission, proven_tx, user). Both `created_at` and `updated_at` on each. |
| `src/serde_datetime.rs` (option module) | `EntitySyncMap.max_updated_at`                  | `#[serde(with = "crate::serde_datetime::option")]` | VERIFIED | `sync_map.rs` line 80 uses `option` submodule. Test `option_timestamp_serializes_with_z` exercises this path. |
| `tests/table_tests.rs`                  | `src/serde_datetime.rs`                         | `serde_json` round-trip assertions            | VERIFIED | Tests assert `ends_with("Z")` and exact string equality `"2024-01-15T10:30:00.718Z"`. Compile dependency verified by test suite passing. |

### Requirements Coverage

| Requirement | Source Plan | Description                                                                                  | Status    | Evidence                                                                                      |
|-------------|-------------|----------------------------------------------------------------------------------------------|-----------|-----------------------------------------------------------------------------------------------|
| WIRE-01     | 01-01-PLAN  | serde_datetime emits ISO 8601 timestamps with trailing "Z" and 3-digit ms precision          | SATISFIED | `SERIALIZE_FORMAT` + `format!("{}Z", ...)`. Tests `timestamp_serializes_with_z_and_3ms`, `timestamp_zero_millis_serializes_as_000z`, `option_timestamp_serializes_with_z` all pass. |
| WIRE-02     | 01-01-PLAN  | Vec<u8> fields serialize as JSON number arrays (not base64)                                  | SATISFIED | Default serde Vec<u8> produces integer arrays. Confirmed by `vec_u8_some_serializes_as_array` and `proven_tx_ts_fixture_roundtrip` (asserts `merkle[0] == 1`). |
| WIRE-03     | 01-01-PLAN  | SyncChunk and SyncMap serialize to JSON matching TS wire format (camelCase, optional absent) | SATISFIED | `#[serde(rename_all = "camelCase")]` on SyncChunk/SyncMap/EntitySyncMap. `skip_serializing_if` on all 13 SyncChunk optional fields. Tests `sync_chunk_none_fields_absent_not_null` and `sync_chunk_present_fields_included` pass. |
| WIRE-04     | 01-01-PLAN  | Round-trip serde tests pass with TS-generated fixture JSON for all entity types              | SATISFIED | Three TS-fixture round-trip tests pass: `proven_tx_ts_fixture_roundtrip`, `transaction_ts_fixture_roundtrip`, `sync_chunk_ts_fixture_roundtrip`. Fixture strings use exact TS Date.toISOString() format with integer arrays. |

### Anti-Patterns Found

No blockers or warnings found in the three modified files.

| File                               | Line | Pattern                            | Severity | Impact |
|------------------------------------|------|------------------------------------|----------|--------|
| `tests/sync_tests.rs` (pre-existing) | 133, 190 | `missing field offsets in GetSyncChunkArgs` compile error | Warning (pre-existing) | Does not affect this phase. Present before any phase 01 changes (no commits to file since `dfb9f13`). Noted in SUMMARY as deferred. |

The pre-existing `sync_tests.rs` compile error causes `cargo test` (full suite) to abort before running any test binaries. However this error predates this phase entirely — `git log` confirms zero commits to `tests/sync_tests.rs` since before the phase began. It is not a regression introduced by this phase.

### Human Verification Required

None. All success criteria are verifiable programmatically through the test suite.

### Gaps Summary

No gaps. All six must-have truths are verified, all three artifacts are substantive and wired, all four WIRE-0x requirement IDs are satisfied, and no anti-patterns were introduced.

The pre-existing `sync_tests.rs` compile error is noted but is out of scope for this phase. It was explicitly logged as a deferred item in the SUMMARY and should be resolved before Phase 4 (integration testing).

---

## Test Suite Results

- `cargo test --test table_tests`: 44 passed, 0 failed
  - 34 pre-existing tests: all pass (no regressions)
  - 10 new wire-format tests: all pass
- `cargo test` (full): blocked by pre-existing `sync_tests.rs` compile error unrelated to this phase

## Task Commits

- `a96a3d1` — fix timestamp serialization and SyncChunk optional field handling
- `10c4bdf` — add TS-fixture round-trip tests for ProvenTx, Transaction, and SyncChunk

---
_Verified: 2026-03-24_
_Verifier: Claude (gsd-verifier)_
