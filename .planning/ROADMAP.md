# Roadmap: rust-wallet-toolbox StorageClient

## Overview

This milestone delivers `StorageClient<W>` — a JSON-RPC client that speaks the TypeScript StorageServer wire protocol and integrates as a backup provider in `WalletStorageManager`. The work proceeds in dependency order: fix the wire format first so every subsequent method is correct from the start, then define the trait, implement the client, integrate it into the manager, and finally validate cross-language compatibility against the live TypeScript server at storage.babbage.systems.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Wire Format** - Fix serde_datetime and Vec<u8> serialization to match TS server expectations (completed 2026-03-24)
- [ ] **Phase 2: Trait Definition** - Define WalletStorageProvider trait and blanket impl for local providers
- [ ] **Phase 3: StorageClient** - Implement StorageClient struct with rpc_call and all ~25 method stubs
- [ ] **Phase 4: Manager Integration** - Wire StorageClient into WalletStorageManager as backup provider
- [ ] **Phase 5: Integration Testing** - Prove cross-language wire compatibility against live TS server
- [ ] **Phase 6: PR Submission** - Fork repo, clean branch, create professional pull request to b1narydt/rust-wallet-toolbox

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
**Plans**: TBD

### Phase 3: StorageClient
**Goal**: StorageClient<W> compiles, passes JSON-RPC envelope tests, and implements every WalletStorageProvider method with correct wire method names
**Depends on**: Phase 2
**Requirements**: CLIENT-01, CLIENT-02, CLIENT-03, CLIENT-04, CLIENT-05, CLIENT-06
**Success Criteria** (what must be TRUE):
  1. `StorageClient::new(wallet, endpoint)` constructs without error and holds `AuthFetch<W>` behind `tokio::sync::Mutex`
  2. `rpc_call` produces a JSON-RPC 2.0 envelope with positional `params` array, auto-incrementing `id`, and correct `method` string
  3. A JSON-RPC error response with a known WERR code maps to the correct `WalletError` variant
  4. All ~25 `WalletStorageProvider` method stubs compile and forward to `rpc_call` with camelCase wire method names that match the TS StorageClient exactly (e.g., `findOutputBaskets` not `findOutputBasketsAuth`)
  5. `is_storage_provider()` returns `false` on `StorageClient`
**Plans**: TBD

### Phase 4: Manager Integration
**Goal**: WalletStorageManager accepts StorageClient as a backup provider and skips CRUD propagation for remote backups
**Depends on**: Phase 3
**Requirements**: MGR-01, MGR-02, MGR-03
**Success Criteria** (what must be TRUE):
  1. `WalletStorageManager` backup field accepts `Arc<dyn WalletStorageProvider>` and the codebase compiles with no type errors
  2. CRUD write propagation paths in `manager.rs` are guarded by `is_storage_provider()` — a `StorageClient` backup does not receive direct CRUD writes
  3. A `StorageClient` instance can be passed to `WalletStorageManager` via its standard constructor without any type casting
**Plans**: TBD

### Phase 5: Integration Testing
**Goal**: Cross-language wire compatibility with the live TypeScript storage server at storage.babbage.systems is proven by passing tests
**Depends on**: Phase 4
**Requirements**: TEST-01, TEST-02, TEST-03, TEST-04, TEST-05
**Success Criteria** (what must be TRUE):
  1. BRC-31 mutual auth handshake completes successfully against storage.babbage.systems (no auth errors in test output)
  2. `makeAvailable()` returns a valid `Settings` struct from the live TS server
  3. `findOrInsertUser()` creates or retrieves a user on the live TS server without error
  4. A getSyncChunk + processSyncChunk round-trip completes against the live TS server with correct entity deserialization
  5. A full payment internalize flow completes end-to-end with `StorageClient` as the backup provider
**Plans**: TBD

### Phase 6: PR Submission
**Goal**: Fork the repo, prepare a clean branch with only implementation changes (no planning docs), and create a professional pull request to b1narydt/rust-wallet-toolbox
**Depends on**: Phase 5
**Requirements**: PR-01, PR-02, PR-03, PR-04
**Success Criteria** (what must be TRUE):
  1. Fork exists under user's GitHub account with `feat/storage-client` branch pushed
  2. Branch contains only implementation files (no `.planning/` directory) with clean commit history
  3. PR is created against b1narydt/rust-wallet-toolbox main branch with professional description covering what was added, how it was tested, and TS parity evidence
  4. All existing repo tests still pass alongside new StorageClient tests
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Wire Format | 1/1 | Complete   | 2026-03-24 |
| 2. Trait Definition | 0/? | Not started | - |
| 3. StorageClient | 0/? | Not started | - |
| 4. Manager Integration | 0/? | Not started | - |
| 5. Integration Testing | 0/? | Not started | - |
| 6. PR Submission | 0/? | Not started | - |
