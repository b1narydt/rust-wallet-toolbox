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

Each runner asserts the exact number of vectors loaded and executed, names the
vector `id` in every failure, and runs error vectors as first-class assertions
(the operation must fail, for the stated reason). Divergences from the
reference that we deliberately do not paper over are pinned in an explicit
per-runner ledger keyed by vector `id` — the test fails if a divergence
appears, disappears, or changes shape, so the ledger cannot drift silently.

Not yet wired (vendored for future runners): `createaction.json`,
`listoutputs.json`, `listactions.json`, `internalizeaction.json`,
`provecertificate.json`, `relinquishoutput.json`, `signaction.json`,
`getnetwork.json`, `wallet/storage/adapter-conformance.json`.

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
