# Cross-impl conformance vectors

Vendored copies of the BRC conformance vectors maintained in
[`bsv-blockchain/ts-stack`](https://github.com/bsv-blockchain/ts-stack) under
`conformance/vectors/`, pinned via `conformance/SOURCE`. The reference
implementation is `@bsv/sdk@2.0.14 + wallet-toolbox` (TypeScript); the Go
implementation (`bsv-blockchain/go-wallet-toolbox`) runs the same corpus.

Tests embed the JSON at compile time via `include_str!` — no network fetch at
test time, no sibling-repo filesystem path. Tests stay hermetic and work
identically for every cloner.

## Why vendor?

- **Determinism** — every commit pins the exact vector revision the code
  passes against.
- **Offline / CI hermeticity** — no GitHub fetch during `cargo test`.
- **Diffability** — vector changes show up in PR review.

## Runners

| Vectors | Runner |
|---|---|
| `vectors/wallet/brc100/getpublickey.json` (201) | `tests/conformance_getpublickey.rs` — BRC-42/43 key derivation |
| `vectors/sync/brc40-user-state.json` (24) | `tests/conformance_brc40.rs` — BRC-40 sync semantics |
| `vectors/wallet/brc100/{createaction,signaction,internalizeaction,relinquishoutput}.json` (90+8+10+8) | `tests/conformance_brc100_actions.rs` — action/write surface. The upstream reference never executed these channels; see the runner header for the executable characterization of the synthetic expected values and the pinned divergences. |
| `vectors/wallet/brc100/createaction-funded.json` + `signaction-funded.json` | `tests/conformance_brc100_funded.rs` — byte-for-byte offline replay of vectors **recorded** against a real faucet-funded wallet on mainnet (see below) |
| `vectors/wallet/brc100/{listoutputs,listactions,provecertificate,getnetwork}.json` (144+16+8+5) | `tests/conformance_brc100_read.rs` — read surface |
| `vectors/wallet/storage/adapter-conformance.json` (18) | `tests/conformance_storage_adapter.rs` — storage adapter contract |

Each runner asserts the exact number of vectors loaded and executed, names the
vector `id` in every failure, and runs error vectors as first-class assertions
(the operation must fail, for the stated reason). Divergences from the
reference that we deliberately do not paper over are pinned in an explicit
per-runner ledger keyed by vector `id` — the test fails if a divergence
appears, disappears, or changes shape, so the ledger cannot drift silently.

Every vendored channel now has a runner.

## The funded createAction/signAction corpus

The vendored upstream `createaction.json` (90) and `signaction.json` (8) carry
**fabricated** expected values — every `createaction` expected.tx is a
zero-input transaction no wallet can produce (nothing spent, no fee), and the
signaction vectors reference in-flight actions that never existed. Upstream's
own metadata says so (`skip_reason: "Requires funded wallet…"`); the TS
dispatcher demotes all 98 to intended-skip and the Go toolbox has no runner at
all. They stay vendored, unmodified, as provenance.

`createaction-funded.json` and `signaction-funded.json` replace fiction with
recordings: a real wallet, funded by the PeerPay faucet on mainnet, replayed
every vector's args, and the files pin what actually happened — real txids,
real signed bytes, real fees, real errors. Each vector carries its funding as
AtomicBEEF internalize fixtures plus the pinned merkle roots those BEEFs prove
against, so `tests/conformance_brc100_funded.rs` rebuilds the wallet from
nothing and must reproduce every recorded byte with zero network access (the
harness services panic on any outbound call). Entropy is seeded per vector
(`utility::conformance_entropy`); ECDSA nonces are RFC 6979.

`funded-ledger.json` records every mainnet action of the recording run:
faucet requests per burner identity, every broadcast, and which vectors were
broadcast-verified on the network. Recorder: `tests/record_funded_vectors.rs`
(`#[ignore]`, gated on `BSV_FUNDED_RECORD=1`, spends real satoshis).

Known upstream corpus defects, pinned as recorded vectors (`corpus-defect`
tag): every upstream vector claims the reserved basket `default` on an action
output (BRC-100 reserves it; a conforming wallet rejects it), and upstream
places `noSend`/`acceptDelayedBroadcast` at args top level where every
implementation's parser silently drops them (BRC-100 defines them inside
`options`). `expected.noSendTxid` is a field no implementation's
`CreateActionResult` has; the recorded files do not carry it.

## Refreshing

Fetch the tracked files from `raw.githubusercontent.com` at the new upstream
SHA, update `conformance/SOURCE` (`upstream_sha`, `fetched_at`), and re-run:

```sh
cargo test --features sqlite --test conformance_getpublickey
cargo test --features sqlite --test conformance_brc40
```

If new vectors land that this implementation does not satisfy, fix the
implementation or record the divergence in the runner's ledger with the
failing vector ID — never shrink an assertion to make a vector pass.
