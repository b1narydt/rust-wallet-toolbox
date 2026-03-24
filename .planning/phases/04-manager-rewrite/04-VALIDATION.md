---
phase: 4
slug: manager-rewrite
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-24
---

# Phase 4 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test + tokio test (tokio 1.x) |
| **Config file** | Cargo.toml `[dev-dependencies]` |
| **Quick run command** | `cargo test --features sqlite manager_tests 2>&1` |
| **Full suite command** | `cargo test --features sqlite 2>&1` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --features sqlite manager_tests 2>&1`
- **After every plan wave:** Run `cargo test --features sqlite 2>&1`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 04-01-01 | 01 | 0 | MGR-01 | unit | `cargo test --features sqlite manager_tests::test_constructor` | ❌ W0 | ⬜ pending |
| 04-01-02 | 01 | 0 | MGR-02 | unit | `cargo test --features sqlite manager_tests::test_managed_storage_cache` | ❌ W0 | ⬜ pending |
| 04-01-03 | 01 | 0 | MGR-03 | unit | `cargo test --features sqlite manager_tests::test_make_available_partition` | ❌ W0 | ⬜ pending |
| 04-01-04 | 01 | 0 | MGR-04 | unit | `cargo test --features sqlite manager_tests::test_hierarchical_locks` | ❌ W0 | ⬜ pending |
| 04-01-05 | 01 | 0 | MGR-05 | unit | `cargo test --features sqlite manager_tests::test_sync_to_writer` | ❌ W0 | ⬜ pending |
| 04-01-06 | 01 | 0 | MGR-06 | unit | `cargo test --features sqlite manager_tests::test_update_backups` | ❌ W0 | ⬜ pending |
| 04-01-07 | 01 | 0 | MGR-07 | unit | `cargo test --features sqlite manager_tests::test_delegation_methods` | ❌ W0 | ⬜ pending |
| 04-01-08 | 01 | 0 | MGR-08 | unit | `cargo test --features sqlite manager_tests::test_add_provider_runtime` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `tests/manager_tests.rs` — full rewrite from OLD manager API to new WalletStorageProvider-based interface; covers MGR-01 through MGR-08
- [ ] `tests/common/` — may need `make_wallet_provider` helper returning `Arc<dyn WalletStorageProvider>` from SqliteStorage

*The existing `tests/manager_tests.rs` uses the OLD manager API and must be replaced entirely.*

---

## Manual-Only Verifications

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
