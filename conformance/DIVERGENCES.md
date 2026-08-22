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
| `RUST_DEFECT` | 24 | Still executed and individually pinned below. |
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

## Remaining divergences

For BRC-100 call code 10, the wire definition admits `self` and `anyone`
counterparty sentinels. `ProtoWallet.ts` lines 186-212 derives the specific
secret, encrypts the linkage and proof, and returns a result for these vectors;
`wallet.ts` lines 528-535 observes the required result properties. Rust instead
reaches `proto_wallet.rs` lines 438-445 and returns
`InvalidParameter("counterparty public key required for linkage revelation")`
before it can return any result. That is a Rust conformance defect even though
the official dispatcher intentionally does not compare randomized ciphertext.

| Vector ID | Official expectation | Rust behaviour | Verdict |
|---|---|---|---|
| `wallet.brc100.revealspecifickeylinkage.1` | Result with `prover`, `encryptedLinkage`, `encryptedLinkageProof`, and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.2` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.3` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.4` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.5` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.6` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.13` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.14` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.15` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.16` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.17` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.18` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.19` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.20` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.21` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.22` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.23` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.24` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.31` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.32` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.33` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.34` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.35` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.revealspecifickeylinkage.36` | Result with required properties and matching deterministic fields. | Returns `InvalidParameter`; no result object. | `RUST_DEFECT` |
| `wallet.brc100.getheaderforheight.2` | Exact Bitcoin genesis header for height 0. | BRC-100 argument validation rejects height 0 before services are called. | `SPEC_AMBIGUOUS`: the corpus/dispatcher accepts genesis height 0, but BRC-100 types `height` as `PositiveInteger` excluding zero. |
| `wallet.brc100.getheaderforheight.8` | Exact Bitcoin genesis header for height 0. | BRC-100 argument validation rejects height 0 before services are called. | `SPEC_AMBIGUOUS`: same normative type/vector conflict; the vector note itself acknowledges implementations starting at 1. |

All 26 rows execute. The two source ledgers fail if an ID unexpectedly appears
or disappears; none is listed in `RUST_RUNNERS.json` as skipped.
