---
phase: 1
slug: wire-format
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-24
---

# Phase 1 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test (`cargo test`) |
| **Config file** | `Cargo.toml` — `[dev-dependencies]` tokio |
| **Quick run command** | `cargo test --test table_tests` |
| **Full suite command** | `cargo test --test table_tests` |
| **Estimated runtime** | ~5 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --test table_tests`
- **After every plan wave:** Run `cargo test --test table_tests`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 5 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 1-01-01 | 01 | 0 | WIRE-01 | unit | `cargo test --test table_tests timestamp_format` | ❌ W0 | ⬜ pending |
| 1-01-02 | 01 | 0 | WIRE-01 | unit | `cargo test --test table_tests zero_millis` | ❌ W0 | ⬜ pending |
| 1-01-03 | 01 | 0 | WIRE-01 | unit | `cargo test --test table_tests ts_fixture_deserialize` | ❌ W0 | ⬜ pending |
| 1-01-04 | 01 | 0 | WIRE-02 | unit | `cargo test --test table_tests vec_u8_some_serializes_as_array` | ✅ | ⬜ pending |
| 1-01-05 | 01 | 0 | WIRE-02 | unit | `cargo test --test table_tests vec_u8_optional_none_serializes_as_null` | ✅ | ⬜ pending |
| 1-01-06 | 01 | 0 | WIRE-03 | unit | `cargo test --test table_tests sync_chunk_absent_fields` | ❌ W0 | ⬜ pending |
| 1-01-07 | 01 | 0 | WIRE-03 | unit | `cargo test --test table_tests sync_chunk_present_fields` | ❌ W0 | ⬜ pending |
| 1-01-08 | 01 | 0 | WIRE-04 | unit | `cargo test --test table_tests proven_tx_ts_fixture_roundtrip` | ❌ W0 | ⬜ pending |
| 1-01-09 | 01 | 0 | WIRE-04 | unit | `cargo test --test table_tests transaction_ts_fixture_roundtrip` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `tests/table_tests.rs` — add timestamp format assertions (WIRE-01): Z suffix + 3-digit ms
- [ ] `tests/table_tests.rs` — add SyncChunk optional-absent assertions (WIRE-03)
- [ ] `tests/table_tests.rs` — add TS-fixture round-trip tests (WIRE-04): ProvenTx and Transaction

*Existing infrastructure covers WIRE-02. No new test files or framework setup needed.*

---

## Manual-Only Verifications

*All phase behaviors have automated verification.*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 5s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
