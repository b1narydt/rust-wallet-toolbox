# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
