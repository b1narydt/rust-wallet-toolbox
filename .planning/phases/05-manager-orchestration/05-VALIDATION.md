---
phase: 05
slug: manager-orchestration
status: draft
nyquist_compliant: true
wave_0_complete: true
created: 2026-03-25
---

# Phase 05 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (tokio::test for async) |
| **Config file** | Cargo.toml (features = ["sqlite"]) |
| **Quick run command** | `cargo test --features sqlite --test manager_tests` |
| **Full suite command** | `cargo test --features sqlite` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --features sqlite --test manager_tests`
- **After every plan wave:** Run `cargo test --features sqlite`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 05-01-01 | 01 | 1 | ORCH-01 | unit+integration | `cargo test --features sqlite test_set_active` | ✅ (manager_tests.rs) | ⬜ pending |
| 05-01-02 | 01 | 1 | ORCH-02..05 | verification | `cargo test --features sqlite --test manager_tests` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

Existing infrastructure covers all phase requirements. `tests/manager_tests.rs` already has 22 tests covering the manager; new tests will be added to the same file.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Multi-provider conflict resolution with real StorageClient | ORCH-01 | Requires live TS server | Create wallet with 2 active providers, call set_active, verify state merge |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 30s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
