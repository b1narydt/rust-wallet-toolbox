---
phase: 02
slug: trait-definition
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-24
---

# Phase 02 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` / `#[tokio::test]` |
| **Config file** | `Cargo.toml` (test feature gating via `#[cfg(feature = "sqlite")]`) |
| **Quick run command** | `cargo test --test trait_tests 2>&1` |
| **Full suite command** | `cargo test 2>&1` |
| **Estimated runtime** | ~5 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --test trait_tests 2>&1`
- **After every plan wave:** Run `cargo test 2>&1`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 10 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 02-01-01 | 01 | 1 | TRAIT-01 | unit (compile) | `cargo test --test trait_tests wallet_storage_provider_object_safe` | ❌ W0 | ⬜ pending |
| 02-01-02 | 01 | 1 | TRAIT-02 | unit (compile) | `cargo test --test trait_tests sqlite_storage_satisfies_wallet_provider` | ❌ W0 | ⬜ pending |
| 02-01-03 | 01 | 1 | TRAIT-03 | unit (runtime) | `cargo test --test trait_tests is_storage_provider_returns_true` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `tests/trait_tests.rs` — stubs for TRAIT-01, TRAIT-02, TRAIT-03
  - Test 1: `fn wallet_storage_provider_object_safe()` — proves `Arc<dyn WalletStorageProvider>` compiles
  - Test 2: `fn sqlite_storage_satisfies_wallet_provider()` — proves blanket impl compiles for `SqliteStorage`
  - Test 3: `fn is_storage_provider_returns_true()` — runtime check on blanket impl default

*Existing test infrastructure (cargo test) covers all framework needs.*

---

## Manual-Only Verifications

*All phase behaviors have automated verification.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 10s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
