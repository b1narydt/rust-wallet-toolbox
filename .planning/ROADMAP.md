# Roadmap: rust-wallet-toolbox StorageClient

## Overview

This milestone delivers full TypeScript WalletStorageProvider parity in Rust — a JSON-RPC `StorageClient` that speaks the TS wire protocol, a fully rewritten `WalletStorageManager` with multi-provider support, chunked sync loops, hierarchical locking, and active-switching orchestration. The work proceeds in dependency order: wire format → trait → client → manager rewrite → manager orchestration → integration testing → PR.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Wire Format** - Fix serde_datetime and Vec<u8> serialization to match TS server expectations (completed 2026-03-24)
- [x] **Phase 2: Trait Definition** - Define WalletStorageProvider trait and blanket impl for local providers (completed 2026-03-24)
- [x] **Phase 3: StorageClient** - Implement StorageClient struct with rpc_call, all ~25 WalletStorageProvider methods, and updateProvenTxReqWithNewProvenTx (completed 2026-03-24)
- [x] **Phase 4: Manager Rewrite** - Multi-provider WalletStorageManager with ManagedStorage, sync loops, and hierarchical locking (completed 2026-03-25)
- [x] **Phase 5: Manager Orchestration** - setActive conflict resolution, updateBackups fan-out, reprove proof re-validation (completed 2026-03-25)
- [x] **Phase 6: Integration Testing** - Prove cross-language wire compatibility against live TS server (completed 2026-03-25)
- [ ] **Phase 7: PR Submission** - Fork repo, clean branch, create professional pull request to b1narydt/rust-wallet-toolbox

## Phase Details

### Phase 1: Wire Format
**Goal**: Serialization matches the TypeScript server wire format before any client code is written
**Depends on**: Nothing (first phase)
**Requirements**: WIRE-01, WIRE-02, WIRE-03, WIRE-04
**Success Criteria** (what must be TRUE):
  1. `serde_datetime::serialize` emits timestamps with 3-digit millisecond precision and a trailing "Z" (e.g., `"2024-01-15T10:30:00.718Z"`)
  2. `Vec<u8>` fields (raw_tx, merkle_path, input_beef) serialize as JSON integer arrays, not base64 strings
  3. `SyncChunk` and `SyncMap` serialize with camelCase fields and optional arrays as null or absent (not empty arrays)
  4. A round-trip serde test passes using TS-generated fixture JSON for entity types containing timestamps and binary data
**Plans:** 1/1 plans complete
Plans:
- [ ] 01-01-PLAN.md — Fix serde_datetime (Z + 3ms), SyncChunk skip_serializing_if, and TS-fixture round-trip tests

### Phase 2: Trait Definition
**Goal**: WalletStorageProvider trait exists in the type system with a blanket impl covering all local storage providers
**Depends on**: Phase 1
**Requirements**: TRAIT-01, TRAIT-02, TRAIT-03
**Success Criteria** (what must be TRUE):
  1. `WalletStorageProvider` trait compiles with ~25 async methods matching the TS WalletStorageProvider interface hierarchy
  2. Existing `SqlxProvider` satisfies `WalletStorageProvider` via the blanket impl without any modification to its source file
  3. `is_storage_provider()` returns `true` from the blanket impl and can be overridden to return `false` on remote clients
**Plans:** 1/1 plans complete
Plans:
- [x] 02-01-PLAN.md — Define WalletStorageProvider trait, blanket impl over StorageProvider, supporting types (AuthId extension, RequestSyncChunkArgs)

### Phase 3: StorageClient
**Goal**: StorageClient<W> compiles, passes JSON-RPC envelope tests, and implements every WalletStorageProvider method plus updateProvenTxReqWithNewProvenTx with correct wire method names
**Depends on**: Phase 2
**Requirements**: CLIENT-01, CLIENT-02, CLIENT-03, CLIENT-04, CLIENT-05, CLIENT-06, CLIENT-07
**Success Criteria** (what must be TRUE):
  1. `StorageClient::new(wallet, endpoint)` constructs without error and holds `AuthFetch<W>` behind `tokio::sync::Mutex`
  2. `rpc_call` produces a JSON-RPC 2.0 envelope with positional `params` array, auto-incrementing `id`, and correct `method` string
  3. A JSON-RPC error response with a known WERR code maps to the correct `WalletError` variant
  4. All ~25 `WalletStorageProvider` method stubs compile and forward to `rpc_call` with camelCase wire method names that match the TS StorageClient exactly (e.g., `findOutputBaskets` not `findOutputBasketsAuth`)
  5. `is_storage_provider()` returns `false` on `StorageClient`
  6. `updateProvenTxReqWithNewProvenTx` is implemented as an extra method on StorageClient (beyond WalletStorageProvider trait), matching TS behavior
  7. Settings are cached after first `makeAvailable()` call; `getSettings()` returns cached value or errors if not yet available
**Plans:** 2/2 plans complete
Plans:
- [ ] 03-01-PLAN.md — StorageClient struct, rpc_call, error mapping, settings caching, module wiring
- [ ] 03-02-PLAN.md — All 23 WalletStorageProvider method implementations as rpc_call delegates

### Phase 4: Manager Rewrite
**Goal**: WalletStorageManager is rewritten to match TS architecture — multi-provider, ManagedStorage wrappers, chunked sync loops, and hierarchical locking
**Depends on**: Phase 3
**Requirements**: MGR-01, MGR-02, MGR-03, MGR-04, MGR-05, MGR-06, MGR-07, MGR-08
**Success Criteria** (what must be TRUE):
  1. `WalletStorageManager::new(identity_key, active, backups)` accepts N backup providers as `Vec<Arc<dyn WalletStorageProvider>>`
  2. Each provider is wrapped in `ManagedStorage` with cached `settings`, `user`, `is_available`, `is_storage_provider` fields
  3. `makeAvailable()` partitions stores into active/backups/conflicting_actives matching TS logic (enabled-active = storageIdentityKey matches user.activeStorage AND no conflicts)
  4. Four-level hierarchical lock system: reader < writer < sync < storage_provider, with ordered acquisition matching TS `runAsReader/Writer/Sync/StorageProvider`
  5. `syncToWriter()` and `syncFromReader()` implement chunked sync loops using `EntitySyncState` + `RequestSyncChunkArgs` → getSyncChunk/processSyncChunk iteration until `done`
  6. CRUD write propagation is replaced with chunk-based sync replication — writes go to active only, backups sync via `updateBackups()`
  7. All manager-level delegation methods exist: `createAction`, `processAction`, `internalizeAction`, `insertCertificate`, `findCertificates`, `findOutputBaskets`, `findOutputs`, `findProvenTxReqs` with appropriate lock acquisition and auth checks
  8. `addWalletStorageProvider()` adds at runtime, resets `is_available`, re-runs `makeAvailable()` partitioning
**Plans:** 4/4 plans complete
Plans:
- [ ] 04-01-PLAN.md — ManagedStorage struct, WalletStorageManager rewrite with hierarchical locks, makeAvailable partition logic, TS-parity getter methods, auto-init lock helpers
- [ ] 04-02-PLAN.md — Sync loops (syncToWriter/syncFromReader) with progLog support, delegation methods with lock semantics and WERR_* error codes, addWalletStorageProvider
- [ ] 04-03-PLAN.md — reproveHeader/reproveProven under StorageProvider lock, WalletStorageInfo accessor, remaining TS-parity methods
- [ ] 04-04-PLAN.md — Gap closure: WERR_UNAUTHORIZED guard in sync_from_reader (identity key mismatch check)

### Phase 5: Manager Orchestration
**Goal**: Complete WalletStorageManager orchestration — setActive conflict resolution is the remaining work; updateBackups, reprove, and getStores were implemented ahead of schedule in Phase 4
**Depends on**: Phase 4
**Requirements**: ORCH-01, ORCH-02, ORCH-03, ORCH-04, ORCH-05
**Success Criteria** (what must be TRUE):
  1. `setActive(auth, storageIdentityKey)` performs full conflict resolution: detect conflicting actives, merge state via syncToWriter, update user.activeStorage across all stores, re-partition
  2. `updateBackups()` iterates all backup stores calling syncToWriter, with error handling per-backup (one failure doesn't block others) — **already implemented in Phase 4 (manager.rs:1522)**
  3. `reproveHeader()` re-validates proofs against orphaned block headers using ChainTracker + getMerklePath, updates affected ProvenTx records — **already implemented in Phase 4 (manager.rs:1297)**
  4. `reproveProven()` re-validates individual ProvenTx proofs against current chain state — **already implemented in Phase 4 (manager.rs:1213)**
  5. `getStores()` returns `WalletStorageInfo` for all managed providers including status (isActive, isEnabled, isBackup, isConflicting, endpointUrl) — **already implemented in Phase 4 (manager.rs:1116)**
**Plans:** 1/1 plans complete
Plans:
- [x] 05-01-PLAN.md — setActive conflict resolution with 8-step TS-parity orchestration, verification of ORCH-02..05

### Phase 6: Integration Testing
**Goal**: Cross-language wire compatibility with the live TypeScript storage server at storage.babbage.systems is proven by passing tests
**Depends on**: Phase 5
**Requirements**: TEST-01, TEST-02, TEST-03, TEST-04, TEST-05, TEST-06, TEST-07
**Success Criteria** (what must be TRUE):
  1. BRC-31 mutual auth handshake completes successfully against storage.babbage.systems (no auth errors in test output)
  2. `makeAvailable()` returns a valid `Settings` struct from the live TS server
  3. `findOrInsertUser()` creates or retrieves a user on the live TS server without error
  4. A getSyncChunk + processSyncChunk round-trip completes against the live TS server with correct entity deserialization
  5. A full payment internalize flow completes end-to-end with `StorageClient` as the backup provider
  6. `syncToWriter()` loop completes a full sync from local storage to remote StorageClient
  7. `updateBackups()` successfully syncs to a StorageClient backup
**Plans:** 1/1 plans complete
Plans:
- [x] 06-01-PLAN.md — Direct StorageClient tests (auth, makeAvailable, findOrInsertUser, sync chunk), full wallet with StorageClient backup, syncToWriter + updateBackups to remote

### Phase 7: PR Submission
**Goal**: Fork the repo, prepare a clean branch with only implementation changes (no planning docs), and create a professional pull request to b1narydt/rust-wallet-toolbox
**Depends on**: Phase 6
**Requirements**: PR-01, PR-02, PR-03, PR-04
**Success Criteria** (what must be TRUE):
  1. Fork exists under user's GitHub account with `feat/storage-client` branch pushed
  2. Branch contains only implementation files (no `.planning/` directory) with clean commit history
  3. PR is created against b1narydt/rust-wallet-toolbox main branch with professional description covering what was added, how it was tested, and TS parity evidence
  4. All existing repo tests still pass alongside new StorageClient tests
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Wire Format | 1/1 | Complete | 2026-03-24 |
| 2. Trait Definition | 1/1 | Complete | 2026-03-24 |
| 3. StorageClient | 2/2 | Complete   | 2026-03-24 |
| 4. Manager Rewrite | 4/4 | Complete   | 2026-03-25 |
| 5. Manager Orchestration | 1/1 | Complete   | 2026-03-25 |
| 6. Integration Testing | 1/1 | Complete | 2026-03-25 |
| 7. PR Submission | 0/? | Not started | - |
