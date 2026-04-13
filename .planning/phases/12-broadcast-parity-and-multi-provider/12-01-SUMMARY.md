---
phase: 12-broadcast-parity-and-multi-provider
plan: 01
subsystem: monitor/broadcast
tags: [broadcast, orphan-mempool, input-restoration, parity]
dependency_graph:
  requires: []
  provides: [PostReqStatus::Orphan, immediate-input-restoration-invalidtx]
  affects: [monitor-helpers, broadcast-outcome]
tech_stack:
  added: []
  patterns: [orphan-mempool-transient-retry, spent_by-based-input-restoration]
key_files:
  created: []
  modified:
    - src/monitor/helpers.rs
    - src/signer/broadcast_outcome.rs
decisions:
  - "Task 3 (orphan cascade arm) already satisfied by Task 2 mapping orphan to ProvenTxReqStatus::Sending, which has an existing cascade arm"
  - "InvalidTx input restoration uses spent_by field to find consumed inputs, matching default_signer create_action pattern"
  - "DoubleSpend input restoration deferred to Plan 02 (needs chain verification)"
metrics:
  duration: "3m 47s"
  completed: "2026-04-13T19:10:25Z"
  tasks_completed: 4
  tasks_total: 4
  tests_passed: 601
  tests_failed: 0
---

# Phase 12 Plan 01: Broadcast Correctness Fixes Summary

Orphan mempool classification separated from invalid-tx in monitor broadcast aggregation, with immediate input restoration for permanently failed InvalidTx broadcasts using spent_by-based output lookup.

## Completed Tasks

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add PostReqStatus::Orphan variant | 7d77943 | src/monitor/helpers.rs |
| 2 | Fix orphan classification in attempt_to_post_reqs_to_network | e98a65e | src/monitor/helpers.rs |
| 3 | Add orphan arm to tx status cascade | (covered by Task 2) | src/monitor/helpers.rs |
| 4 | Immediate input restoration for InvalidTx | 397efad | src/signer/broadcast_outcome.rs |

## What Changed

**PostReqStatus::Orphan (Tasks 1-3):** Added an `Orphan` variant to `PostReqStatus` and wired orphan mempool detection into the broadcast aggregation loop. Previously, orphan mempool results fell through to `status_error_count` which mapped to `ProvenTxReqStatus::Invalid` (permanent failure). Now orphan results are tracked via `orphan_count` and map to `ProvenTxReqStatus::Sending` (transient retry), which is MORE correct than TS (which lets orphan fall to serviceError).

**InvalidTx input restoration (Task 4):** When a broadcast is permanently rejected as InvalidTx, all outputs consumed as inputs by that transaction (found via `spent_by = tx.transaction_id`) are immediately restored to `spendable=true`. Previously this was deferred to the monitor's unfail/review tasks on the next cycle. DoubleSpend input restoration remains deferred to Plan 02 which adds chain verification.

## Deviations from Plan

### Task 3: Already satisfied

**Task 3** specified adding `PostReqStatus::Orphan => Some(TransactionStatus::Sending)` to the tx status cascade. However, the cascade matches on `ProvenTxReqStatus`, not `PostReqStatus`. Task 2 already maps orphan results to `ProvenTxReqStatus::Sending`, which has an existing cascade arm mapping to `TransactionStatus::Sending`. No additional code change was needed.

## Verification

- `cargo build --all-targets`: PASSED
- `cargo test`: 601 passed, 0 failed

## Self-Check: PASSED

All files exist, all commits verified.
