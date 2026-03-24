---
phase: 3
slug: storageclient
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-24
---

# Phase 3 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[cfg(test)]` + `#[tokio::test]` (tokio 1.x) |
| **Config file** | none (standard Cargo test runner) |
| **Quick run command** | `cargo test --lib storage::remoting` |
| **Full suite command** | `cargo test --features sqlite` |
| **Estimated runtime** | ~5 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --lib storage::remoting`
- **After every plan wave:** Run `cargo test --features sqlite`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 10 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 03-01-01 | 01 | 1 | CLIENT-01 | unit | `cargo test --lib storage::remoting::tests::rpc_envelope` | ❌ W0 | ⬜ pending |
| 03-01-02 | 01 | 1 | CLIENT-02 | unit | `cargo test --lib storage::remoting::tests::new_constructs` | ❌ W0 | ⬜ pending |
| 03-01-03 | 01 | 1 | CLIENT-04 | unit | `cargo test --lib storage::remoting::tests::error_mapping` | ❌ W0 | ⬜ pending |
| 03-01-04 | 01 | 1 | CLIENT-03 | compile-check | `cargo build` | ❌ W0 | ⬜ pending |
| 03-01-05 | 01 | 1 | CLIENT-05 | compile-check | `cargo build` | ❌ W0 | ⬜ pending |
| 03-01-06 | 01 | 1 | CLIENT-06 | unit | `cargo test --lib storage::remoting::tests::wire_names` | ❌ W0 | ⬜ pending |
| 03-01-07 | 01 | 1 | CLIENT-07 | unit | `cargo test --lib storage::remoting::tests::settings_cache` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `src/storage/remoting/mod.rs` — module entry point
- [ ] `src/storage/remoting/storage_client.rs` — struct + impl with `#[cfg(test)]` block
- [ ] `wallet_error_from_object` function for JSON-RPC error mapping
- [ ] `UpdateProvenTxReqWithNewProvenTxArgs` and `UpdateProvenTxReqWithNewProvenTxResult` structs

*Existing infrastructure covers test framework (cargo test already configured).*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live JSON-RPC round-trip | CLIENT-01 | Requires running storage server | Start server, run `cargo test --features integration` |

*All other phase behaviors have automated verification via unit tests with mock responses.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 10s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
