# Phase 6: Integration Testing - Research

**Researched:** 2026-03-25
**Domain:** Rust async integration testing against live BRC-31 storage servers
**Confidence:** HIGH

## Summary

Phase 6 proves wire compatibility between the Rust StorageClient and the live TypeScript storage server at `https://storage.babbage.systems` (main chain) and `https://staging-storage.babbage.systems` (test chain). All implementation phases (1-5) are complete. This phase is purely test authoring — no production code changes are needed unless bugs are discovered during test execution.

The key technical challenge is constructing a test wallet that acts as a `WalletInterface` for BRC-31 auth. The bsv-sdk crate exposes `ProtoWallet` (re-exported from `bsv_wallet_toolbox::ProtoWallet`) which implements `WalletInterface` using only a `PrivateKey` — sufficient for all cryptographic operations needed by `AuthFetch`. The existing `WalletBuilder` can also build a full wallet with a live `StorageClient` as a backup provider, which covers TEST-05 through TEST-07.

The established project pattern for live/network tests is: `#[ignore]` + `std::env::var` for credentials, gated under the sqlite feature. This keeps CI clean while allowing manual execution. The TS reference tests (in `.ref/wallet-toolbox/test/utils/TestUtilsWalletStorage.ts`) confirm the exact setup sequence for tests against `storage.babbage.systems`.

**Primary recommendation:** Write all integration tests in a new `tests/storage_client_integration_tests.rs` file, gated with `#[cfg(feature = "sqlite")]` + `#[ignore]`, reading a root key hex from `BSV_TEST_ROOT_KEY` environment variable. Tests must be run with `-- --ignored` to execute against the live server.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| TEST-01 | BRC-31 auth handshake succeeds against live storage.babbage.systems | ProtoWallet + AuthFetch wiring verified; StorageClient::rpc_call handles full BRC-31 peer handshake on first request |
| TEST-02 | makeAvailable() retrieves Settings from live TS server | StorageClient::make_available() calls `makeAvailable` RPC, caches result; Settings serde verified in Phase 1 |
| TEST-03 | findOrInsertUser() creates/retrieves user on live TS server | StorageClient::find_or_insert_user maps to `findOrInsertUser` RPC; FindOrInsertUserWire helper deserializes tuple response |
| TEST-04 | Sync chunk round-trip (getSyncChunk + processSyncChunk) works against live TS server | StorageClient::get_sync_chunk + process_sync_chunk RPCs implemented; SyncChunk serde verified in Phase 1; full sync loop verified in manager_tests |
| TEST-05 | Full payment internalize flow works end-to-end with StorageClient as backup | WalletBuilder supports full wallet construction; add_wallet_storage_provider attaches StorageClient; internalize_action delegates through manager |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `bsv_wallet_toolbox::ProtoWallet` | 0.1.20 (re-export of bsv-sdk 0.1.75) | `WalletInterface` impl for BRC-31 auth in tests | Crypto-only wallet; no transaction DB required; exactly what tests need |
| `bsv::primitives::private_key::PrivateKey` | bsv-sdk 0.1.75 | Root key for ProtoWallet construction | Already used in `tests/common/mod.rs` |
| `bsv_wallet_toolbox::storage::remoting::StorageClient` | project | The implementation under test | Phase 3 deliverable |
| `tokio` | 1.x | Async test runtime | `#[tokio::test]` already used throughout |
| `std::env::var` | stdlib | Read test credentials from env | Established pattern from postgres/mysql tests |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `bsv_wallet_toolbox::wallet::setup::WalletBuilder` | project | Full wallet for TEST-05..07 | Tests needing internalize_action or updateBackups |
| `tests/common/mod.rs` | project | `create_test_wallet`, `MockWalletServices` | Baseline local wallet construction for hybrid tests |

**Installation:** No new dependencies. All required types are already in the crate and its dependencies.

## Architecture Patterns

### Test File Location
```
tests/
├── storage_client_integration_tests.rs   # NEW — Phase 6 deliverable
├── common/
│   └── mod.rs                            # Existing shared helpers
├── manager_tests.rs                      # Reference for multi-provider patterns
└── storage_postgres_tests.rs             # Reference for #[ignore] + env-var pattern
```

### Pattern 1: Test Infrastructure — ProtoWallet as WalletInterface
**What:** Construct a `ProtoWallet` from a known private key to serve as the `WalletInterface` parameter for `StorageClient::new`.
**When to use:** All TEST-01 through TEST-04 tests (direct StorageClient tests without a full wallet).

```rust
// Source: bsv-sdk-0.1.75/src/wallet/proto_wallet.rs:80-86
// ProtoWallet::new(PrivateKey) implements WalletInterface
use bsv::primitives::private_key::PrivateKey;
use bsv_wallet_toolbox::ProtoWallet;
use bsv_wallet_toolbox::storage::remoting::StorageClient;

fn get_test_key() -> PrivateKey {
    let hex = std::env::var("BSV_TEST_ROOT_KEY")
        .expect("BSV_TEST_ROOT_KEY must be set to run live integration tests");
    PrivateKey::from_hex(&hex).expect("BSV_TEST_ROOT_KEY must be valid hex private key")
}

fn make_storage_client(wallet: ProtoWallet) -> StorageClient<ProtoWallet> {
    let endpoint = "https://staging-storage.babbage.systems".to_string();
    StorageClient::new(wallet, endpoint)
}
```

### Pattern 2: Test Gating — #[ignore] + Feature Gate
**What:** All live tests are marked `#[ignore]` and gated with `#[cfg(feature = "sqlite")]`. Run with `cargo test --features sqlite -- --ignored`.
**When to use:** All tests in `storage_client_integration_tests.rs`.

```rust
// Source: tests/storage_postgres_tests.rs — established project pattern
#[cfg(feature = "sqlite")]
mod storage_client_integration_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_brc31_auth_handshake() { ... }
}
```

### Pattern 3: Full Wallet with StorageClient Backup (TEST-05..07)
**What:** Use `WalletBuilder` + `add_wallet_storage_provider` to build a wallet with a live `StorageClient` as a backup.
**When to use:** TEST-05 (internalize flow), TEST-06 (syncToWriter), TEST-07 (updateBackups).

```rust
// Source: .ref/wallet-toolbox/test/utils/TestUtilsWalletStorage.ts:482-498
// and src/storage/manager.rs:1886 (add_wallet_storage_provider)
use std::sync::Arc;
use bsv_wallet_toolbox::wallet::setup::WalletBuilder;
use bsv_wallet_toolbox::storage::remoting::StorageClient;
use bsv_wallet_toolbox::types::Chain;

async fn make_wallet_with_remote_backup(root_key: PrivateKey) -> SetupWallet {
    let setup = WalletBuilder::new()
        .chain(Chain::Test)
        .root_key(root_key.clone())
        .with_sqlite_memory()
        .with_services(Arc::new(common::MockWalletServices))
        .build()
        .await
        .expect("build wallet");

    // StorageClient takes the Wallet itself (which implements WalletInterface)
    // Use ProtoWallet from same root key for transport auth
    let proto = ProtoWallet::new(root_key);
    let client = Arc::new(StorageClient::new(
        proto,
        "https://staging-storage.babbage.systems",
    )) as Arc<dyn WalletStorageProvider>;

    setup.storage
        .add_wallet_storage_provider(client)
        .await
        .expect("add remote backup");

    setup
}
```

### Pattern 4: Staged Test Isolation
**What:** Tests TEST-01 through TEST-04 test StorageClient directly (no full wallet). Tests TEST-05 through TEST-07 use a full wallet with StorageClient as backup.
**When to use:** Separating concerns — auth/RPC tests (TEST-01..04) from manager orchestration tests (TEST-05..07).

This mirrors the TS test pattern from `createTestWalletWithStorageClient` vs the full-setup variant.

### Recommended Test Sequence
```
TEST-01: make StorageClient, call make_available() — verifies auth handshake + settings retrieval
TEST-02: assert returned Settings has non-empty storage_identity_key
TEST-03: call find_or_insert_user() — verifies user creation/retrieval RPC
TEST-04: call get_sync_chunk() with empty SyncMap, verify result, call process_sync_chunk()
TEST-05: full wallet internalize flow with StorageClient as backup
TEST-06: syncToWriter from local SQLite → remote StorageClient
TEST-07: updateBackups() with StorageClient as backup provider
```

Note: TEST-01 and TEST-02 are the same test in practice — `make_available()` both performs the BRC-31 handshake and returns Settings. Split them as success criteria but combine as one test function if desired.

### Anti-Patterns to Avoid
- **Sharing a single ProtoWallet instance across concurrent tests:** `AuthFetch` is behind `Mutex`; each test should create its own `StorageClient` instance.
- **Using Chain::Main in tests:** Always use `Chain::Test` + `staging-storage.babbage.systems` to avoid polluting mainnet user records.
- **Running live tests in CI without `#[ignore]`:** Will fail in environments without `BSV_TEST_ROOT_KEY` set.
- **Asserting exact Settings field values:** The TS server controls Settings content; assert non-empty/non-zero, not exact values.
- **Not calling make_available() before other methods:** StorageClient requires `make_available()` to be called first (caches settings, sets `settings_cached` flag).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| BRC-31 auth | Custom HTTP client | `AuthFetch` (in StorageClient) | Already wired in StorageClient::rpc_call |
| WalletInterface for tests | Mock struct | `ProtoWallet` | Implements full WalletInterface; handles all auth crypto operations |
| Sync loop test | Manual chunk iteration | Call `sync_to_writer` via manager | sync_to_writer loop is already tested in manager_tests; tests here prove wire compat, not loop logic |
| Environment credentials | Hardcoded keys | `std::env::var("BSV_TEST_ROOT_KEY")` | Follows postgres/mysql test pattern; keeps credentials out of source |

**Key insight:** Phase 6 is not implementing new logic — it is writing tests that prove the existing Phase 3 StorageClient implementation works against a live server. Keep test scope narrow: one assertion per success criterion.

## Common Pitfalls

### Pitfall 1: StorageClient Not Send+Sync When Using ProtoWallet
**What goes wrong:** `StorageClient<ProtoWallet>` fails to compile as `Arc<dyn WalletStorageProvider>` because of missing `Send+Sync` bounds on `ProtoWallet`.
**Why it happens:** The `WalletStorageProvider` trait requires `Send + Sync`. `ProtoWallet` must satisfy those bounds.
**How to avoid:** Verify `ProtoWallet: Send + Sync` compiles. In bsv-sdk 0.1.75, `ProtoWallet` wraps `KeyDeriver` which wraps `PrivateKey` — all are `Send + Sync`. If not, wrap in `Arc<Mutex<ProtoWallet>>` and implement a thin newtype.
**Warning signs:** `the trait bound ProtoWallet: Sync is not satisfied` compiler error.

### Pitfall 2: AuthFetch Timeout on First Request
**What goes wrong:** The BRC-31 handshake on first connection to a cold server times out.
**Why it happens:** The Peer handshake involves multiple round-trips over the network. Default timeout may be too short.
**How to avoid:** Use `#[tokio::test(flavor = "multi_thread")]` for integration tests and ensure the test timeout (set by `tokio::time::timeout`) is generous (30+ seconds for first auth).
**Warning signs:** Test fails with "auth fetch: ..." error or timeout panic.

### Pitfall 3: Staging vs. Main Endpoint Mismatch
**What goes wrong:** Tests connect to `storage.babbage.systems` (main) instead of `staging-storage.babbage.systems` (test chain), creating mainnet user records with test keys.
**Why it happens:** Hardcoded URL.
**How to avoid:** Always use `staging-storage.babbage.systems` for `Chain::Test` in tests. Match the TS TestUtilsWalletStorage pattern: `chain === 'main' ? 'https://storage.babbage.systems' : 'https://staging-storage.babbage.systems'`.
**Warning signs:** Settings returned have unexpected `network: "main"` or real BSV balances visible.

### Pitfall 4: find_or_insert_user Needs AuthId — Not Just identity_key
**What goes wrong:** `WalletStorageProvider::find_or_insert_user` takes `&AuthId` but the test constructs an incorrect `AuthId`.
**Why it happens:** `AuthId` has `identity_key`, `user_id: Option<i64>`, and `is_active: bool`.
**How to avoid:** For the initial call (before user_id is known): construct `AuthId { identity_key: proto.get_public_key_sync(...).to_der_hex(), user_id: None, is_active: false }`. After `make_available()` returns Settings and user is inserted, subsequent calls can use the cached user_id.
**Warning signs:** RPC error "missing userId" or deserialization failure on response.

### Pitfall 5: get_sync_chunk Args Require Both Storage Identity Keys
**What goes wrong:** `get_sync_chunk` / `WalletStorageProvider::get_sync_chunk` requires `from_storage_identity_key` and `to_storage_identity_key` in `RequestSyncChunkArgs`.
**Why it happens:** The sync protocol is bidirectional and needs both endpoints identified.
**How to avoid:** After `make_available()`, the Settings returned contains the remote `storage_identity_key`. Use that as `from`. Use a locally generated random hex as `to` (matches what WalletStorageManager does).
**Warning signs:** Server returns error about missing or invalid storageIdentityKey parameter.

### Pitfall 6: internalize_action Requires a Real BEEF Transaction
**What goes wrong:** TEST-05 (internalize flow) needs a valid BEEF-format transaction to pass to `internalize_action`. Constructing a valid one offline is nontrivial.
**Why it happens:** The TS server validates transaction structure and proof.
**How to avoid:** For TEST-05, use the wallet's own `create_action` output (from local storage + manager). Alternatively, scope TEST-05 to verifying the RPC call reaches the server and returns an expected error for a malformed BEEF, demonstrating the auth and routing layers work. Consult the TS test pattern in `.ref/wallet-toolbox/test/utils/TestUtilsWalletStorage.ts:460-476` for the full "spread change" flow.
**Warning signs:** `processSyncChunk` or `internalizeAction` returns `WERR_INVALID_PARAMETER` about beef format.

## Code Examples

Verified patterns from official sources:

### Constructing StorageClient with ProtoWallet
```rust
// Source: bsv-sdk-0.1.75/src/wallet/proto_wallet.rs, src/storage/remoting/storage_client.rs:123-133
use bsv::primitives::private_key::PrivateKey;
use bsv_wallet_toolbox::ProtoWallet;
use bsv_wallet_toolbox::storage::remoting::StorageClient;

let root_key = PrivateKey::from_hex(&std::env::var("BSV_TEST_ROOT_KEY").unwrap()).unwrap();
let proto_wallet = ProtoWallet::new(root_key);
let client = StorageClient::new(
    proto_wallet,
    "https://staging-storage.babbage.systems",
);
```

### make_available() Call and Settings Assertion
```rust
// Source: src/storage/remoting/storage_client.rs — WalletStorageProvider impl
use bsv_wallet_toolbox::storage::traits::WalletStorageProvider;

let settings = client.make_available().await.expect("makeAvailable should succeed");
assert!(!settings.storage_identity_key.is_empty(),
    "Settings must have non-empty storageIdentityKey");
assert!(client.is_available(), "Client should be available after makeAvailable");
```

### Building AuthId for RPC Calls
```rust
// Source: src/wallet/types.rs (AuthId), src/storage/remoting/storage_client.rs
use bsv_wallet_toolbox::wallet::types::AuthId;

// Derive identity key from ProtoWallet
let identity_key_hex = {
    use bsv::wallet::interfaces::WalletInterface;
    use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};
    // ProtoWallet::get_public_key with identity_key=true returns root pub key
    // Use the hex from the root key directly for test simplicity
    PrivateKey::from_hex(&root_key_hex)
        .unwrap()
        .to_public_key()
        .unwrap()
        .to_der_hex()
};
let auth = AuthId {
    identity_key: identity_key_hex,
    user_id: None,
    is_active: false,
};
```

### get_sync_chunk → process_sync_chunk Round-Trip
```rust
// Source: src/storage/traits/wallet_provider.rs — trait method signatures
// Source: .ref/wallet-toolbox test patterns for sync
use bsv_wallet_toolbox::storage::sync::request_args::RequestSyncChunkArgs;
use bsv_wallet_toolbox::storage::sync::SyncMap;

let remote_settings = client.make_available().await.unwrap();
let sync_map = SyncMap::new();
let args = RequestSyncChunkArgs {
    from_storage_identity_key: remote_settings.storage_identity_key.clone(),
    to_storage_identity_key: "test-local-sik".to_string(),
    user_identity_key: auth.identity_key.clone(),
    max_items: Some(100),
    sync_map: serde_json::to_string(&sync_map).unwrap(),
};
let chunk = client.get_sync_chunk(&auth, args).await.expect("getSyncChunk should succeed");

// process_sync_chunk on the same client merges the received chunk
let result = client.process_sync_chunk(&auth, ...).await.expect("processSyncChunk should succeed");
```

### Full Test Module Structure
```rust
// Source: tests/storage_postgres_tests.rs pattern adapted for live storage client
#[cfg(feature = "sqlite")]
mod storage_client_integration_tests {
    use std::sync::Arc;
    use bsv::primitives::private_key::PrivateKey;
    use bsv_wallet_toolbox::ProtoWallet;
    use bsv_wallet_toolbox::storage::remoting::StorageClient;
    use bsv_wallet_toolbox::storage::traits::WalletStorageProvider;

    const STAGING_ENDPOINT: &str = "https://staging-storage.babbage.systems";

    fn test_root_key() -> PrivateKey {
        let hex = std::env::var("BSV_TEST_ROOT_KEY")
            .expect("BSV_TEST_ROOT_KEY must be set. Run with: BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored");
        PrivateKey::from_hex(&hex).expect("BSV_TEST_ROOT_KEY must be valid hex private key")
    }

    fn make_client() -> StorageClient<ProtoWallet> {
        let wallet = ProtoWallet::new(test_root_key());
        StorageClient::new(wallet, STAGING_ENDPOINT)
    }

    /// TEST-01 + TEST-02: BRC-31 handshake completes, Settings returned.
    #[tokio::test]
    #[ignore]
    async fn test_brc31_auth_and_make_available() {
        let client = make_client();
        let settings = client.make_available().await
            .expect("BRC-31 handshake and makeAvailable must succeed");
        assert!(!settings.storage_identity_key.is_empty(),
            "Settings must contain non-empty storageIdentityKey");
        assert!(client.is_available());
    }

    /// TEST-03: findOrInsertUser creates/retrieves user.
    #[tokio::test]
    #[ignore]
    async fn test_find_or_insert_user() { ... }

    /// TEST-04: Sync chunk round-trip.
    #[tokio::test]
    #[ignore]
    async fn test_sync_chunk_round_trip() { ... }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Direct HTTPS + manual JSON | BRC-31 mutual auth via AuthFetch | Phase 3 | Server validates both client and server identity; no plain HTTP |
| StorageProvider (60+ methods) | WalletStorageProvider (~25 methods) | Phase 2 | StorageClient implements lighter interface; no DB-level methods needed |
| Full wallet required for auth tests | ProtoWallet (crypto only) | bsv-sdk 0.1.75 | Tests need only signing capability; no transaction management |
| Hardcoded test credentials | BSV_TEST_ROOT_KEY env var | This phase | Consistent with existing postgres/mysql test pattern in this repo |

**Deprecated/outdated:**
- `Chain::Main` for tests: Always use `Chain::Test` + staging endpoint.

## Open Questions

1. **Does ProtoWallet implement Send + Sync in bsv-sdk 0.1.75?**
   - What we know: `ProtoWallet` wraps `KeyDeriver` which wraps `PrivateKey` — these contain standard Rust integer types. No `Rc`, no `Cell`, no non-Send types visible in source.
   - What's unclear: Whether bsv-sdk adds any explicit `!Send` bounds.
   - Recommendation: Attempt `Arc::new(StorageClient::new(proto_wallet, url)) as Arc<dyn WalletStorageProvider>` in a compilation-only test first. If it fails, add a `#[allow(dead_code)] fn assert_send_sync<T: Send + Sync>() {}` check and implement a newtype wrapper.

2. **Does StorageClient need Arc to be used as WalletStorageProvider in manager?**
   - What we know: `add_wallet_storage_provider` takes `Arc<dyn WalletStorageProvider>`. `StorageClient` implements `WalletStorageProvider` via `#[async_trait]`.
   - What's unclear: Nothing — the pattern is clear. `Arc::new(client)` is all that's needed.
   - Recommendation: Use `Arc::new(StorageClient::new(...)) as Arc<dyn WalletStorageProvider>`.

3. **What test identity key to use against staging?**
   - What we know: A generated `PrivateKey::from_random()` will work for auth; each distinct key gets its own user record on the server. Using a random key per test run creates new users each time — acceptable for staging.
   - What's unclear: Whether staging server imposes rate limits or caps on user creation.
   - Recommendation: For TEST-01..04, a stable test key from `BSV_TEST_ROOT_KEY` env var is preferred so test runs don't proliferate users. Document this in the test file header.

4. **TEST-05 (internalize_action): What BEEF to use?**
   - What we know: `internalize_action` requires a valid `InternalizeActionArgs` with a BEEF transaction. The TS server validates the BEEF. Constructing a valid BEEF from scratch requires a real transaction.
   - What's unclear: Whether staging has existing funded test wallets we can call `create_action` on.
   - Recommendation: Scope TEST-05 to: (a) build wallet with StorageClient as backup, (b) call `make_available()` and `find_or_insert_user()` through the manager, (c) call `internalize_action` with a minimal BEEF (may return a server-side validation error, but that proves the auth and routing layers worked). The success criterion is "no auth errors" — not "transaction accepted".

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + tokio (1.x) |
| Config file | Cargo.toml `[dev-dependencies]` — tokio with "full" features |
| Quick run command | `cargo test --features sqlite storage_client_integration 2>&1` (skips `#[ignore]` tests) |
| Full suite command | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored storage_client_integration 2>&1` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| TEST-01 | BRC-31 handshake completes against live server (no auth errors) | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_brc31_auth_and_make_available` | ❌ Wave 0 |
| TEST-02 | makeAvailable() returns valid Settings from live TS server | live integration | Combined with TEST-01 in `test_brc31_auth_and_make_available` | ❌ Wave 0 |
| TEST-03 | findOrInsertUser() creates/retrieves user without error | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_find_or_insert_user` | ❌ Wave 0 |
| TEST-04 | getSyncChunk + processSyncChunk round-trip, correct entity deserialization | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_chunk_round_trip` | ❌ Wave 0 |
| TEST-05 | Full payment internalize flow with StorageClient as backup | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_internalize_with_storage_client_backup` | ❌ Wave 0 |
| TEST-06 | syncToWriter() from local SQLite to remote StorageClient | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_sync_to_writer_remote` | ❌ Wave 0 |
| TEST-07 | updateBackups() syncs to StorageClient backup | live integration | `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored test_update_backups_remote` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --features sqlite 2>&1` (does NOT run ignored live tests — confirms no regressions)
- **Per wave merge:** `BSV_TEST_ROOT_KEY=<hex> cargo test --features sqlite -- --ignored 2>&1` (runs live tests)
- **Phase gate:** All live tests pass + full local suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `tests/storage_client_integration_tests.rs` — new file, all TEST-01 through TEST-07
- [ ] `BSV_TEST_ROOT_KEY` — operator must set this env var (not automated; documented in test file header)
- [ ] Framework already present — no new dependencies needed

## Sources

### Primary (HIGH confidence)
- `src/storage/remoting/storage_client.rs` — StorageClient implementation (777 lines); rpc_call, make_available, find_or_insert_user, get_sync_chunk, process_sync_chunk all verified present
- `bsv-sdk-0.1.75/src/wallet/proto_wallet.rs` — ProtoWallet::new(PrivateKey); implements WalletInterface; constructor verified
- `bsv-sdk-0.1.75/src/auth/clients/auth_fetch.rs` — AuthFetch::new(wallet); fetch(&mut self) API; BRC-31 peer management
- `src/storage/manager.rs:1886` — add_wallet_storage_provider signature; takes Arc<dyn WalletStorageProvider>
- `tests/storage_postgres_tests.rs` — established `#[ignore]` + env-var project pattern
- `tests/common/mod.rs` — create_test_wallet, MockWalletServices, seed helpers
- `src/lib.rs:69` — `pub use bsv::wallet::proto_wallet::ProtoWallet` — ProtoWallet is re-exported

### Secondary (MEDIUM confidence)
- `.ref/wallet-toolbox/test/utils/TestUtilsWalletStorage.ts:400-498` — TS reference for `createTestWalletWithStorageClient` pattern; StorageClient construction + makeAvailable + addWalletStorageProvider sequence
- `.planning/REQUIREMENTS.md` — TEST-01..07 requirement text; success criteria definition
- `https://storage.babbage.systems` / `https://staging-storage.babbage.systems` — live endpoints confirmed in multiple project `.env` files

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all types and patterns verified from source code
- Test structure: HIGH — pattern mirrors existing postgres/mysql tests in repo
- Live endpoint URLs: HIGH — confirmed from multiple `.env` files across related projects
- ProtoWallet Send+Sync: MEDIUM — types appear safe but not compiler-verified yet (Wave 0 check)
- TEST-05 exact scope: MEDIUM — internalize_action BEEF construction may need adjustment once we know staging wallet balance

**Research date:** 2026-03-25
**Valid until:** 2026-04-25 (live server availability may change; library APIs stable)
