# Phase 11 — Universal Test Vectors & Final Verification

## Status: PARTIAL — BRC-100 vectors blocked on SDK upgrade

## What passed

- **594 tests**, all green (378 lib + 216 integration/doc)
- `cargo build --all-targets` clean
- `cargo fmt --check` clean
- 14 existing serialization parity tests in `tests/serialization_parity_tests.rs` pass
  - camelCase field naming
  - None field omission
  - Wire format round-trip for StorageCreateActionArgs, StorageProcessActionArgs, etc.

## What's blocked

**BRC-100 universal test vectors cannot be wired up** with the current `bsv-sdk 0.2.5` dependency.

The 62 BRC-100 vector files (in `universal-test-vectors/generated/brc100/`) test wire-format
serialization of wallet interface types (`CreateActionArgs`, `SignActionArgs`, etc.) using
`to_binary()`/`from_binary()` methods that live in `bsv-sdk`'s `wallet::wire` module.

That module exists in `bsv-sdk 0.3.4+` (visible in `calhooon-bsv-rs/src/wallet/wire/`) but
does NOT exist in `bsv-sdk 0.2.5` (our current dependency). Upgrading bsv-sdk is a separate
initiative — it touches every type in the codebase and should not be done as part of M2.

### Vector format reference

Each vector file has `{ "json": {...}, "wire": "hex_bytes" }`. Testing requires:
1. Deserialize JSON → Rust type
2. Serialize Rust type → wire bytes via `WireWriter`
3. Compare hex output to expected `wire` field

Without `WireWriter` from bsv-sdk 0.3.4+, step 2 is impossible.

## Cross-reference verification (manual)

### vs. TypeScript wallet-toolbox

| Feature | TS | Rust | Match |
|---------|----|----|-------|
| TaskReviewDoubleSpends | Yes (12min trigger, unfails false positives) | Yes (same pattern) | Yes |
| TaskReviewProvenTxs | Yes (10min, merkle root audit) | Partial (structure ready, needs chaintracks) | Partial |
| TaskReviewUtxos | Yes (disabled, manual trigger) | Yes (same pattern) | Yes |
| Spend lock | Implicit (Node.js single-threaded) | Explicit `Arc<Mutex<()>>` | Adapted |
| TaskNewHeader trigger | Always true | Always true | Yes |
| TaskNewHeader shared state | Full header via monitor | AtomicU32 height + check_now flag | Partial |
| processNewBlockHeader | Sets checkNow on TaskCheckForProofs | Sets check_now + last_new_header_height | Yes |
| Structured tracing | makeLogger pattern | tracing crate spans | Adapted |

### vs. Calhooon wallet-toolbox (Rust)

| Feature | Calhooon | Our impl | Match |
|---------|----------|----------|-------|
| Spend lock | `Arc<Mutex<()>>` on all 4 spend methods | Same | Yes |
| NewHeaderTask | AtomicBool flag shared with CheckForProofs | Same pattern (AtomicBool + AtomicU32) | Yes |
| ReviewDoubleSpends | Not a separate task (inline in SendWaiting) | Separate task (matches TS) | TS parity |
| ReviewProvenTxs | Not a separate task (split across Check/Review) | Separate task (matches TS) | TS parity |
| ReviewUtxos | Not a task | Separate task (matches TS, disabled) | TS parity |

## Remaining gaps

1. **BRC-100 vectors**: Blocked on bsv-sdk 0.3.4+ upgrade
2. **TaskReviewProvenTxs merkle audit**: Needs chaintracks integration for height-by-height header lookup
3. **TaskNewHeader shared state**: Shares height via AtomicU32 but not full BlockHeader struct (would need Arc<Mutex<Option<BlockHeader>>> for merkle_root, hash fields)

## M2 commit history

| Commit | Phase | Description |
|--------|-------|-------------|
| `5aef0b6` | 07 | feat(monitor): add 3 review tasks |
| `b898f5a` | 08 | fix(wallet): add spend lock |
| `210d77f` | 09 | fix(monitor): align TaskNewHeader |
| `30ccf5e` | 10 | feat(wallet): add structured tracing |

## Recommendation

M2 parity fixes are complete for what's achievable with bsv-sdk 0.2.5. A future milestone
should:
1. Upgrade bsv-sdk to 0.3.4+ (coordinate with Ishaan)
2. Wire up all 62 BRC-100 vector tests
3. Complete TaskReviewProvenTxs merkle audit with chaintracks
