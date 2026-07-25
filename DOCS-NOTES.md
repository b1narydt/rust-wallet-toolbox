# DOCS-NOTES — 2026-07-24 documentation true-up (0.3.4)

Uncertainties and out-of-scope findings from the README/CHANGELOG true-up. Not guesses —
each item was verified as far as the worktree allows and stopped there.

1. **CHANGELOG gap: 0.3.0 and 0.3.1 have no entries.** Both are published on crates.io.
   The version history is nonlinear: `37e60ef` bumped 0.2.22-era code to 0.3.0, then
   `07fa4a3` bumped back DOWN to 0.2.23, then 0.2.24, then `6aceb31` merged to 0.3.1
   ("restores SigningProvider::prepare_spend_contexts", includes `96a5fa6` bsv-sdk 0.3
   UMP-token fix). Reconstructing accurate 0.3.0/0.3.1 entries needs a human call on
   what each published artifact actually contained; left out rather than guessed.

2. **Issue #31 placed under [0.3.2], not [0.3.3].** The docs brief groups the delegated
   BRC-29 wallet-payment script derivation into the 0.3.3 bundle, but commit `02a4b97`
   (the feature) is the commit that bumped Cargo.toml to 0.3.2, and 0.3.2 is published
   on crates.io. Code wins; the entry is under [0.3.2] - 2026-07-23.

3. **README "15 bg tasks" corrected to 16.** Counted from `MonitorBuilder::build()`
   (`src/monitor/mod.rs`, default_tasks preset): Clock, MonitorCallHistory, NewHeader,
   SendWaiting, CheckForProofs, CheckNoSends, FailAbandoned, UnFail, ReviewStatus,
   ReviewDoubleSpends, ReviewProvenTxs, ReviewUtxos, Reorg, ArcSse, SyncWhenIdle,
   Purge = 16. (17 task modules exist; TaskMineBlock is not wired by any preset.)

4. **Stale rustdoc (source, not edited — out of docs scope):** the `default_tasks()` /
   `multi_user_tasks()` doc comments in `src/monitor/mod.rs` (lines ~608–621) list
   "TaskMineBlock (mock chain only)" and omit ReviewDoubleSpends/ReviewProvenTxs/
   ReviewUtxos/SyncWhenIdle/Purge — neither matches what `build()` actually wires.
   Same for the `WalletBuilder` rustdoc example in `src/wallet/setup.rs` (~line 94),
   which still shows `.with_monitor()` (harmless: it is a documented no-op).

5. **Monitor task intervals** are not documented in the README (only "configurable
   intervals"), so nothing to verify there. Pool defaults in the README (max=50,
   min=2, idle=600s, connect=5s) verified against `StorageConfig::default()` in
   `src/storage/mod.rs:44-54`. The `start_tasks()` double-start error text verified as
   "monitor tasks are already running" (`src/monitor/mod.rs:244-249`).
