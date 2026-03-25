# Deferred Items — Phase 04 Manager Rewrite

## Plan 03 Execution — 2026-03-25

### Pre-existing Test Failures (out of scope for Plan 03)

The following tests exist in `tests/manager_tests.rs` from pre-Plan-03 working tree modifications.
They reference Plan 02 methods (`sync_to_writer`, `sync_from_reader`) that have a bug
where `ProcessSyncChunkResult.done` is never set to `true`, causing `sync_to_writer` to
loop infinitely.

**Affected tests:**
- `test_sync_to_writer` — hangs (infinite loop, `done` never set)
- `test_sync_from_reader_matching_identity` — hangs (same root cause)
- `test_sync_prog_log` — hangs (same root cause)

**Root cause:** `process_sync_chunk()` in `src/storage/sync/process_sync_chunk.rs` does not
set `ProcessSyncChunkResult.done = true` when the chunk contains no data (i.e., sync is
complete). The `get_sync_chunk` implementation needs to signal completion by setting
`done = true` when no more data is available.

**Fix needed in Plan 02:** Add logic to set `done = true` in `ProcessSyncChunkResult`
when `get_sync_chunk` returns an empty chunk (no entities and no `user` change).
The blanket `process_sync_chunk` in `wallet_provider.rs` should propagate this flag from
the chunk metadata or detect it from the empty result.

**Files to fix:**
- `src/storage/sync/process_sync_chunk.rs` — set `result.done = true` when no entities processed
- `src/storage/traits/wallet_provider.rs` — ensure done flag propagates through blanket impl

**Not fixed in Plan 03** — out of scope per SCOPE BOUNDARY rule.
