# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-08-07

Breaking. Signing calls now say who they are made on behalf of.

A wallet authenticates its caller at the transport, then loses that identity
before signing, because no signer seam carries it. When the signer is a
different trust domain (an MPC cosigner below the seam), it enforces blind —
applying the union of every grant on the vault instead of just that caller's.
The reference TS implementation has the identical hole and avoids the cost by
enforcing above the signer, in the same trust domain; a provider in another
trust domain cannot.

### Added

- **`SigningContext` / `CallerRef`** (`signer::signing_context`) — per-call
  context threaded to the signing seam. `caller` is deliberately **not** an
  `Option`: a wallet acting for itself says `CallerRef::Itself` explicitly, so
  a call site that forgets cannot silently inherit the wallet's own authority.
  `CallerRef::Authenticated(String)` carries the principal the transport
  authenticated, and **the toolbox never interprets the string** — a
  browser-substrate wallet puts an origin domain there, a mutually
  authenticated server boundary an identity public key; the provider decides
  what it means. That makes the channel usable by a domain-keyed wallet
  without a further breaking change. It is deliberately not the BRC-100
  `originator`, which is specified and validated as an FQDN, so a value of
  another shape cannot ride in it. The struct is `#[non_exhaustive]` — that
  does not make this change non-breaking (nothing does); it makes the *next*
  field free. `host` is an opaque `Option<Arc<dyn Any + Send + Sync>>` the
  toolbox never inspects, for authorization artifacts whose shape it must not
  know.

- **`ContextualWallet`** — object-safe companion trait to `WalletInterface`
  (which belongs to `bsv-sdk` and is the BRC-100 surface, so it cannot carry
  the context) with `create_action_in` / `sign_action_in` taking a
  `&SigningContext`. Implemented for `Wallet` and for `WalletArc` (the
  `Clone`-able shape auth middleware holds — exactly the transport that
  authenticates callers). Purely additive: `WalletInterface::create_action` /
  `sign_action` delegate with `SigningContext::itself()`, so no BRC-100 client
  can observe it.

### Changed

- **BREAKING — `SigningProvider`'s ceremony-driving methods take
  `ctx: &SigningContext`**: `sign_input`, `prepare_spend_contexts`, and
  `create_signature`. The ECDH-only derivation methods
  (`derive_change_locking_script`, `derive_wallet_payment_locking_script`,
  `derive_public_key`, `derive_symmetric_key`) and `identity_public_key` are
  unchanged — they produce public data or keys, not signatures, and drive no
  ceremony. `StandardSigningProvider` ignores the context: it signs from a
  locally-held root key, the same trust domain as the wallet.

- **BREAKING — the signer pipeline threads the context.**
  `WalletSigner::create_action` / `sign_action`, `signer_create_action`,
  `signer_sign_action`, `build_signable_transaction_with_provider` and
  `complete_signed_transaction_with_provider` all take a `&SigningContext`.
  It is distinct from the existing `auth: &str` (`AuthId`), which is the
  wallet's own identity scoping storage rows. The context is deliberately not
  stored in `PendingSignAction`: a deferred ceremony belongs to the caller
  completing it, not the one that deferred it. On the BRC-100
  `createSignature` path the wallet passes `SigningContext::itself()` to the
  provider — that surface has no caller parameter.

- crates.io reports zero reverse dependencies for `bsv-wallet-toolbox`, so no
  published crate breaks; private consumers are unverifiable and any
  out-of-tree `SigningProvider` or `WalletSigner` implementation must add the
  `ctx` parameter.

### Added — portable export, and a recovery path with evidence

The wallet database is key material, not a cache. BRC-42 output derivation is
not enumerable, so a lost `derivation_prefix` leaves UTXOs unspendable with
every key in hand. That makes export and restore custody-grade features.

- **`storage::portable`** — BRC-38 (portable wallet document) and BRC-39
  (Argon2id + AES-256-GCM container): `export_brc38`, `encrypt_brc39`,
  `decrypt_brc39`, `import_brc38` in `Restore` (empty target, primary keys
  preserved) or `Merge` (into a populated store) mode.

  Verified against the TypeScript implementation in **both directions** with
  committed fixtures, because a self-consistent bug round-trips perfectly. A
  TS-produced container with fixed salt/nonce is **re-encrypted byte-identically**
  by Rust — empirical proof of the Argon2id parameters and the TS SDK's
  non-standard GHASH framing. A TS container encrypted under an NFD password
  decrypts under the NFC form, proving normalization. A Rust-produced container
  round-trips through TS restore and re-export.

  Known limits, documented in-code: restore is SQLite-only (merge works on all
  backends); `canonicalize` rejects non-integer numbers, where TS stringifies
  any finite float — ECMAScript float formatting is not reproducible without a
  JS `Number::toString` port, so Rust fails loudly rather than silently emitting
  different bytes.

- **`WalletBuilder::with_backup_sqlite` / `with_backup_mysql` /
  `with_backup_postgres` / `with_backup_provider`** — `WalletStorageManager`
  always supported backup stores; the builder passed it an empty vector, so the
  machinery was unreachable. Replication runs at build time and on `set_active`
  / `add_wallet_storage_provider`. It is **not periodic** — TS's
  `TaskSyncWhenIdle` is a stub and this port is faithful to that; call
  `WalletStorageManager::update_backups` to bring a replica current.

- **`tests/recovery_proof.rs`** — from a BRC-39 container and a password alone,
  with a `WalletServices` that panics on contact, a wallet is restored and a
  spend of a restored output is validated through the script interpreter. Not a
  row count: a restore can return rows with wrong derivation coordinates and
  produce a wallet that owns nothing.

### Fixed — storage

- **Cross-tenant reads and writes on the authenticated storage surface.** Six
  `*_auth` methods of `WalletStorageProvider` accepted an `AuthId` and
  discarded it, returning or mutating rows for every user in the database.
  `outputs` rows carry the complete BRC-42 derivation coordinates, so in a
  multi-user database this disclosed another tenant's entire output set.

  Enforcement is now a capability: `UserScope` has a private field and one
  constructor, which resolves the transport-verified identity key and rejects a
  mismatching `user_id` claim. `StorageReader` stays deliberately
  tenant-unscoped — the monitor sweeps every user and `proven_txs` is
  deduplicated chain data shared across users.

  Also: `find_certificates_auth` silently ignored a foreign `user_id` instead of
  rejecting it and dropped the `certificate_id` filter; `relinquish_*` now scope
  their find, so a cross-tenant attempt is indistinguishable from "not found".

- **Writes failed instead of waiting under load.** All writes funnel through a
  single-connection writer pool whose acquire timeout was the 5s *connect*
  timeout, so at high concurrency roughly half of all writes hard-failed with
  `PoolTimedOut`. Now a configurable `write_acquire_timeout` (default 60s).

- **SQLite pragmas configured only one connection.** They were issued as pool
  queries, so replacement connections came up without them. Now set in connect
  options. `synchronous` is configurable and defaults to `FULL`; the measured
  cost against `NORMAL` is within noise, because fsync amortizes over large
  commits.

- **The process-global spend lock was held across broadcast**, so every spend,
  internalize and abort queued behind a network call. Narrowed to UTXO
  allocation and `process_action`'s check-then-act; signing and broadcast run
  outside it.

- **Storage-to-storage sync parity**: the persisted `idMap` was discarded every
  chunk, so foreign→local ID mappings never accumulated; an unmapped required FK
  silently fell back to the raw foreign id, attaching rows to unrelated local
  records; and the sync window advanced per chunk while offsets reset, skipping
  rows.

## [0.4.0] - 2026-07-28

Breaking. `Wallet` no longer hard-codes its custody model.

### Added

- **`WalletArgs::signing_provider`** — an optional
  `Arc<dyn SigningProvider>`. When set, `create_action`, `sign_action` and
  `internalize_action` route every BRC-29 change-output derivation and every
  input signature through the provider, and the wallet never reaches for a root
  private key on those paths. When `None`, behaviour is exactly as in 0.3.x.
  Exposed on the builder as `WalletBuilder::with_signing_provider`.

  This is what the TS reference always allowed by handing `Wallet` a
  `WalletSigner` the caller had already built. The Rust port instead
  constructed its own signer and passed `None` for the provider, so it
  supported exactly one custody model — a local root key — and every other
  backend (cloud HSM, MPC coordinator) had to bypass `Wallet` and re-implement
  BRC-100 alongside it. The provider pipeline had existed and been complete
  since 0.3.1; nothing in the crate ever called it.

  The provider is injected **into** the wallet's single `DefaultWalletSigner`,
  not wrapped around it, so `createAction` and `signAction` keep sharing one
  `pending_sign_actions` map. A second signer bolted alongside would split that
  map and `signAction` would never find its reference.

- **`WalletBuilder::key_deriver`** — supply the deriver directly instead of a
  root private key, for a wallet whose identity key is a joint or threshold
  public key. Pair it with a signing provider; such a deriver cannot lock or
  sign anything itself.

- **`SigningBackend`** — the enum the signer pipeline dispatches on, `Local`
  (root-key derivation in-process) or `Delegated` (a `SigningProvider` answers
  everything).

### Changed

- **BREAKING — the key deriver is a trait object.** `WalletArgs::key_deriver`,
  `Wallet::key_deriver` and `SetupWallet::key_deriver` are now
  `Arc<dyn KeyDeriverApi>` (the object-safe trait added in `bsv-sdk` 0.3.4,
  implemented for both `KeyDeriver` and `CachedKeyDeriver`). The concrete
  `Arc<CachedKeyDeriver>` unsize-coerces at construction, so most call sites are
  unchanged; `&wallet.key_deriver` becomes `wallet.key_deriver.as_ref()`.
  Threaded through `build_signable_transaction`, `complete_signed_transaction`,
  `signer_internalize_action`, `get_key_pair`, `get_lock_p2pkh` and
  `create_p2pkh_outputs`, which now take `&dyn KeyDeriverApi`. Without this an
  identity key could not be decoupled from a locally-held root key at all.

- **BREAKING — one createAction, two custody modes.**
  `signer_create_action_with_provider` and its module are deleted;
  `signer_create_action` and `signer_sign_action` now take a `&SigningBackend`.
  The provider variant was a near-verbatim copy of the local one and the two had
  already drifted (the copy logged post-broadcast status-update failures, the
  original swallowed them). The merged path keeps the logging.

- **BREAKING — `DefaultWalletSigner::new`** takes a sixth argument, the
  optional signing provider.

- **BREAKING — `StandardSigningProvider::new`** takes
  `Arc<dyn KeyDeriverApi>` instead of an owned `CachedKeyDeriver`, and
  `key_deriver()` returns `&dyn KeyDeriverApi`.

- **BREAKING — `Wallet::get_client_change_key_pair`** returns
  `WalletResult<KeyPair>` and errors when a signing provider is injected.
  `Wallet::sweep_to` likewise refuses. Both lock BRC-29 outputs with
  `key_deriver.root_key()`, which under delegation is not the key that locks the
  wallet's coins — they now fail closed instead of producing outputs nobody can
  spend. `SigningProvider` has no lock-to-counterparty member to route them
  through.

- **BREAKING — `Wallet::pending_sign_actions` is removed.** The field was
  written once at construction and never read; the live map has always been the
  signer's. Removing it makes "there is exactly one map" structural rather than
  a thing to remember.

### Notes

- **The provider does not cover the crypto surface.** The nine `WalletInterface`
  crypto methods (`get_public_key`, `encrypt`, `decrypt`, `create_hmac`,
  `verify_hmac`, `create_signature`, `verify_signature`,
  `reveal_counterparty_key_linkage`, `reveal_specific_key_linkage`) still
  delegate to a `ProtoWallet` built from `key_deriver.root_key()`. A caller
  whose root key is a throwaway must wrap the wallet and intercept them.
- **`internalize_action`** keeps its documented fallback: a provider returning
  `Ok(None)` from `derive_wallet_payment_locking_script` falls back to local
  derivation. A provider without a usable root key behind its deriver must
  implement that method, or incoming wallet payments are rejected (fail-closed,
  not fund loss).

## [0.3.4] - 2026-07-24

### Changed

- **Monitor ON by default, STARTED by `build()`** — `WalletBuilder` now enables
  the background monitor by default, and `build()` both creates and starts it
  whenever services are configured. Rationale: `createAction` (with the default
  `acceptDelayedBroadcast`) stores every new transaction as an *unsent*
  `ProvenTxReq`, and only the monitor's `TaskSendWaiting` ever broadcasts it — a
  wallet built without a monitor silently never posts a transaction to the
  network. `with_monitor()` is kept as a back-compat no-op; `without_monitor()`
  is the explicit opt-out for callers that own broadcasting (or offline tests).
  Do not call `start_tasks()` on a `WalletBuilder`-produced monitor — it is
  already running and errors with "monitor tasks are already running". Monitors
  constructed manually via `Monitor::builder()` still require `start_tasks()`.

### Fixed

Parity-audit MUST fixes vs `@bsv/wallet-toolbox` (all TS-parity ports;
follow-ups F5–F11 tracked in #34):

- **Panic-isolated task loop** — one panicking task (or a non-char-boundary
  log-truncation slice) killed the whole spawned monitor loop while
  `running` stayed true, silently stopping all broadcasting. New
  `run_task_isolated` (per-task `catch_unwind`, mirroring TS
  `Monitor.runTask` try/catch) plus a char-boundary-safe `truncate_log`.
- **`attempts` persisted** — the per-req attempt counter was never written back
  (in the no-proof branch of `get_proofs` and the serviceError post arm), so
  the proof-timeout breaker could never fire and a broadcast-but-evicted tx was
  polled forever.
- **Rebroadcast-on-timeout** — ported `applyProofTimeout` with the TS-canonical
  was-broadcast-by-status inference: reqs whose status shows a provider
  accepted them (`unmined`/`callback`/`unconfirmed`/`completed`) are reset to
  `unsent` with `attempts = 0` so `TaskSendWaiting` re-broadcasts (TS default:
  no cycle cap); only never-broadcast reqs are marked `invalid`.
- **Double-spend restore gated per input** — DoubleSpend recovery previously
  restored ALL consumed inputs blindly, resurrecting genuinely-spent UTXOs.
  It now restores only inputs still unspent per `services.is_utxo`,
  fail-closed on lookup uncertainty; `InvalidTx` (never accepted anywhere)
  keeps restore-all.

## [0.3.3] - 2026-07-23

### Fixed

TS-parity critical+high tier fixes:

- **Full AtomicBEEF SPV verification on `internalize_action`** — structural and
  SPV (merkle proof) verification of the incoming BEEF; previously proofless
  BEEFs were accepted.
- **`is_utxo` endianness** — fixed a double-reverse in `hash_output_script`
  that made `is_utxo` lookups miss.
- **Nosend-change allocation guard** — unbroadcast noSend change is excluded
  from change allocation.
- **Output-substitution defense** (GHSA-36f9-7rg5-cpf8) — verification at the
  permissions layer that signed outputs match what was requested.
- **Local bulk header storage** — chaintracks SQLite bulk header store
  (migrate + serve); CDN ingest deferred as labeled backlog.
- **Storage-sync parity** — persisted `idMap` now accumulates across chunks
  (was rebuilt fresh every chunk, breaking foreign→local ID mapping); required
  FK remaps fail loudly on an idMap miss instead of silently attaching rows to
  unrelated records; the sync window (`since`/`when`) is held fixed per round
  with counts accumulating as offsets, advancing only on a completed round.
- **Signer / permissions / auth-manager / services parity batch** — GHSA
  injected-output and echo-verification defenses, `sign_input` single-hash and
  `sweep_to` encoding fixes (signer); cumulative-work fork choice
  (chaintracks); external-input BEEF resolution (`createAction`); PushDrop
  token, privileged-flag, and monthly-spend fixes (permissions); UMP
  root-derivation and Argon2id v3 (auth manager); proof-bearing BEEF builder
  (services).

## [0.3.2] - 2026-07-23

### Added

- **`SigningProvider::derive_wallet_payment_locking_script`** — defaulted
  (non-breaking) trait method letting a provider derive the expected BRC-29
  P2PKH locking script for an incoming wallet-payment output during
  `internalize_action` validation (issue #31). Needed for Q-as-root MPC vaults
  with no local root private key. `Some(script)` short-circuits the local
  `key_deriver` path; `Ok(None)` or no provider falls back unchanged; errors
  propagate (fail closed). `DefaultWalletSigner` passes `None` (identical
  behavior).

## [0.3.1] - 2026-07-12

### Fixed

- **Restored `SigningProvider::prepare_spend_contexts` on the 0.3 line.** 0.3.0
  was cut from the `feat/signing-provider` branch and published WITHOUT main's
  `prepare_spend_contexts` hook (shipped in 0.2.24), so it was a regression
  against 0.2.24 for any consumer implementing `SigningProvider`. 0.3.1 merges
  main into the 0.3 line, making the 0.3.x baseline `0.2.24 + the SigningProvider
  seam`. 0.3.0 is yanked; use 0.3.1 or later.

## [0.3.0] - 2026-04-18 [YANKED]

### Added

- **First 0.3.x cut of the `SigningProvider` seam** — the pluggable async
  signing-backend abstraction (threshold / remote / HSM) that lets a Q-as-root
  MPC vault sign without a local root private key, plus the provider-routed
  transaction builders (`build_signable_transaction_with_provider` /
  `create_action_with_provider`). Also CI clippy/rustfmt cleanup for Rust 1.95.

### Note

- **Yanked** — published from `feat/signing-provider`, which lacked main's
  `prepare_spend_contexts` hook (0.2.24), making it a regression for
  `SigningProvider` implementors. Superseded by 0.3.1.

## [0.2.24]

### Added
- `SigningProvider::prepare_spend_contexts` — a defaulted (no-op) hook invoked on the inline create-action signing path before signing, giving providers the unsigned transaction and per-input prevout data so they can capture per-input spend context (used by MPC cosigner integrations to bind signatures to the transaction). Backward compatible; existing implementors inherit the default.

## [0.2.23] - 2026-04-21

### Added

- **`SigningProvider` trait** — Async trait abstraction that lets the wallet
  work with any signing backend (threshold signing, remote signer, HSM, etc.)
  without changes to the transaction construction pipeline. Two methods:
  `sign_input` (produces unlocking scripts) and `derive_change_locking_script`
  (produces change outputs). Purely additive — existing APIs preserved.
  - `StandardSigningProvider` — reference implementation wrapping
    `CachedKeyDeriver` for the in-process signing path.
  - `build_signable_transaction_with_provider` /
    `complete_signed_transaction_with_provider` — async versions of the
    transaction builders that route signing through a `SigningProvider`.
  - `create_action_with_provider` — full `createAction` workflow using
    the provider abstraction, including `verify_unlock_scripts` before BEEF
    serialization, broadcast outcome classification and recovery handlers,
    `return_txid_only` gating, and `send_with` txid forwarding.

### Fixed

- **Broadcast status-update logging consistency** — The `Success` /
  `OrphanMempool` arm in the broadcast status update match was using
  `let _` to discard the `Result` while other arms logged errors via
  `tracing::error!`. All arms now log failures consistently.
- **`hex_to_bytes` error propagation** — Returns `Result` and propagates
  invalid-hex errors rather than panicking or silently producing empty
  bytes.
- **Transaction deserialization logging** — Failures are logged via
  `tracing` instead of being swallowed; missing `derivation_prefix` /
  `derivation_suffix` on change outputs are logged as warnings.

### Changed

- **`derive_change_locking_script` signature** — Removed redundant
  `identity_pub_key` parameter; the derivation key is already carried
  by the provider instance.
- **`StandardSigningProvider` internals** — Eliminated an unnecessary
  bytes → hex → bytes round-trip in the change-script derivation path.
- **CI lint cleanup** — Replaced the remaining `sort_by(|a, b| ...)` in
  `chaintracks/storage/memory.rs` with `sort_by_key(|b| Reverse(...))`
  so `cargo clippy -D warnings` passes on Rust 1.95 stable
  (`clippy::unnecessary_sort_by`). Signer-side lint fixups shipped in
  the same series.

## [0.2.22] - 2026-04-15

### Fixed

- **`merge_input_beef` silent failure elimination** — Changed return type from
  `()` to `WalletResult<()>` and replaced `let _ =` error suppression with
  proper `map_err()?` propagation. Previously, corrupt or malformed BEEF data
  would be silently swallowed, producing incomplete BEEF with no diagnostics.
  Affects caller-provided inputBEEF merge, storage-input BEEF merge, and BEEF
  serialization. **Breaking:** callers must now handle the `WalletResult`.

- **`wallet.rs` BeefParty merge logging** — Replaced `let _ =` with
  `log::warn!` on `merge_beef` failure so merge errors are observable.

## [0.2.21] - 2026-04-15

### Fixed

- **Recursive BEEF inputBEEF merging** — `collect_tx_recursive` and
  `collect_tx_recursive_reader` now merge stored `inputBEEF` from transaction
  records during recursive BEEF construction. Previously, ancestor proof chains
  stored on intermediate transactions (e.g. from received PeerPay payments) were
  lost at recursion depth > 1, producing incomplete BEEF in rapid-spend scenarios.
  This matches the TS SDK's `StorageProvider.getValidBeefForTxid` behavior where
  `beef.mergeBeef(r.inputBEEF)` is called at every recursion level.

## [0.2.19] - 2026-04-13

### Added

- **Chain-verified input restoration** — `utxo_verified_input_ids` queries chain
  via `get_status_for_txids` before restoring inputs on DoubleSpend. Only inputs
  whose parent txs are confirmed "mined" or "known" are restored to spendable.
  InvalidTx immediately restores all inputs (tx was malformed, not consumed).

- **Monitor event purge** — `purge_data` now cleans old `monitor_events` entries
  using configurable age threshold (default 30 days). Prevents unbounded growth.

- **Checkpoint cleanup** — `TaskReviewDoubleSpends` purges stale checkpoint
  entries from `monitor_events` after completing a full review cycle.

### Changed

- **Shared WalletStorageManager** — `WalletBuilder` now creates ONE
  `Arc<WalletStorageManager>` shared across wallet, monitor, signer, and setup
  return value. Adding a `StorageClient` to `setup.storage` is now visible to
  the wallet's internal storage. 17 files refactored.

- **Backup sync wiring** — `update_backups()` called after `set_active()` and
  `add_wallet_storage_provider()` to propagate changes to backup providers.

### Fixed

- **Monitor orphan mempool misclassification** — `attempt_to_post_reqs_to_network`
  now checks `orphan_mempool` flag and maps it to `ProvenTxReqStatus::Sending`
  (transient retry). Previously fell through to `Invalid` (permanent failure).
  New `PostReqStatus::Orphan` variant added.

### Verified

- All 8 StorageClient integration tests pass against both
  `staging-storage.babbage.systems` (TS reference) and `rust.b1nary.cloud`.
  BRC-31 auth, makeAvailable, findOrInsertUser, getSyncChunk, syncToWriter,
  updateBackups all wire-compatible.

### Hardening (post-review, PR #17)

External code review surfaced correctness, atomicity, and architectural
issues across the broadcast permanent-failure path, monitor tasks, and the
multi-provider Arc refactor. This release rolls all those fixes in.

#### Added

- **`restore_consumed_inputs(tx_id, trx)`** on `StorageReaderWriter`, with
  passthroughs on `WalletStorageProvider` and `WalletStorageManager`. Replaces
  the implicit cascade in `update_transaction_status` (see Changed below).
- **Storage transaction passthroughs** on `WalletStorageProvider` and
  `WalletStorageManager` — `begin_transaction`, `commit_transaction`,
  `rollback_transaction` plus trx-aware variants of `update_output`,
  `update_proven_tx_req`, `update_transaction_status`, `find_outputs`,
  `find_transactions`, `find_proven_tx_reqs`. Enables atomic multi-write
  paths through the manager facade. `StorageClient` correctly returns
  `NotImplemented` (remote backends cannot offer SQL transactions).
- **`WalletStorageManager::acquire_spend_lock()`** — fifth hierarchical lock
  alongside reader/writer/sync/sp. Serializes spend operations across all
  `Wallet` instances sharing the same `Arc<WalletStorageManager>`.
- **`ProvenTxReqPartial.attempts: Option<i32>`** — enables atomic
  status+attempts updates from the ServiceError retry path.
- **Helpers** `apply_service_error_outcome` and `apply_success_or_orphan_outcome`
  in `signer::broadcast_outcome` — extracted per-outcome state transitions
  for testability and de-duplication across `create_action`/`sign_action`.
- **Behavioral test coverage** — `tests/signer_flow_tests.rs` (9 tests:
  spend_lock concurrency, broadcast outcome branches, ServiceError retry,
  OrphanMempool retry path), `tests/multi_provider_arc_tests.rs` (5 tests:
  Arc sharing across components, set_active backup replication, error
  propagation), `tests/brc100_vectors.rs` (57 round-trip tests over
  `testdata/brc100/*` against bsv-sdk 0.2.81). Replaced 6 tautological
  monitor task tests with 14 behavioral ones. Total: 1069 tests, 0 failures.

#### Changed

- **`spend_lock` relocated** from per-`Wallet` field to `WalletStorageManager`.
  With multiple `Wallet` instances sharing one `Arc<WalletStorageManager>`
  (the intended pattern after the 0.2.19 multi-provider refactor),
  per-`Wallet` locks did not serialize cross-`Wallet` UTXO contention on
  shared storage. Now matches TS architectural placement (storage-manager-level
  serialization).
- **`StorageReaderWriter::update_transaction_status`** no longer cascades input
  restoration as a side effect when marking `Failed`. Callers (`process_action`,
  `monitor::helpers` post-broadcast, `monitor::tasks::task_fail_abandoned`) now
  invoke `restore_consumed_inputs` explicitly. Required to make DoubleSpend's
  chain-verified filter actually take effect; previously the cascade ran first
  and restored every consumed UTXO regardless of chain state.
- **Per-outpoint UTXO check** — `utxo_verified_input_ids` now calls
  `services.is_utxo(script, parent_txid, vout)` per outpoint instead of
  `get_status_for_txids(parent_txid)`. The previous tx-level check could not
  distinguish "parent exists" from "this specific output is unspent", so a
  real DoubleSpend caused the wallet to restore inputs the competing tx had
  already consumed. Mirrors Calhooon's per-outpoint pattern.
- **`bsv-sdk` pinned to crates.io `0.2`** — reverted from a git pin that was
  in place during BRC-100 vector iteration. Verified against `bsv-sdk 0.2.81`,
  which fixes the BRC-100 wire-format drifts tracked as
  `b1narydt/bsv-rust-sdk#24`.

#### Fixed

- **Atomic permanent-failure writes** — `handle_permanent_broadcast_failure`
  now opens a storage transaction, passes `Some(&trx)` to all writes
  (`update_transaction_status`, `update_proven_tx_req`, `update_output`),
  and commits at the end. Replaces five independent `let _ = storage.X(...)`
  calls that silently dropped errors and left partial DB state on failure.
- **Error propagation in broadcast handler** — replaced `.unwrap_or_default()`
  on `Result` returns from `find_outputs`, `find_transactions`,
  `find_proven_tx_reqs` with `?`. DB timeouts no longer surface as empty
  result sets that silently skip restoration. Caller arms now log the
  handler's `Result` via `tracing::error!` instead of dropping it.
- **`ServiceError` retry semantics** — `BroadcastOutcome::ServiceError` now
  transitions ProvenTxReq + Transaction to `Sending` and bumps `attempts` via
  `apply_service_error_outcome`. Previously the arm only emitted
  `tracing::warn!("(will retry)")` while leaving both at `Unprocessed` —
  a status TaskSendWaiting does not retry. Matches TS
  `attemptToPostReqsToNetwork.ts:240-244`.
- **`OrphanMempool` tx status** — `apply_success_or_orphan_outcome` now sets
  `Transaction::Sending` for OrphanMempool (matches the ProvenTxReq side and
  the module's own design comment) instead of `Transaction::Unproven`.
  Previously `list_outputs` and `list_actions` surfaced orphan-mempool txs
  as confirmed-pending.
- **`reconcile_tx_status` wrapper-level failure** — now checks
  `status_result.status != "success"` before iterating per-result statuses.
  On all-providers-failed, returns `BroadcastOutcome::ServiceError` (which
  chains into the retry path) instead of letting the original outcome
  propagate unchanged. Previously a chain-status query outage during
  reconciliation could lock in a spurious permanent failure.
- **`TaskReviewProvenTxs` ChainTracker outage handling** — replaced
  `.unwrap_or(false)` on `is_valid_root_for_height` with explicit `match`.
  On `Err`, log warn and `continue`; do not count outage as merkle mismatch.
  Brief tracker downtime no longer triggers `reprove_proven` on every healthy
  tx at the reviewed heights.
- **`update_backups` error propagation** — both `WalletStorageManager::set_active`
  and `add_wallet_storage_provider` now propagate `update_backups()` errors via
  `?` instead of silently logging via `warn!`. Backups silently desyncing after
  a successful active-store cutover was the previous failure mode. Mirrors
  Calhooon's `?` propagation pattern.

## [0.2.18] - 2026-04-13

### Added

- **Monitor tasks** — TaskReviewDoubleSpends (reviews false-positive double-spend
  flags with 60-minute age filter and checkpoint persistence to monitor_events),
  TaskReviewProvenTxs (audits proven_txs merkle roots against canonical chain via
  ChainTracker, reproves mismatches), TaskReviewUtxos (disabled by default, manual
  review via `review_by_identity_key`). All three match TS wallet-toolbox behavior.
  Closes #13.

- **Spend lock** — Per-wallet `Arc<tokio::sync::Mutex<()>>` serializing
  createAction, signAction, internalizeAction, abortAction, and relinquishOutput
  to prevent concurrent UTXO double-spend races. Matches Calhooon 5-method pattern.
  Closes #15 (Part A).

- **BroadcastOutcome classifier** — `classify_broadcast_results` function with
  `BroadcastOutcome` enum (Success/ServiceError/InvalidTx/DoubleSpend/OrphanMempool).
  Wired into create_action and sign_action signer methods. OrphanMempool keeps
  ProvenTxReq in Sending for monitor retry instead of treating as permanent failure.
  Closes #15 (Parts B+C).

- **Structured tracing** — `tracing::debug` on entry and `tracing::info` on
  completion for all four action methods with description, txid, and reference fields.

### Fixed

- **TaskNewHeader alignment** — `trigger()` now always returns true (matches TS —
  runs every scheduler cycle). Shares `last_new_header_height` with
  TaskCheckForProofs and TaskCheckNoSends for max acceptable height guard.
  Closes #14.

- **ARC orphan-mempool distinction** — `SEEN_IN_ORPHAN_MEMPOOL` is no longer
  conflated with `DOUBLE_SPEND_ATTEMPTED`. New `orphan_mempool` field on
  `PostTxResultForTxid` enables downstream classification.

### Changed

- **bsv-sdk dependency** — Updated to git main branch with complete BRC-100
  universal test vector coverage (62/62 vectors, byte-identical to Go/TS SDKs).

## [0.2.17] - 2026-04-12

### Fixed

- **Broadcast BEEF rebuild** — `attempt_to_post_reqs_to_network` now rebuilds
  the broadcast BEEF from authoritative storage state at send time (matching
  TS `StorageProvider.mergeReqToBeefToShareExternally`) instead of using the
  stored `input_beef` blob directly. Delayed broadcasts with partial or NULL
  stored `input_beef` were silently failing ARC with "script(N): got M bytes:
  unexpected EOF" errors. Source BEEF merge failures now set `missing_source`
  with a diagnostic log entry so corrupted stored proofs are observable
  rather than producing incomplete BEEFs that ARC rejects opaquely.

- **Transaction-row cascade on broadcast outcome** — Broadcast outcomes now
  cascade from `proven_tx_reqs.status` to the `transactions.status` row so
  that `list_outputs` (which filters on `TX_STATUS_ALLOWED`) sees broadcasted
  outputs as usable. Previously the tx row stayed at `unprocessed` forever,
  hiding broadcasted outputs from wallet queries. Service-error status maps
  to `Sending` (not `Unproven`) to avoid falsely signaling "broadcast
  accepted". Cascade failures surface via a new structured
  `cascade_update_failed` field on `PostReqDetail` rather than being buried
  in free-text logs.

- **Proof cascade to transactions row** — `update_proven_tx_req_with_new_proven_tx`
  now also updates the matching `transactions` row to `completed` +
  `provenTxId`, matching TS `processProvenTx`. Wrapped in a storage
  transaction for atomicity, with find-or-insert semantics on `proven_txs`
  to handle concurrent SPV ingest races. Without this, confirmed incoming
  payments were reported as unconfirmed indefinitely and confirmed change
  appeared unspendable.

- **BRC-29 derivation params base64-encoded in `internalize_action`** —
  Both the storage-layer and signer-layer paths were passing raw binary
  derivation bytes through `String::from_utf8_lossy`, corrupting non-UTF-8
  bytes with U+FFFD. The corrupted `key_id` then derived a different pubkey
  than the sender used to lock the output, causing BRC-29 P2PKH script
  matches to fail and subsequent UTXO spends to be rejected by ARC with
  error 461 "Script failed an OP_EQUALVERIFY operation". Fix mirrors the
  random-generator fix from 0.2.16 for the receive side of the storage
  path. Regression tests added for non-UTF-8 derivation byte sequences.

- **Monitor task `make_available` initialization** — Each task's private
  `WalletStorageManager` was never having `make_available()` called on it,
  so tasks using methods gated on `is_available_flag` (`TaskFailAbandoned`,
  `TaskReviewStatus`, `TaskPurge`, `TaskUnFail`) errored every run with
  "WalletStorageManager not yet available". Added a `storage_manager()`
  hook to `WalletMonitorTask` with a default `async_setup()` that calls
  `make_available()` on it — auto-initializes each task's storage on first
  tick. `make_available()` is idempotent.

- **`TaskSendWaiting` / `TaskCheckForProofs` race** — Added a
  `['completed', 'unmined']` status guard (matching TS) so `TaskSendWaiting`
  no longer clobbers state advanced concurrently by `TaskCheckForProofs`,
  which would knock a `Completed` req back to `Unmined`/`Sending` and a
  `Completed` tx back to `Unproven`.

### Internal

- GitHub Actions CI (fmt, clippy, test, doc, MSRV 1.87, feature-matrix
  compile check, `cargo publish --dry-run`) and release workflow (triggered
  on `v*.*.*` tag push with tag-vs-Cargo.toml version verification).
- README dependency examples switched from hardcoded versions to
  `cargo add` commands; the Crates.io badge is now the single source of
  truth for the current version.

## [0.2.16] - 2026-04-08

### Fixed

- **signAction broadcast results discarded** — `sign_action` was silently dropping
  `post_beef` results with `let _post_results`, leaving transactions stuck as
  `unprocessed` forever. Now mirrors `create_action`'s post-broadcast status updates
  (`unproven`/`unmined`), matching the TS shared `processAction` → `shareReqsWithWorld`
  code path.

- **UTXOs permanently locked after failed transactions** — When releasing UTXOs on
  tx failure, `spent_by` was set to `Some(0)` which wrote `spentBy = 0` to the DB,
  but UTXO selection filtered with `spent_by.is_none()`, permanently excluding released
  UTXOs. Fix: `spent_by = 0` now emits `spentBy = NULL` in SQL, and the secondary
  `spent_by.is_none()` filter was removed, matching TS which uses `spendable = true`
  as the sole availability gate.

- **`is_delayed` inversion** — `is_delayed` was set to `!accept_delayed`, but the TS
  reference assigns `isDelayed = acceptDelayedBroadcast` (no negation). With default
  `acceptDelayedBroadcast = true`, the inversion caused immediate broadcast instead of
  deferring to `TaskSendWaiting`.

- **Derivation prefix/suffix now base64** — Changed `random_hex_string()` from hex to
  base64 encoding (8 random bytes via `rand`), matching TS `randomBytesBase64(8)`.
  Fixes "derivationPrefix must be valid base64" BRC-42 validation errors.

- **`WalletStorageManager.get_auth()` auto-initializes** — Now calls `make_available()`
  instead of returning `WERR_NOT_ACTIVE`, matching TS and preventing "not yet available"
  errors from monitor tasks.

- **Monitor storage initialization** — Polling loop now calls `storage.make_available()`
  before tasks run, ensuring task storage clones are initialized on first iteration.

### Added

- **`ArcConfig.headers` field** — `headers: Option<HashMap<String, String>>` matching
  TS SDK's `headers?: Record<string, string>`. Custom headers (e.g.,
  `X-SkipScriptValidation`) are injected into all TX submissions.

- **ARC BEEF as JSON hex format** — `post_beef` now sends `{ "rawTx": "<hex>" }` with
  `Content-Type: application/json` instead of raw binary with `application/octet-stream`,
  matching the TS SDK's `postRawTx` pattern.

- **`get_tx_data` for multi-txid BEEF** — After posting BEEF, queries
  `GET /v1/tx/{txid}` for additional txids, matching TS SDK's `postBeef` pattern.

- **Random `deployment_id` default** — `default_deployment_id()` generates
  `rust-sdk-{16 hex chars}` matching TS SDK's `ts-sdk-{Random(16) as hex}` pattern.

- 4 new ARC integration tests: JSON content-type, custom headers, post body format,
  and serialization verification.

## [0.2.15] - 2026-04-06

### Fixed

- **ProvenTxReq status for internalized transactions** — `internalize_action` now
  sets ProvenTxReq status to `Unmined` instead of `Unsent` for externally received
  transactions, correctly reflecting that the transaction has already been broadcast
  by the sender.

### Changed

- `setup_wallet` example: `for_self` field now explicitly set to `Some(false)` in
  `create_action` call; `beef_bytes` cloned for reuse after internalization.

### Added

- **BEEF broadcast step in `setup_wallet` example** — After internalizing a
  payment, the example now broadcasts the BEEF to miners via `post_beef`,
  displaying per-miner broadcast status.

## [0.2.14] - 2026-04-04

### Changed

- Storage manager and wallet_provider trait updates.
- Merged examples feature branch (PR #10).
- Ran clippy and resolved all warnings.

## [0.2.13] - 2026-03-31

### Added

- **11 runnable examples** (`examples/`) demonstrating the full wallet lifecycle:
  - `setup_wallet` — Fund a Rust wallet via BRC-42 payment from a local
    BRC-100 desktop wallet (`HttpWalletJson`). Generates keys, persists
    config to `examples/.env`, internalizes the payment as `WalletPayment`.
  - `chaintracks_sync` — Sync block headers from WhatsOnChain into local
    `MemoryStorage`, display chain tip.
  - `chaintracks_validate` — SPV merkle root validation (real vs bogus root).
  - `p2pkh_transfer` — P2PKH transfer between two wallets.
  - `brc29_transfer` — BRC-29 key-derived transfer with receiver
    internalization.
  - `list_balance` — Query balance and list UTXOs via `balance_and_utxos()`.
  - `list_actions` — List transaction history with pagination.
  - `pushdrop_tokens` — Mint a PushDrop data-bearing token.
  - `nosend_batch` — Create 3 noSend transactions and batch send via
    `sendWith`.
  - `internalize_payment` — Receive and internalize an external BRC-29
    payment.
  - `backup_wallet` — Demonstrate backup sync architecture.

- New dev-dependency `dotenvy` for `.env` file loading in examples.

### Fixed

- **Transaction status not updated after broadcast** — `create_action`'s
  signer now updates Transaction status to `unproven` and ProvenTxReq status
  to `unmined` after calling `post_beef`. Previously, transactions stayed in
  `unprocessed` status permanently after broadcast, making their change
  outputs invisible to balance queries and UTXO selection. The TS
  implementation handles this in `shareReqsWithWorld`; the Rust
  implementation was missing this post-broadcast status transition.

- **UTXO selection excluded internalized outputs** — Removed erroneous
  `change: Some(true)` filter from `create_action`'s available change query.
  The TS `countChangeInputs`/`allocateChangeInput` query by `(userId,
  spendable, basketId)` without a `change` flag. The Rust filter excluded
  legitimately spendable outputs that entered the wallet via
  `internalizeAction` with `BasketInsertion`.

## [0.2.12] - 2026-03-30

### Added

- **Chaintracks module** — Full local block header management system ported from
  reference implementation (~7,500 lines, 152 tests). Replaces sole dependency on
  remote Babbage Chaintracks service with a local, embedded alternative.

  - **Two storage backends:** `MemoryStorage` (in-memory with 4 concurrent
    indexes) and `SqliteStorage` (persistent with partial indexes and batch
    insert).
  - **Four ingestors:** `BulkCdnIngestor` (Babbage CDN binary headers),
    `BulkWocIngestor` (WhatsOnChain API fallback), `LivePollingIngestor`
    (WoC REST polling), `LiveWebSocketIngestor` (WoC WebSocket real-time,
    feature-gated behind `chaintracks-ws`).
  - **Chaintracks orchestrator:** Background sync (500ms polling), header
    processing pipeline with double-SHA256 hash computation, chain
    reorganization detection and handling, subscriber callbacks for new headers
    and reorgs, readonly mode, chain validation.
  - **Trait hierarchy:** `ChaintracksStorageQuery` / `ChaintracksStorageIngest` /
    `ChaintracksStorage` for pluggable storage; `ChaintracksClient` /
    `ChaintracksManagement` for the orchestrator API; `BulkIngestor` /
    `LiveIngestor` for pluggable header sources.
  - **Services integration:** `ChaintracksChainTracker` now supports both
    `Remote` (existing HTTP client, default) and `Local` (new embedded
    chaintracks) backends via `ChaintracksBackend` enum. Existing behavior
    unchanged; local mode opt-in via `ChaintracksChainTracker::with_local()`.

- New feature flag `chaintracks-ws` for WebSocket live ingestor dependencies
  (`tokio-tungstenite`, `url`).

- New dependencies: `sha2`, `hex`, `uuid` (non-optional, used by chaintracks
  hash computation and subscription IDs).

## [0.2.11] - 2026-03-30

### Fixed

- **Post-signing unlock script verification (H2)** — New `verify_unlock_scripts`
  module validates every input's unlocking script via `Spend::validate()` after
  signing, matching TS `verifyUnlockScripts`. Catches signing bugs before
  broadcast.

- **`StorageProcessActionResult` missing fields (H3)** — Added
  `not_delayed_results: Option<Vec<ReviewActionResult>>` and
  `log: Option<String>` to match TS wire format.

- **`StorageInternalizeActionResult` missing field (H4)** — Added
  `not_delayed_results` field.

- **`WERR_REVIEW_ACTIONS` error variant (H5)** — New error variant (code=5) with
  `review_action_results`, `send_with_results`, `txid`, `tx`, `no_send_change`
  fields for undelayed broadcast result handling.

- **`returnTXIDOnly` handling (H6)** — `createAction` and `signAction` now
  conditionally omit `tx` from results when `returnTXIDOnly` is set, matching TS
  behavior.

- **`mergePriorOptions` in signAction (H7)** — signAction now inherits
  `is_no_send`, `is_delayed`, `is_send_with` flags from the prior createAction
  when not explicitly specified. Uses `Option<bool>` semantics so explicit
  `false` correctly overrides prior `true`.

- **`sendWith` array population (H8)** — `processAction` calls now pass
  `options.send_with` when `is_send_with` is true instead of always empty.

- **`maxAcceptableHeight` guard (H10)** — `TaskCheckForProofs` and
  `TaskCheckNoSends` now skip proofs from blocks above the last known header
  height, preventing acceptance of bleeding-edge proofs vulnerable to reorgs.
  Shared via `Arc<AtomicU32>` with the Monitor.

- **`TaskUnFail` steps 3-4 (H11)** — After unfailing a transaction, inputs are
  now matched to user outputs (updating `spentBy`) and output spendability is
  validated via `isUtxo`. Added `userId` filter for multi-tenant safety. Added
  `validate_output_script` to recover NULL locking scripts from raw transaction
  data.

- **`WERR_INVALID_MERKLE_ROOT` error variant (H12)** — New error variant
  (code=8) with `block_hash`, `block_height`, `merkle_root`, `txid` fields.

- **`WERR_INVALID_PUBLIC_KEY.key` field (H13)** — Wire deserialization now reads
  from `key` field first, falling back to `parameter` for backward
  compatibility.

- **`SignerSignActionResult` missing field (H14)** — Added
  `not_delayed_results` to sign action results.

### Changed

- Bumped `bsv-sdk` dependency to 0.2.3 (adds `ReviewActionResult` type).
- `ValidSignActionArgs.is_no_send`, `is_delayed`, `is_send_with` changed from
  `bool` to `Option<bool>` to support proper merge semantics.

## [0.2.1] - 2026-03-30

### Fixed

- **Wire format parity with TypeScript wallet-toolbox** — All action types
  (`StorageCreateActionArgs`, `StorageProcessActionArgs`,
  `StorageInternalizeActionArgs`, etc.) now serialize to camelCase JSON matching
  the TypeScript wire format exactly. Adds `#[serde(rename_all = "camelCase")]`
  to all 20 structs in `action_types.rs`.

- **`StorageCreateTransactionSdkInput.output_type`** serializes as `"type"` (not
  `"outputType"`) matching the TS field name.

- **`input_beef` fields** serialize as `"inputBEEF"` (uppercase BEEF) matching TS.

- **`createAction` wire format** — Expanded `StorageCreateActionArgs` with nested
  `outpoint: { txid, vout }` objects, `options` object (signAndProcess,
  acceptDelayedBroadcast, randomizeOutputs, etc.), and computed boolean flags
  (isNewTx, isSendWith, isDelayed, isRemixChange, includeAllSourceTransactions).

- **`internalizeAction` wire format** — Replaced flat `StorageInternalizeOutput`
  with SDK's `InternalizeOutput` tagged enum serializing as
  `{ protocol: "wallet payment", paymentRemittance: {...} }`. Added
  `seekPermission` field.

- **BRC-29 locking script validation** in `internalizeAction` — Parses
  AtomicBEEF, validates output indices, derives expected P2PKH script via BRC-29
  key derivation, and rejects outputs with mismatched locking scripts. Prevents
  accepting unspendable outputs (security fix).

- **`find_outputs` txStatus filtering** — Adds SQL subquery
  `(SELECT status FROM transactions WHERE transactionId = outputs.transactionId)
  IN (...)` matching TS StorageKnex behavior. Supports SQLite, MySQL, PostgreSQL.

- **`find_transactions` multi-status filtering** — Adds `status IN (?, ...)`
  clause for array-based status queries matching TS `whereIn` behavior.

- **`Option` field omission** — All `Option<T>` fields across 16 table structs
  now use `skip_serializing_if = "Option::is_none"`, omitting `None` values from
  JSON instead of serializing as `null`. Matches TS `undefined` omission behavior.

### Changed

- Bumped `bsv-sdk` dependency to 0.2.2 (includes `outputIndex` serde fix for
  `InternalizeOutput` enum).

### Added

- 14 BRC-100 cross-language parity tests verifying wire format against exact
  TypeScript test vectors (status enums, tagged enum serialization, boolean
  defaults, StorageProvidedBy enum).

## [0.2.0] - 2026-03-26

### Added

- **StorageClient\<W\> — full TS WalletStorageProvider parity** -- JSON-RPC 2.0
  client implementing all ~25 WalletStorageProvider methods with BRC-31 mutual
  authentication via AuthFetch, WERR-coded error mapping, and settings caching.
  Wire method names exactly match the TypeScript StorageClient (e.g.,
  `findOutputBaskets` not `findOutputBasketsAuth`).

- **WalletStorageProvider async trait** -- ~25 method trait matching the TS
  WalletStorageProvider interface hierarchy, with a blanket impl for existing
  StorageProvider types. `is_storage_provider()` returns true for local storage,
  false for remote clients.

- **WalletStorageManager rewrite** -- Multi-provider architecture with
  ManagedStorage wrappers, four-level hierarchical locking (reader < writer <
  sync < storage_provider), chunked sync loops (syncToWriter/syncFromReader),
  and automatic store partitioning into active/backup/conflicting categories.

- **Manager orchestration** -- `setActive()` with full 8-step conflict
  resolution matching TS behavior (detect conflicts, merge via syncToWriter,
  update user.activeStorage, propagate to all stores, re-partition).
  `updateBackups()` fan-out sync to all backup stores with per-backup error
  isolation. `reproveHeader()` and `reproveProven()` for proof re-validation
  against orphaned block headers.

- **WalletArc\<W\>** -- Arc wrapper enabling Clone for non-Clone wallet types,
  required by AuthFetch for BRC-31 auth signing.

- **Integration test suite** -- 8 live integration tests against
  staging-storage.babbage.systems proving BRC-31 auth, makeAvailable,
  findOrInsertUser, getSyncChunk/processSyncChunk wire format round-trip,
  full wallet with StorageClient backup, syncToWriter, updateBackups, non-empty
  SyncChunk handling, and funded-key authentication. Plus 6 local parity tests
  covering populated sync, incremental sync, setActive twice, two-wallet
  isolation, bidirectional sync, and setActive-with-backup-first.

- **serde_helpers module** -- Lenient bool deserialization (`bool_from_int_or_bool`)
  for TS server interop where boolean fields arrive as `0`/`1` instead of
  `false`/`true`.

### Fixed

- **serde_datetime** -- Now emits ISO 8601 timestamps with trailing "Z" and
  3-digit millisecond precision matching TS server expectations.

- **Vec\<u8\> serialization** -- Binary fields (raw_tx, merkle_path, input_beef)
  now serialize as JSON number arrays matching TS `Array.from(Buffer)` wire
  format, not base64.

- **SyncState.init deserialization** -- The `init` boolean field on SyncState
  now accepts integer values from the TS server.

- **ProcessSyncChunkResult.done deserialization** -- The `done` boolean field
  now accepts integer values from the TS server, preventing infinite sync loops
  when processing non-empty chunks.

### Changed

- Bumped version from 0.1.20 to 0.2.0 (minor version bump for new public API surface).

## [0.1.20] - 2026-03-20

### Fixed

- **`StorageProvidedBy` enum now supports `"you-and-storage"`** -- Added the
  missing variant to match the TypeScript SDK. Previously, syncing outputs with
  `providedBy: "you-and-storage"` from a TS storage instance would fail with a
  deserialization error, breaking cross-platform sync compatibility.

## [0.1.19] - 2026-03-19

### Added

- **Pool configuration API on `WalletBuilder`** -- New builder methods
  `with_max_connections(u32)`, `with_min_connections(u32)`,
  `with_pool_idle_timeout(Duration)`, and `with_pool_connect_timeout(Duration)`
  allow operators to tune DB pool sizing for multi-replica deployments.
  Defaults are unchanged (max=50, min=2, idle=600s, connect=5s).

## [0.1.15] - 2026-03-19

### Fixed

- **`listCertificates` applies partial filter** -- When `args.partial` is
  provided (with certType, serialNumber, certifier, subject), these fields are
  now used to filter the certificate query. Previously the partial filter was
  ignored, returning all certificates and causing `proveCertificate` to fail
  with "must be a unique certificate match" when multiple certificates exist.

### Changed

- Bumped `bsv-sdk` dependency to `0.1.75`.

## [0.1.14] - 2026-03-19

### Fixed

- **`getSyncChunk` now respects per-entity offsets** -- The sync protocol uses
  per-entity row offsets for pagination. Previously these were ignored (always
  offset 0), causing the server to return the same data on every call and
  creating an infinite sync loop. Added `SyncChunkOffsets` struct and threading
  through `GetSyncChunkArgs` to each entity query.

- **Increased default MySQL pool size** -- `max_connections` from 10 to 50,
  `min_connections` from 1 to 2, `connect_timeout` from 30s to 5s. Prevents
  connection pool exhaustion under concurrent request load and fails fast
  instead of cascading 30s timeouts.

### Changed

- Bumped `bsv-sdk` dependency to `0.1.74`.

## [0.1.13] - 2026-03-19

### Added

- **`merge_input_beef` for remote storage servers** -- New public function in
  `storage::methods::create_action` that merges BEEF proof data for all
  storage-allocated change inputs into the `createAction` result's `inputBeef`
  field. Remote clients' signers need complete BEEF (raw transactions + merkle
  proofs) to build and sign transactions. Matches the TS
  `mergeAllocatedChangeBeefs` behavior.

- **Ancestor proven tx storage in `internalize_action`** -- When internalizing
  a transaction from AtomicBEEF, all ancestor transactions that carry merkle
  proofs are now stored in the `proven_txs` table (not just the main
  transaction). This allows `get_valid_beef_for_txid` to reconstruct complete
  BEEF chains for subsequent `createAction` calls.

- **`raw_tx` field on `TransactionPartial`** -- Added `raw_tx: Option<Vec<u8>>`
  to `TransactionPartial` and the corresponding `Bytes` bind variant in both
  SQLite and MySQL update implementations.

- **Self-payment BRC-29 test** -- Added `test_self_payment_lock_unlock_correspondence`
  to verify lock/unlock key derivation works correctly when the same key is both
  locker and unlocker (the change output scenario).

### Fixed

- **`internalize_action` now stores `raw_tx` and `input_beef`** -- The
  Transaction record created during internalization now includes the serialized
  raw transaction bytes and the full BEEF data. Previously both were `None`,
  which prevented BEEF reconstruction for change inputs in later
  `createAction` calls.

- **`processAction` stores `raw_tx` on Transaction record** -- The signed
  raw transaction bytes are now persisted on the Transaction record during
  `processAction`, enabling `get_valid_beef_for_txid` to build BEEF for
  subsequent transactions that spend this transaction's outputs.

- **`createAction` populates `source_transaction` on change inputs** -- Storage-
  allocated change input records now include the source transaction's raw bytes
  (looked up from the `transactions` table). The client signer's
  `makeSignableTransactionBeef` requires `sourceTransaction` on every input for
  the `isSignAction` flow (used by UMP token updates).

- **`createAction` extracts locking script from `raw_tx` for change inputs** --
  When the output record's `locking_script` is NULL (before `processAction`
  populates it), the locking script is now extracted by parsing the source
  transaction's `raw_tx` at the correct vout index. This fixes P2PKH signature
  verification failures in the client signer.

## [0.1.11] - 2026-03-11

### Added

- **BEEF assembly in `list_outputs`** -- When `OutputInclude::EntireTransactions` is requested,
  the wallet layer now assembles a valid BEEF by calling `get_valid_beef_for_txid()` for each
  unique txid in the output set and merging them into a single BEEF structure with proper bump
  index offset adjustment and txid deduplication.

- **Settings persistence** -- `WalletSettingsManager` now reads and writes wallet settings to
  the `settings` table via a new `walletSettingsJson` column. Settings survive process restarts.
  Invalid stored JSON falls back to defaults with a warning log. Added database migrations for
  SQLite, MySQL, and PostgreSQL.

- **signAction reference recovery** -- `DefaultWalletSigner::sign_action()` can now recover
  `PendingSignAction` from storage when the reference is not in the in-memory map (e.g. after
  a process restart). Reconstructs the pending state from the Transaction and Output tables,
  including per-input derivation data for BRC-29 signing.

- **TaskMineBlock service integration** -- The test-only `TaskMineBlock` now holds an
  `Arc<dyn WalletServices>` reference (matching other monitor tasks) and calls
  `services.get_height()` to report chain height. Updated constructor and tests with mock
  services.

- `CHANGELOG.md` file.

### Changed

- Bumped version from 0.1.1 to 0.1.11.
- `WalletSettingsManager._storage` field renamed to `storage` (no longer unused).
- `Settings` table struct now includes `wallet_settings_json: Option<String>`.
- `TaskMineBlock::new()` now requires `Arc<dyn WalletServices>` parameter.

## [0.1.1] - 2026-03-04

Initial public release with full BRC-100 WalletInterface implementation, SQLite/MySQL/PostgreSQL
storage backends, ARC/WhatsOnChain/Bitails/Chaintracks service providers, background monitor with
15 tasks, WAB authentication, and token-based permissions.
