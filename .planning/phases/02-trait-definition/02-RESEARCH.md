# Phase 2: Trait Definition - Research

**Researched:** 2026-03-24
**Domain:** Rust async trait design, TS-to-Rust interface mapping, blanket impl patterns
**Confidence:** HIGH

## Summary

Phase 2 defines a new `WalletStorageProvider` trait in Rust that mirrors the TypeScript `WalletStorageProvider` interface hierarchy. The TS interface is a narrowed view of `StorageProvider` — it covers ~25 methods relevant to remote clients while omitting the low-level table CRUD operations (insert/update/find all tables) that are only needed by local SQL backends.

The critical design constraint is that `SqlxProvider` (and any future local provider) must satisfy `WalletStorageProvider` via a blanket impl (`impl<T: StorageProvider> WalletStorageProvider for T {}`) without any modification to its source files. The `StorageClient` (Phase 3) will implement `WalletStorageProvider` directly without implementing `StorageProvider`. This exactly matches the TS pattern where `StorageClient implements WalletStorageProvider` but not `StorageProvider`.

The trait must be designed for `Arc<dyn WalletStorageProvider>` dynamic dispatch, meaning it must remain object-safe. All methods are async (via `async_trait`). The `is_storage_provider()` method is the runtime type discriminant: `true` from the blanket impl (local), `false` on `StorageClient` (remote).

**Primary recommendation:** Define `WalletStorageProvider` as a standalone trait (not a supertrait of `StorageProvider`) with the ~25 methods from TS, implement them as a blanket impl over `T: StorageProvider`, and add a single `is_storage_provider() -> bool` method with default `true` that `StorageClient` overrides to `false`.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| TRAIT-01 | WalletStorageProvider async trait defined with ~25 methods matching TS WalletStorageProvider interface hierarchy | Full TS interface fetched and mapped below; exact method count is 25 |
| TRAIT-02 | Blanket impl allows existing StorageProvider types to satisfy WalletStorageProvider | Blanket impl pattern confirmed from `StorageActionProvider` precedent in this codebase (`impl<T: StorageProvider> StorageActionProvider for T {}`) |
| TRAIT-03 | `is_storage_provider()` returns false for remote clients, true for local storage via blanket impl | TS `isStorageProvider()` pattern confirmed in `StorageClient.ts` line 79 |
</phase_requirements>

---

## TS Interface Hierarchy (Source of Truth)

Fetched from: `https://raw.githubusercontent.com/bsv-blockchain/wallet-toolbox/master/src/sdk/WalletStorage.interfaces.ts`

The TS `WalletStorageProvider` extends `WalletStorageSync` which extends `WalletStorageWriter` which extends `WalletStorageReader`. Flattened:

```
WalletStorageProvider
  + isStorageProvider(): boolean
  + setServices(v: WalletServices): void
  extends WalletStorageSync
    + findOrInsertSyncStateAuth(auth, storageIdentityKey, storageName): Promise<{syncState, isNew}>
    + setActive(auth, newActiveStorageIdentityKey): Promise<number>
    + getSyncChunk(args: RequestSyncChunkArgs): Promise<SyncChunk>
    + processSyncChunk(args: RequestSyncChunkArgs, chunk: SyncChunk): Promise<ProcessSyncChunkResult>
    extends WalletStorageWriter
      + makeAvailable(): Promise<TableSettings>
      + migrate(storageName, storageIdentityKey): Promise<string>
      + destroy(): Promise<void>
      + findOrInsertUser(identityKey): Promise<{user, isNew}>
      + abortAction(auth, args): Promise<AbortActionResult>
      + createAction(auth, args): Promise<StorageCreateActionResult>
      + processAction(auth, args): Promise<StorageProcessActionResults>
      + internalizeAction(auth, args): Promise<StorageInternalizeActionResult>
      + insertCertificateAuth(auth, certificate): Promise<number>
      + relinquishCertificate(auth, args): Promise<number>
      + relinquishOutput(auth, args): Promise<number>
      extends WalletStorageReader
        + isAvailable(): boolean
        + getServices(): WalletServices
        + getSettings(): TableSettings
        + findCertificatesAuth(auth, args): Promise<TableCertificateX[]>
        + findOutputBasketsAuth(auth, args): Promise<TableOutputBasket[]>
        + findOutputsAuth(auth, args): Promise<TableOutput[]>
        + findProvenTxReqs(args): Promise<TableProvenTxReq[]>
        + listActions(auth, vargs): Promise<ListActionsResult>
        + listCertificates(auth, vargs): Promise<ListCertificatesResult>
        + listOutputs(auth, vargs): Promise<ListOutputsResult>
```

**Total methods: 25** (excluding `isStorageProvider` and `setServices` which are 2 more = 27 total declared in `WalletStorageProvider`).

Note: The TS `StorageClient` also implements `updateProvenTxReqWithNewProvenTx` and has `getChain`/`isActive`/`setActive` equivalents from the `StorageProvider` interface. The `WalletStorageProvider` interface itself does NOT include these — they are in the broader `StorageProvider` or `WalletStorageSync`. The count of ~25 is accurate.

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| async-trait | 0.1 | Enable async methods in traits for dyn dispatch | Already in Cargo.toml; required for `#[async_trait]` on object-safe async traits |
| tokio | 1 | Async runtime | Already present with `sync` feature |

No new dependencies needed for Phase 2.

### Supporting
No new libraries needed. The trait uses types already defined in this codebase:
- `AuthId` from `crate::wallet::types`
- `Settings` / table types from `crate::tables`
- `SyncChunk`, `ProcessSyncChunkResult` from `crate::storage::sync`
- `WalletResult` from `crate::error`
- Action types from `crate::storage::action_types`

---

## Architecture Patterns

### Existing Blanket Impl Precedent

The codebase already uses this exact pattern in `src/storage/action_traits.rs`:

```rust
// Source: src/storage/action_traits.rs line 85
impl<T: StorageProvider> StorageActionProvider for T {}
```

This is the model for Phase 2. `WalletStorageProvider` will use the identical pattern:

```rust
impl<T: StorageProvider> WalletStorageProvider for T {}
```

### Recommended File Location

```
src/storage/traits/
├── mod.rs              # add: pub mod wallet_provider; pub use wallet_provider::WalletStorageProvider;
├── provider.rs         # existing StorageProvider (unchanged)
├── reader.rs           # existing StorageReader (unchanged)
├── reader_writer.rs    # existing StorageReaderWriter (unchanged)
└── wallet_provider.rs  # NEW: WalletStorageProvider trait + blanket impl
```

### Pattern: Standalone Trait (not supertrait)

`WalletStorageProvider` should NOT extend `StorageProvider`. Instead:
- It is a standalone trait with its own method set
- The blanket impl connects `StorageProvider` → `WalletStorageProvider`
- `StorageClient` implements `WalletStorageProvider` directly (no `StorageProvider`)

This mirrors TS exactly and avoids the blanket impl being blocked by `StorageProvider`'s abstract methods (since `StorageClient` cannot implement all table-level CRUD).

### Method Signature Mapping: TS → Rust

**Key translation rules:**
1. TS `AuthId { identityKey, userId?, isActive? }` → Rust `AuthId` in `crate::wallet::types` (currently only has `identity_key: String`). The Rust `AuthId` needs `user_id: Option<i64>` and `is_active: Option<bool>` fields added, or a new `WalletAuthId` struct.
2. TS `Promise<T>` → `async fn ... -> WalletResult<T>`
3. TS `boolean` → `bool`
4. TS `string` → `String` or `&str`
5. TS `number` → `i64` (IDs) or `u64` (amounts)
6. TS `Date` → `chrono::NaiveDateTime`
7. TS `TableSettings` → `crate::tables::Settings`
8. TS `TableUser` → `crate::tables::User`
9. TS `TableSyncState` → `crate::tables::SyncState`
10. TS `TableCertificateX` → `crate::tables::Certificate` (with fields included)

**CRITICAL: AuthId discrepancy.** The TS `WalletStorageProvider` interface uses `AuthId` with `userId?` and `isActive?`. The existing Rust `AuthId` struct only has `identity_key: String`. Phase 2 must extend `AuthId` or define a new `WalletAuthId` struct. **Recommendation:** Add optional fields to the existing `AuthId`:

```rust
pub struct AuthId {
    pub identity_key: String,
    pub user_id: Option<i64>,    // ADD: maps to TS userId?
    pub is_active: Option<bool>, // ADD: maps to TS isActive?
}
```

**CRITICAL: RequestSyncChunkArgs gap.** The TS `RequestSyncChunkArgs` is the wire format for `getSyncChunk`/`processSyncChunk`. The Rust codebase has `GetSyncChunkArgs<'a>` which is an internal type with a borrowed `SyncMap`. These are NOT the same. The `WalletStorageProvider` trait methods must use a new `RequestSyncChunkArgs` struct that matches the TS wire format:

```rust
pub struct RequestSyncChunkArgs {
    pub from_storage_identity_key: String,
    pub to_storage_identity_key: String,
    pub identity_key: String,
    pub since: Option<chrono::NaiveDateTime>,
    pub max_rough_size: i64,
    pub max_items: i64,
    pub offsets: Vec<SyncChunkOffset>,  // { name: String, offset: i64 }
}

pub struct SyncChunkOffset {
    pub name: String,
    pub offset: i64,
}
```

**UpdateProvenTxReqWithNewProvenTxArgs.** The TS `StorageClient` implements `updateProvenTxReqWithNewProvenTx`. The Rust `StorageReaderWriter` already has `update_proven_tx_req_with_new_proven_tx` as a default method. For `WalletStorageProvider`, this method should be included if it appears in the TS `WalletStorageProvider` interface. **Verification:** It does NOT appear in the `WalletStorageProvider` interface — it is a `StorageProvider`-level method. Exclude from `WalletStorageProvider`.

### The 25 Rust Method Signatures

Based on the TS interface hierarchy, the `WalletStorageProvider` trait in Rust should declare:

**From WalletStorageReader (sync/non-async):**
1. `fn is_available(&self) -> bool`
2. `fn get_settings(&self) -> WalletResult<Settings>`

**From WalletStorageReader (async):**
3. `async fn find_certificates_auth(&self, auth: &AuthId, args: &FindCertificatesArgs, ...) -> WalletResult<Vec<Certificate>>`
4. `async fn find_output_baskets_auth(&self, auth: &AuthId, args: &FindOutputBasketsArgs, ...) -> WalletResult<Vec<OutputBasket>>`
5. `async fn find_outputs_auth(&self, auth: &AuthId, args: &FindOutputsArgs, ...) -> WalletResult<Vec<Output>>`
6. `async fn find_proven_tx_reqs(&self, args: &FindProvenTxReqsArgs, ...) -> WalletResult<Vec<ProvenTxReq>>`
7. `async fn list_actions(&self, auth: &AuthId, args: ...) -> WalletResult<ListActionsResult>`
8. `async fn list_certificates(&self, auth: &AuthId, args: ...) -> WalletResult<ListCertificatesResult>`
9. `async fn list_outputs(&self, auth: &AuthId, args: ...) -> WalletResult<ListOutputsResult>`

**From WalletStorageWriter (async):**
10. `async fn make_available(&self) -> WalletResult<Settings>`
11. `async fn migrate(&self, storage_name: &str, storage_identity_key: &str) -> WalletResult<String>`
12. `async fn destroy(&self) -> WalletResult<()>`
13. `async fn find_or_insert_user(&self, identity_key: &str) -> WalletResult<(User, bool)>`
14. `async fn abort_action(&self, auth: &AuthId, args: &AbortActionArgs) -> WalletResult<AbortActionResult>`
15. `async fn create_action(&self, auth: &AuthId, args: ...) -> WalletResult<StorageCreateActionResult>`
16. `async fn process_action(&self, auth: &AuthId, args: ...) -> WalletResult<StorageProcessActionResult>`
17. `async fn internalize_action(&self, auth: &AuthId, args: ...) -> WalletResult<StorageInternalizeActionResult>`
18. `async fn insert_certificate_auth(&self, auth: &AuthId, certificate: &Certificate) -> WalletResult<i64>`
19. `async fn relinquish_certificate(&self, auth: &AuthId, args: ...) -> WalletResult<i64>`
20. `async fn relinquish_output(&self, auth: &AuthId, args: ...) -> WalletResult<i64>`

**From WalletStorageSync (async):**
21. `async fn find_or_insert_sync_state_auth(&self, auth: &AuthId, storage_identity_key: &str, storage_name: &str) -> WalletResult<(SyncState, bool)>`
22. `async fn set_active(&self, auth: &AuthId, new_active_storage_identity_key: &str) -> WalletResult<i64>`
23. `async fn get_sync_chunk(&self, args: &RequestSyncChunkArgs) -> WalletResult<SyncChunk>`
24. `async fn process_sync_chunk(&self, args: &RequestSyncChunkArgs, chunk: &SyncChunk) -> WalletResult<ProcessSyncChunkResult>`

**From WalletStorageProvider itself:**
25. `fn is_storage_provider(&self) -> bool` — default `true`, override to `false` on StorageClient

That is exactly 25 methods (24 async + 1 sync discriminant). The non-async `is_available()` and `get_settings()` from WalletStorageReader, plus the two non-async from WalletStorageProvider itself, keep the object-safety tractable.

**Note on `getServices`/`setServices`:** The TS interface includes these. In the Rust codebase there is no `WalletServices` trait equivalent yet exposed via `StorageProvider`. These can be included with a default `unimplemented!()` body or omitted if `StorageProvider` doesn't have them. Safest: include as methods that return `WalletError::NotImplemented` by default.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Async trait object safety | Manual wrapper types | `async_trait` crate | RPITIT not yet stable for object-safe async traits in Rust stable |
| Blanket impl | Per-type impls | `impl<T: StorageProvider> WalletStorageProvider for T {}` | Same pattern already used for `StorageActionProvider` in this codebase |

---

## Common Pitfalls

### Pitfall 1: Blanket Impl Conflicts with `StorageClient`
**What goes wrong:** If `StorageClient` (Phase 3) also implements `StorageProvider`, the blanket impl and the direct impl conflict. Rust's orphan rules will refuse both.
**Why it happens:** `impl<T: StorageProvider> WalletStorageProvider for T` is a blanket impl; adding `impl WalletStorageProvider for StorageClient` causes "conflicting implementations" if `StorageClient` also impls `StorageProvider`.
**How to avoid:** `StorageClient` must NOT implement `StorageProvider` — only `WalletStorageProvider` directly. This is explicitly called out in REQUIREMENTS.md Out of Scope: "StorageProvider trait impl on StorageClient".
**Warning signs:** Compiler error "conflicting implementations of trait".

### Pitfall 2: AuthId Field Mismatch
**What goes wrong:** The existing Rust `AuthId` struct lacks `user_id` and `is_active` fields that TS `AuthId` has. Blanket impl methods that delegate to `StorageProvider` (which takes `auth: &str`) must strip these fields anyway — but for `WalletStorageProvider` trait methods that are called from `StorageClient`, the full `AuthId` is needed for JSON serialization.
**How to avoid:** Extend `AuthId` with `pub user_id: Option<i64>` and `pub is_active: Option<bool>`. The existing usage of `AuthId { identity_key }` still works since the new fields are `Option` with `None` defaults.
**Warning signs:** Missing fields when serializing `auth` parameter for `rpc_call` in Phase 3.

### Pitfall 3: RequestSyncChunkArgs vs GetSyncChunkArgs
**What goes wrong:** Using the internal `GetSyncChunkArgs<'a>` in the `WalletStorageProvider` trait breaks object safety (lifetime parameter) and doesn't match the TS wire format.
**How to avoid:** Define a new wire-compatible `RequestSyncChunkArgs` (owned, no lifetime) in `src/storage/` that matches the TS interface. The internal `GetSyncChunkArgs<'a>` remains for the local SQL implementation.
**Warning signs:** "the parameter type `T` may not live long enough" or object-safety errors.

### Pitfall 4: Default Methods Breaking Object Safety
**What goes wrong:** Default methods that call `self.other_method()` in a trait with `async_trait` can sometimes fail object safety.
**How to avoid:** Keep `WalletStorageProvider` methods abstract (no defaults except `is_storage_provider`). The blanket impl provides concrete implementations by delegating to `StorageProvider` methods.
**Warning signs:** "the trait `WalletStorageProvider` cannot be made into an object".

### Pitfall 5: Method Name Wire Discrepancy
**What goes wrong:** TS `findOutputBasketsAuth` calls `rpcCall('findOutputBaskets', ...)` (no "Auth" suffix). Phase 3 must use the wire method names, not the Rust method names.
**Why it matters now:** Phase 2 trait method names only affect Rust code, but the trait doc comments should note the wire method name for Phase 3's reference.
**Wire name differences found:**
- `findOutputBasketsAuth` → wire: `"findOutputBaskets"` (no Auth)
- `findOutputsAuth` → wire: `"findOutputsAuth"` (keeps Auth)
- `findCertificatesAuth` → wire: `"findCertificatesAuth"` (keeps Auth)

---

## Code Examples

### Blanket Impl Pattern (from existing codebase)

```rust
// Source: src/storage/action_traits.rs
/// Blanket implementation: every StorageProvider automatically implements
/// StorageActionProvider with the default stubs.
impl<T: StorageProvider> StorageActionProvider for T {}
```

This is exactly the pattern Phase 2 uses.

### WalletStorageProvider Trait Skeleton

```rust
// Source pattern: mirrors TS WalletStorageProvider interface hierarchy
#[async_trait]
pub trait WalletStorageProvider: Send + Sync {
    // ---- WalletStorageProvider level ----
    fn is_storage_provider(&self) -> bool {
        true  // default for local providers; StorageClient overrides to false
    }

    // ---- WalletStorageReader level ----
    fn is_available(&self) -> bool;
    fn get_settings(&self) -> WalletResult<Settings>;

    async fn find_certificates_auth(
        &self,
        auth: &AuthId,
        args: &FindCertificatesArgs,
    ) -> WalletResult<Vec<Certificate>>;

    // ... 21 more methods

    // ---- WalletStorageSync level ----
    async fn get_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
    ) -> WalletResult<SyncChunk>;

    async fn process_sync_chunk(
        &self,
        args: &RequestSyncChunkArgs,
        chunk: &SyncChunk,
    ) -> WalletResult<ProcessSyncChunkResult>;
}

/// Blanket impl: every local StorageProvider satisfies WalletStorageProvider.
/// StorageClient implements WalletStorageProvider directly WITHOUT this blanket impl.
#[async_trait]
impl<T: StorageProvider> WalletStorageProvider for T {
    fn is_storage_provider(&self) -> bool { true }
    fn is_available(&self) -> bool { StorageProvider::is_available(self) }
    fn get_settings(&self) -> WalletResult<Settings> {
        StorageProvider::get_settings(self, None)  // blocks on async — see note below
    }
    // ... delegate to existing StorageProvider methods
}
```

**Note on sync `get_settings`:** The TS `getSettings()` is synchronous (returns cached value). The Rust `StorageProvider::get_settings` takes `Option<&TrxToken>` and is async. The `WalletStorageProvider::get_settings` should be sync and return the cached value. In the blanket impl, delegate to a sync cache read — the `SqlxProvider` caches settings after `make_available()`. If the cache is unavailable, return `WalletError::NotImplemented`. This needs to be verified against the actual `StorageSqlx` struct's settings cache field before implementation.

### RequestSyncChunkArgs (new type needed)

```rust
/// Wire-format args for getSyncChunk / processSyncChunk.
/// Matches TypeScript RequestSyncChunkArgs exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestSyncChunkArgs {
    #[serde(rename = "fromStorageIdentityKey")]
    pub from_storage_identity_key: String,
    #[serde(rename = "toStorageIdentityKey")]
    pub to_storage_identity_key: String,
    #[serde(rename = "identityKey")]
    pub identity_key: String,
    #[serde(with = "crate::storage::serde_datetime", skip_serializing_if = "Option::is_none")]
    pub since: Option<chrono::NaiveDateTime>,
    #[serde(rename = "maxRoughSize")]
    pub max_rough_size: i64,
    #[serde(rename = "maxItems")]
    pub max_items: i64,
    pub offsets: Vec<SyncChunkOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChunkOffset {
    pub name: String,
    pub offset: i64,
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| GAT-based async traits | `async_trait` macro | stable async fn in traits landed in Rust 1.75 but not yet object-safe | Must use `#[async_trait]` for `dyn WalletStorageProvider` |
| Hand-writing per-provider impls | Blanket impl over supertrait | Already in codebase for `StorageActionProvider` | Phase 2 follows same established pattern |

---

## Open Questions

1. **`get_settings()` sync vs async in blanket impl**
   - What we know: TS `getSettings()` is sync (returns cached). Rust `StorageProvider::get_settings` is async and takes `TrxToken`.
   - What's unclear: Does `SqlxProvider` have an in-memory settings cache field accessible synchronously?
   - Recommendation: Check `StorageSqlx` struct definition before writing blanket impl. If no sync cache exists, define `get_settings()` in `WalletStorageProvider` as async and live with the TS/Rust mismatch. Alternatively wrap in a `RwLock<Option<Settings>>` cache.

2. **FindCertificatesArgs with fields in WalletStorageProvider vs StorageReader**
   - What we know: `WalletStorageProvider` uses `findCertificatesAuth(auth, FindCertificatesArgs)` while `StorageReader` uses `find_certificates(&FindCertificatesArgs, Option<&TrxToken>)`. These are different — the `WalletStorageProvider` version is "auth-scoped" (only returns the auth'd user's certs).
   - What's unclear: Should the blanket impl for `find_certificates_auth` add a user_id filter automatically, or pass through as-is?
   - Recommendation: In the blanket impl, add `user_id` from `auth` to the `FindCertificatesArgs` before delegating to `StorageReader::find_certificates`. This requires the blanket impl to do a small amount of work, not just forward.

3. **`getServices`/`setServices` inclusion**
   - What we know: TS `WalletStorageReader` requires `getServices(): WalletServices`. The Rust codebase doesn't expose services from storage.
   - Recommendation: Include both methods in `WalletStorageProvider` with default `WalletError::NotImplemented` bodies. The blanket impl can optionally delegate to `StorageProvider` if it gains service methods in the future.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` / `#[tokio::test]` |
| Config file | `Cargo.toml` (test feature gating via `#[cfg(feature = "sqlite")]`) |
| Quick run command | `cargo test -p bsv-wallet-toolbox --features sqlite trait -- --test-output immediate 2>&1` |
| Full suite command | `cargo test -p bsv-wallet-toolbox --features sqlite 2>&1` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| TRAIT-01 | `WalletStorageProvider` compiles with ~25 methods | unit (compile check) | `cargo test --features sqlite wallet_storage_provider_compiles` | ❌ Wave 0 |
| TRAIT-02 | `SqlxProvider` satisfies `WalletStorageProvider` via blanket impl | unit (compile check) | `cargo test --features sqlite blanket_impl_satisfies_wallet_provider` | ❌ Wave 0 |
| TRAIT-03 | `is_storage_provider()` returns `true` from blanket impl | unit | `cargo test --features sqlite is_storage_provider_true_for_local` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p bsv-wallet-toolbox --features sqlite trait`
- **Per wave merge:** `cargo test -p bsv-wallet-toolbox --features sqlite`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `tests/wallet_provider_trait_tests.rs` — covers TRAIT-01, TRAIT-02, TRAIT-03
  - Test 1: `fn wallet_storage_provider_object_safe()` — proves `Arc<dyn WalletStorageProvider>` compiles
  - Test 2: `fn sqlite_storage_satisfies_wallet_provider()` — proves blanket impl compiles for `SqliteStorage`
  - Test 3: `fn is_storage_provider_returns_true()` — runtime check on blanket impl default

---

## Sources

### Primary (HIGH confidence)
- Fetched `https://raw.githubusercontent.com/bsv-blockchain/wallet-toolbox/master/src/sdk/WalletStorage.interfaces.ts` — full `WalletStorageProvider`, `WalletStorageSync`, `WalletStorageWriter`, `WalletStorageReader`, `AuthId`, `RequestSyncChunkArgs` interface definitions
- Fetched `https://raw.githubusercontent.com/bsv-blockchain/wallet-toolbox/master/src/storage/remoting/StorageClient.ts` — all 25 method implementations confirming wire method names and parameter ordering
- Repo source: `src/storage/action_traits.rs` — blanket impl precedent `impl<T: StorageProvider> StorageActionProvider for T {}`
- Repo source: `src/storage/traits/` — complete existing Rust trait hierarchy

### Secondary (MEDIUM confidence)
- Repo source: `src/storage/sqlx_impl/trait_impls.rs` — confirms `StorageSqlx<Sqlite>` implements `StorageReaderWriter` and `StorageProvider`; confirms blanket impl approach will cover `SqliteStorage`

### Tertiary (LOW confidence)
- None

---

## Metadata

**Confidence breakdown:**
- TS interface methods: HIGH — fetched raw source from GitHub
- Blanket impl pattern: HIGH — direct precedent in codebase (`action_traits.rs`)
- `is_storage_provider` semantics: HIGH — fetched from `StorageClient.ts` source
- `RequestSyncChunkArgs` wire format: HIGH — fetched from `WalletStorage.interfaces.ts`
- `AuthId` extension need: HIGH — confirmed by comparing TS and Rust structs
- `get_settings` sync/async resolution: MEDIUM — needs `SqlxProvider` settings cache inspection

**Research date:** 2026-03-24
**Valid until:** 2026-06-24 (stable — TS interface unlikely to change during this milestone)
