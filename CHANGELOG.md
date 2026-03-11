# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
