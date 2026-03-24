# Requirements: rust-wallet-toolbox StorageClient

**Defined:** 2026-03-24
**Core Value:** Wire-compatible remote storage that lets a Rust wallet sync with TypeScript storage servers

## v1 Requirements

Requirements for StorageClient milestone. Direct translation of TS StorageClient with full parity.

### Wire Format

- [ ] **WIRE-01**: serde_datetime emits ISO 8601 timestamps with trailing "Z" and 3-digit millisecond precision matching TS server expectations
- [ ] **WIRE-02**: Vec<u8> fields serialize as JSON number arrays (not base64), matching TS `Array.from(Buffer)` wire format
- [ ] **WIRE-03**: SyncChunk and SyncMap types serialize to JSON matching TS wire format (camelCase fields, optional arrays as null/absent)
- [ ] **WIRE-04**: Round-trip serde tests pass with TS-generated fixture JSON for all entity types with timestamps and binary data

### Trait Definition

- [ ] **TRAIT-01**: WalletStorageProvider async trait defined with ~25 methods matching TS WalletStorageProvider interface hierarchy
- [ ] **TRAIT-02**: Blanket impl allows existing StorageProvider types to satisfy WalletStorageProvider
- [ ] **TRAIT-03**: `is_storage_provider()` method returns false for remote clients, true for local storage via blanket impl

### StorageClient Implementation

- [ ] **CLIENT-01**: StorageClient struct with rpc_call helper sends JSON-RPC 2.0 requests (positional params array, auto-incrementing ID)
- [ ] **CLIENT-02**: AuthFetch<W> held behind tokio::sync::Mutex for Send+Sync safety in async context
- [ ] **CLIENT-03**: All ~25 WalletStorageProvider methods implemented as pass-throughs to rpc_call, matching TS method signatures exactly
- [ ] **CLIENT-04**: JSON-RPC error responses map back to typed WalletError variants via WalletErrorObject deserialization
- [ ] **CLIENT-05**: Constructor accepts WalletInterface impl for BRC-31 auth signing (same chicken-and-egg pattern as TS)
- [ ] **CLIENT-06**: Wire method names exactly match TS StorageClient (e.g., `findOutputBaskets` not `findOutputBasketsAuth`)

### Manager Integration

- [ ] **MGR-01**: WalletStorageManager backup field accepts WalletStorageProvider (not just StorageProvider)
- [ ] **MGR-02**: CRUD write propagation skipped for remote backup providers (uses sync-chunk replication instead)
- [ ] **MGR-03**: StorageClient plugs into WalletStorageManager as backup via standard constructor

### Integration Testing

- [ ] **TEST-01**: BRC-31 auth handshake succeeds against live storage.babbage.systems TS server
- [ ] **TEST-02**: makeAvailable() retrieves Settings from live TS server
- [ ] **TEST-03**: findOrInsertUser() creates/retrieves user on live TS server
- [ ] **TEST-04**: Sync chunk round-trip (getSyncChunk + processSyncChunk) works against live TS server
- [ ] **TEST-05**: Full payment internalize flow works end-to-end with StorageClient as backup provider

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
| WIRE-01 | Phase 1 | Pending |
| WIRE-02 | Phase 1 | Pending |
| WIRE-03 | Phase 1 | Pending |
| WIRE-04 | Phase 1 | Pending |
| TRAIT-01 | Phase 2 | Pending |
| TRAIT-02 | Phase 2 | Pending |
| TRAIT-03 | Phase 2 | Pending |
| CLIENT-01 | Phase 3 | Pending |
| CLIENT-02 | Phase 3 | Pending |
| CLIENT-03 | Phase 3 | Pending |
| CLIENT-04 | Phase 3 | Pending |
| CLIENT-05 | Phase 3 | Pending |
| CLIENT-06 | Phase 3 | Pending |
| MGR-01 | Phase 4 | Pending |
| MGR-02 | Phase 4 | Pending |
| MGR-03 | Phase 4 | Pending |
| TEST-01 | Phase 5 | Pending |
| TEST-02 | Phase 5 | Pending |
| TEST-03 | Phase 5 | Pending |
| TEST-04 | Phase 5 | Pending |
| TEST-05 | Phase 5 | Pending |
| PR-01 | Phase 6 | Pending |
| PR-02 | Phase 6 | Pending |
| PR-03 | Phase 6 | Pending |
| PR-04 | Phase 6 | Pending |

**Coverage:**
- v1 requirements: 25 total
- Mapped to phases: 25
- Unmapped: 0

---
*Requirements defined: 2026-03-24*
*Last updated: 2026-03-24 after roadmap creation — all requirements mapped*
