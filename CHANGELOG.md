# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
