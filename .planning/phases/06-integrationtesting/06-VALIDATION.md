---
phase: 6
slug: integrationtesting
status: draft
nyquist_compliant: false
wave_0_complete: false
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
| 06-01-01 | 01 | 0 | TEST-01, TEST-02 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_brc31_auth_and_make_available` | ❌ W0 | ⬜ pending |
| 06-01-02 | 01 | 0 | TEST-03 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_find_or_insert_user` | ❌ W0 | ⬜ pending |
| 06-01-03 | 01 | 0 | TEST-04 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_chunk_round_trip` | ❌ W0 | ⬜ pending |
| 06-01-04 | 01 | 0 | TEST-05 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_internalize_with_storage_client_backup` | ❌ W0 | ⬜ pending |
| 06-01-05 | 01 | 0 | TEST-06 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_to_writer_remote` | ❌ W0 | ⬜ pending |
| 06-01-06 | 01 | 0 | TEST-07 | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_update_backups_remote` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `tests/storage_client_integration_tests.rs` — stubs for TEST-01 through TEST-07
- [ ] `BSV_TEST_ROOT_KEY` — operator must set env var (documented in test file header)
- [ ] Verify ProtoWallet is Send + Sync (compiler check)

*Existing test framework covers all tooling needs — no new dependencies required.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| None | — | — | — |

*All phase behaviors have automated verification.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
