# Requirements: rust-wallet-toolbox StorageClient

**Defined:** 2026-03-24
**Core Value:** Wire-compatible remote storage that lets a Rust wallet sync with TypeScript storage servers

## v1 Requirements

Requirements for StorageClient milestone. Direct translation of TS StorageClient with full parity.

### Wire Format

- [x] **WIRE-01**: serde_datetime emits ISO 8601 timestamps with trailing "Z" and 3-digit millisecond precision matching TS server expectations
- [x] **WIRE-02**: Vec<u8> fields serialize as JSON number arrays (not base64), matching TS `Array.from(Buffer)` wire format
- [x] **WIRE-03**: SyncChunk and SyncMap types serialize to JSON matching TS wire format (camelCase fields, optional arrays as null/absent)
- [x] **WIRE-04**: Round-trip serde tests pass with TS-generated fixture JSON for all entity types with timestamps and binary data

### Trait Definition

- [x] **TRAIT-01**: WalletStorageProvider async trait defined with ~25 methods matching TS WalletStorageProvider interface hierarchy
- [x] **TRAIT-02**: Blanket impl allows existing StorageProvider types to satisfy WalletStorageProvider
- [x] **TRAIT-03**: `is_storage_provider()` method returns false for remote clients, true for local storage via blanket impl

### StorageClient Implementation

- [x] **CLIENT-01**: StorageClient struct with rpc_call helper sends JSON-RPC 2.0 requests (positional params array, auto-incrementing ID)
- [x] **CLIENT-02**: AuthFetch<W> held behind tokio::sync::Mutex for Send+Sync safety in async context
- [x] **CLIENT-03**: All ~25 WalletStorageProvider methods implemented as pass-throughs to rpc_call, matching TS method signatures exactly
- [x] **CLIENT-04**: JSON-RPC error responses map back to typed WalletError variants via WalletErrorObject deserialization
- [x] **CLIENT-05**: Constructor accepts WalletInterface impl for BRC-31 auth signing (same chicken-and-egg pattern as TS)
- [x] **CLIENT-06**: Wire method names exactly match TS StorageClient (e.g., `findOutputBaskets` not `findOutputBasketsAuth`)
- [x] **CLIENT-07**: updateProvenTxReqWithNewProvenTx implemented as extra method on StorageClient (beyond WalletStorageProvider trait), settings cached after makeAvailable

### Manager Rewrite

- [x] **MGR-01**: WalletStorageManager constructor accepts identity_key + optional active + Vec of backup WalletStorageProviders (multi-provider)
- [x] **MGR-02**: ManagedStorage wrapper per provider caching settings, user, is_available, is_storage_provider
- [x] **MGR-03**: makeAvailable() partitions stores into active/backups/conflicting_actives matching TS enabled-active logic
- [x] **MGR-04**: Four-level hierarchical lock system (reader < writer < sync < storage_provider) with ordered acquisition
- [x] **MGR-05**: syncToWriter() and syncFromReader() chunked sync loops using EntitySyncState + RequestSyncChunkArgs iteration
- [x] **MGR-06**: CRUD write propagation replaced with chunk-based sync — writes to active only, backups sync via updateBackups()
- [x] **MGR-07**: All manager-level delegation methods with appropriate lock acquisition and auth checks
- [x] **MGR-08**: addWalletStorageProvider() runtime addition with re-partition

### Manager Orchestration

- [x] **ORCH-01**: setActive() full conflict resolution — detect conflicts, merge via syncToWriter, update user.activeStorage, re-partition
- [x] **ORCH-02**: updateBackups() fan-out sync to all backup stores with per-backup error handling
- [x] **ORCH-03**: reproveHeader() re-validates proofs against orphaned headers using ChainTracker
- [x] **ORCH-04**: reproveProven() re-validates individual ProvenTx proofs against current chain
- [x] **ORCH-05**: getStores() returns WalletStorageInfo for all providers with full status metadata

### Integration Testing

- [x] **TEST-01**: BRC-31 auth handshake succeeds against live storage.babbage.systems TS server
- [x] **TEST-02**: makeAvailable() retrieves Settings from live TS server
- [x] **TEST-03**: findOrInsertUser() creates/retrieves user on live TS server
- [x] **TEST-04**: Sync chunk round-trip (getSyncChunk + processSyncChunk) works against live TS server
- [x] **TEST-05**: Full payment internalize flow works end-to-end with StorageClient as backup provider
- [x] **TEST-06**: syncToWriter() completes a full sync from local to remote StorageClient
- [x] **TEST-07**: updateBackups() syncs to StorageClient backup successfully

### Integration Test Parity

- [x] **PARITY-01**: Populated sync: syncToWriter with seeded outputs produces inserts > 0 locally
- [x] **PARITY-02**: Incremental sync: second syncToWriter after new data only syncs the delta (inserts == new count, not total)
- [x] **PARITY-03**: setActive twice: swapping active store twice leaves user timestamps advanced after swap1 and backup_stores reflects swapped-out store after each swap
- [x] **PARITY-04**: Two-wallet isolation: two managers with different identity keys don't cross-contaminate data
- [x] **PARITY-05**: Bidirectional sync: sync A→B then B→A produces matching entity counts
- [x] **PARITY-06**: Non-empty remote sync: getSyncChunk from staging server returns non-empty SyncChunk with User entity
- [x] **PARITY-07**: Funded-key live: wallet with different BSV key authenticates via BRC-31 to staging server
- [x] **PARITY-08**: setActive with backup first: adding backup before calling setActive works correctly

### PR Submission

- [ ] **PR-01**: Fork created under user's GitHub account with feat/storage-client branch
- [ ] **PR-02**: Branch contains only implementation files, no .planning/ docs or AI artifacts
- [ ] **PR-03**: PR created with professional description, test evidence, and TS parity documentation
- [ ] **PR-04**: All existing repo tests pass alongside new StorageClient code

## Future Requirements

### Rust Storage Server Interop

- **RSRV-01**: StorageClient works against rust-wallet-storage server (secondary target)
- **RSRV-02**: Stress testing with concurrent sync operations

### Advanced Features

- **ADV-01**: Retry logic on transient auth failures with session reset
- **ADV-02**: Configurable request timeouts
- **ADV-03**: Connection health monitoring and reconnection

## Out of Scope

| Feature | Reason |
|---------|--------|
| StorageServer (Rust HTTP server) | Exists separately at rust-wallet-storage repo |
| MySQL/PostgreSQL testing | SQLite sufficient for this milestone |
| Browser/WASM support | Rust targets server/CLI environments |
| New wallet features beyond TS parity | Direct translation only |
| StorageProvider trait impl on StorageClient | TS pattern: WalletStorageProvider is the correct lighter interface |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| WIRE-01 | Phase 1 | Complete |
| WIRE-02 | Phase 1 | Complete |
| WIRE-03 | Phase 1 | Complete |
| WIRE-04 | Phase 1 | Complete |
| TRAIT-01 | Phase 2 | Complete |
| TRAIT-02 | Phase 2 | Complete |
| TRAIT-03 | Phase 2 | Complete |
| CLIENT-01 | Phase 3 | Complete |
| CLIENT-02 | Phase 3 | Complete |
| CLIENT-03 | Phase 3 | Complete |
| CLIENT-04 | Phase 3 | Complete |
| CLIENT-05 | Phase 3 | Complete |
| CLIENT-06 | Phase 3 | Complete |
| CLIENT-07 | Phase 3 | Complete |
| MGR-01 | Phase 4 | Complete — 04-01 |
| MGR-02 | Phase 4 | Complete — 04-01 |
| MGR-03 | Phase 4 | Complete — 04-01 |
| MGR-04 | Phase 4 | Complete — 04-01 |
| MGR-05 | Phase 4 | Complete |
| MGR-06 | Phase 4 | Complete |
| MGR-07 | Phase 4 | Complete |
| MGR-08 | Phase 4 | Complete |
| ORCH-01 | Phase 5 | Complete |
| ORCH-02 | Phase 5 | Complete |
| ORCH-03 | Phase 5 | Complete |
| ORCH-04 | Phase 5 | Complete |
| ORCH-05 | Phase 5 | Complete |
| TEST-01 | Phase 6 | Complete |
| TEST-02 | Phase 6 | Complete |
| TEST-03 | Phase 6 | Complete |
| TEST-04 | Phase 6 | Complete |
| TEST-05 | Phase 6 | Complete |
| TEST-06 | Phase 6 | Complete |
| TEST-07 | Phase 6 | Complete |
| PARITY-01 | Phase 6.1 | Complete |
| PARITY-02 | Phase 6.1 | Complete |
| PARITY-03 | Phase 6.1 | Complete |
| PARITY-04 | Phase 6.1 | Complete |
| PARITY-05 | Phase 6.1 | Complete |
| PARITY-06 | Phase 6.1 | Complete |
| PARITY-07 | Phase 6.1 | Complete |
| PARITY-08 | Phase 6.1 | Complete |
| PR-01 | Phase 7 | Pending |
| PR-02 | Phase 7 | Pending |
| PR-03 | Phase 7 | Pending |
| PR-04 | Phase 7 | Pending |

**Coverage:**
- v1 requirements: 46 total
- Mapped to phases: 46
- Unmapped: 0

---
*Requirements defined: 2026-03-24*
*Last updated: 2026-03-26 — added PARITY-01..08 for Phase 6.1*
