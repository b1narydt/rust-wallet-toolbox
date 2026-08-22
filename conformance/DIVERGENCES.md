# Official-dispatcher divergence ledger

This ledger adjudicates the 77 divergences originally reported by the five
437-vector runners added on `feat/conformance-official-vectors`. Conformance is
the vector corpus plus the assertion performed by the official TypeScript
dispatcher, not an exact comparison of every fixture field.

The official report records all 77 vector IDs as passing. The relevant
dispatcher evidence is:

- `wallet.ts` lines 454-476 asserts encrypt round-trip because the IV is random.
- `wallet.ts` lines 372-447, 464-495, and 505-535 uses `rejects.toThrow()` for
  crypto/linkage error vectors; it does not compare error identity.
- `wallet.ts` lines 528-535 asserts specific-linkage result properties plus the
  deterministic `prover`, `counterparty`, `protocolID`, and `keyID` fields.
- `wallet.ts` lines 541-623 implements state methods as stubs and explicitly
  weakens or omits assertions for scenario-only vectors.

## Re-adjudication summary

| Classification | Count | Disposition |
|---|---:|---|
| Runner over-assertion | 51 | Removed; assertions now mirror the dispatcher. |
| `RUST_DEFECT` | 24 | Fixed in the Rust SDK; pins removed after all vectors passed. |
| `SPEC_AMBIGUOUS` | 2 | Still executed and individually pinned below. |

Five of the 51 false divergences are specifically `TS_STUB_ARTEFACT` cases:

| Vector ID | Stated expectation | Rust observation | Verdict and evidence |
|---|---|---|---|
| `wallet.brc100.getversion.4` | service-unavailable error | A configured wallet returns its version. | `TS_STUB_ARTEFACT`: `wallet.ts:620` returns without invoking a state layer. |
| `wallet.brc100.getheight.5` | `ERR_NOT_AUTHENTICATED` | With an artificial unauthorized service, Rust collapses the service error to SDK `Internal`. | `TS_STUB_ARTEFACT`: `wallet.ts:571` never invokes a state layer, and the synthetic service does not establish a real wallet authentication state. |
| `wallet.brc100.isauthenticated.3` | `{ authenticated: false }` for a locked session | Rust has no constructed locked session and returns a boolean `true`. | `TS_STUB_ARTEFACT`: `wallet.ts:545-553` asserts boolean shape only. |
| `wallet.brc100.waitforauthentication.4` | authentication-timeout error | No timeout/session state is constructed; Rust returns immediately. | `TS_STUB_ARTEFACT`: `wallet.ts:560-562` performs no assertion. |
| `wallet.brc100.waitforauthentication.5` | wallet-process-close error | No process-close state is constructed; Rust returns immediately. | `TS_STUB_ARTEFACT`: `wallet.ts:560-562` performs no assertion. |

They are not Rust defects: the official TypeScript pass is against a stub, and
neither dispatcher creates the state named by the scenario. They remain
explicit at their method branches in `tests/conformance_brc100_info.rs` rather
than being hidden by a blanket skip.

## Resolved SDK defect

The 24 `wallet.brc100.revealspecifickeylinkage.*` divergences were a real Rust
defect found by this corpus. The SDK incorrectly refused `self` and `anyone`
after completing the cryptography. The SDK now returns the original
`Counterparty` and serializes it with the wire representation (`"self"`,
`"anyone"`, or compressed public-key hex); all 24 formerly pinned vectors pass
and have been removed from the divergence ledger.

## Remaining divergences

| Vector ID | Official expectation | Rust behaviour | Verdict |
|---|---|---|---|
| `wallet.brc100.getheaderforheight.2` | Exact Bitcoin genesis header for height 0. | BRC-100 argument validation rejects height 0 before services are called. | `SPEC_AMBIGUOUS`: the corpus/dispatcher accepts genesis height 0, but BRC-100 types `height` as `PositiveInteger` excluding zero. |
| `wallet.brc100.getheaderforheight.8` | Exact Bitcoin genesis header for height 0. | BRC-100 argument validation rejects height 0 before services are called. | `SPEC_AMBIGUOUS`: same normative type/vector conflict; the vector note itself acknowledges implementations starting at 1. |

Both rows execute. The source ledger fails if an ID unexpectedly appears or
disappears; neither is listed in `RUST_RUNNERS.json` as skipped.
