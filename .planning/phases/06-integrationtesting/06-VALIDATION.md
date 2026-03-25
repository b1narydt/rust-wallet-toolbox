---
phase: 6
slug: integrationtesting
status: draft
nyquist_compliant: true
wave_0_complete: true
created: 2026-03-25
---

# Phase 6 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test + tokio (1.x) |
| **Config file** | Cargo.toml `[dev-dependencies]` — tokio with "full" features |
| **Quick run command** | `cargo test --features sqlite storage_client_integration 2>&1` |
| **Full suite command** | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored storage_client_integration 2>&1` |
| **Estimated runtime** | ~30 seconds (live network calls) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --features sqlite storage_client_integration 2>&1`
- **After every plan wave:** Run `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored storage_client_integration 2>&1`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 06-01-01 | 01 | 1 | TEST-01, TEST-02 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_brc31_auth_and_make_available` | Yes (Task 1 creates) | ⬜ pending |
| 06-01-02 | 01 | 1 | TEST-03 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_find_or_insert_user` | Yes (Task 1 creates) | ⬜ pending |
| 06-01-03 | 01 | 1 | TEST-04 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_chunk_round_trip` | Yes (Task 1 creates) | ⬜ pending |
| 06-01-04 | 01 | 1 | TEST-05 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_internalize_with_storage_client_backup` | Yes (Task 2 creates) | ⬜ pending |
| 06-01-05 | 01 | 1 | TEST-06 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_to_writer_remote` | Yes (Task 3 creates) | ⬜ pending |
| 06-01-06 | 01 | 1 | TEST-07 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_update_backups_remote` | Yes (Task 3 creates) | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [x] `tests/storage_client_integration_tests.rs` — Tasks 1-3 create all 6 test functions for TEST-01 through TEST-07
- [ ] `BSV_TEST_ROOT_KEY` — operator must set env var (documented in test file header)
- [x] Verify ProtoWallet is Send + Sync (compiler check — Tasks 1-2 will fail to compile if not satisfied)

*Existing test framework covers all tooling needs — no new dependencies required.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| None | — | — | — |

*All phase behaviors have automated verification.*

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 30s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** ready
