# Phase 1: Wire Format - Research

**Researched:** 2026-03-24
**Domain:** Rust serde serialization / TypeScript JSON wire format compatibility
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- The goal is PARITY with the TypeScript StorageClient, not adherence to any specific fix list
- Read the TS source code to determine exact wire behavior, then make Rust match
- If the existing Rust serialization already matches TS, leave it alone
- If it doesn't match, fix it — the TS server is the authority
- Don't assume issues exist based on prior conversation — verify each one against TS source

### Claude's Discretion
- All serialization implementation details (timestamp format, binary encoding, optional field handling)
- Test fixture strategy (capture from live server, hand-craft, generate from TS SDK)
- Which entities and edge cases to cover in round-trip tests
- Whether to fix serde_datetime or use a different approach entirely
- Changes to the Rust storage server if needed for interop

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| WIRE-01 | serde_datetime emits ISO 8601 timestamps with trailing "Z" and 3-digit millisecond precision matching TS server expectations | Confirmed: current format is wrong (no Z, variable ms digits). Fix: change format string to `%.3f` + append `Z`. |
| WIRE-02 | Vec<u8> fields serialize as JSON number arrays (not base64), matching TS `Array.from(Buffer)` wire format | Confirmed: current serde default already produces integer arrays. No change needed. |
| WIRE-03 | SyncChunk and SyncMap types serialize to JSON matching TS wire format (camelCase fields, optional arrays as null/absent) | Confirmed: camelCase is correct; optional arrays need `skip_serializing_if = "Option::is_none"` on SyncChunk. SyncMap is internal storage, not wire format. |
| WIRE-04 | Round-trip serde tests pass with TS-generated fixture JSON for all entity types with timestamps and binary data | New tests needed: timestamp-Z-format test, binary-as-integers test, SyncChunk optional-absent test, TS-fixture round-trip deserialization test. |
</phase_requirements>

## Summary

This phase establishes that Rust serialization produces JSON byte-for-byte compatible with what the TypeScript StorageServer accepts. Research involved reading the TS wallet-toolbox source directly from local `node_modules`, examining the TS table interface definitions, and running the existing Rust test suite against the current code.

**Three concrete issues were identified by reading TS source, two confirmed real, one confirmed already correct:**

1. **Timestamp format (WIRE-01) — REAL ISSUE.** The current `serde_datetime` serialize function uses chrono format string `%Y-%m-%dT%H:%M:%S%.f`. The `%.f` specifier produces variable-precision fractional seconds with trailing zeros stripped (e.g., `10:30:00.7` instead of `10:30:00.700`), and does not append `Z`. TypeScript's `Date.toISOString()` always produces exactly 3 millisecond digits and a trailing `Z` (e.g., `2024-01-15T10:30:00.718Z`). Fix: use `%.3f` and append `Z`.

2. **Vec<u8> binary encoding (WIRE-02) — ALREADY CORRECT.** TS interfaces define binary fields as `number[]` (e.g., `merklePath: number[]`, `rawTx: number[]`, `inputBEEF?: number[]`). Serde's default Vec<u8> serialization produces JSON integer arrays. The existing `vec_u8_some_serializes_as_array` test already verifies this. No change needed.

3. **SyncChunk optional arrays (WIRE-03) — REAL ISSUE.** The TS `SyncChunk` interface uses optional fields (`provenTxs?: TableProvenTx[]`), which means absent fields are `undefined` in JavaScript — omitted when `JSON.stringify` serializes them. The Rust `SyncChunk` struct uses `Option<Vec<T>>` with no `skip_serializing_if`, so `None` fields serialize as JSON `null`. The TS `MergeEntity.merge()` checks `if (!this.stateArray) return` which handles both `undefined` and `null`, so null is technically safe — but the canonical wire format omits absent fields entirely. Fix: add `#[serde(skip_serializing_if = "Option::is_none")]` to all optional array fields on `SyncChunk`.

**Primary recommendation:** Fix `serde_datetime` serialize to use `%.3f` format and append `Z`, then add `skip_serializing_if` to SyncChunk's optional fields, then write TS-fixture round-trip tests to lock in the behavior.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde | 1 | Rust serialization framework | Already in Cargo.toml, used throughout |
| serde_json | 1 | JSON encode/decode | Already in Cargo.toml, used throughout |
| chrono | 0.4 | Date/time with serde feature | Already in Cargo.toml; provides NaiveDateTime + format strings |

### No New Dependencies Needed
All fixes use existing dependencies. No crate additions required.

## Architecture Patterns

### Recommended Project Structure
No structural changes. All changes are in existing files:
```
src/
├── serde_datetime.rs        # Fix: serialize format + Z suffix
└── storage/sync/
    └── sync_map.rs          # Fix: add skip_serializing_if to SyncChunk
tests/
└── table_tests.rs           # Extend: add wire-format-specific assertions
```

### Pattern 1: Timestamp Serialization Fix

**What:** Change the serialize function in `serde_datetime` to use `%.3f` (exactly 3 ms digits) and append `Z`.

**When to use:** On all `NaiveDateTime` fields that cross the wire (all 16 table structs via `#[serde(with = "crate::serde_datetime")]`).

**Example:**
```rust
// src/serde_datetime.rs — serialize function
// BEFORE (current):
const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";
pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer,
{
    let s = date.format(FORMAT).to_string();
    serializer.serialize_str(&s)
}

// AFTER (corrected):
const SERIALIZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";

pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer,
{
    let s = format!("{}Z", date.format(SERIALIZE_FORMAT));
    serializer.serialize_str(&s)
}
```

**Deserialize does not change** — `%.f` remains correct for parsing (accepts variable precision), and `trim_end_matches('Z')` already handles the incoming Z.

### Pattern 2: SyncChunk Optional Array Fields

**What:** Add `skip_serializing_if = "Option::is_none"` to all optional entity-list fields in `SyncChunk`.

**When to use:** On all `Option<Vec<T>>` fields that represent entity lists (not on primitive `Option<T>` fields where null has semantic meaning).

**Example:**
```rust
// src/storage/sync/sync_map.rs — SyncChunk struct
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChunk {
    pub from_storage_identity_key: String,
    pub to_storage_identity_key: String,
    pub user_identity_key: String,
    pub user: Option<User>,  // single object — keep as-is (null vs absent is fine here)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven_txs: Option<Vec<ProvenTx>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_baskets: Option<Vec<OutputBasket>>,
    // ... all entity-list fields get skip_serializing_if
}
```

### Pattern 3: TS-Fixture Round-Trip Tests

**What:** Craft JSON strings that match the exact format the TS server produces, then deserialize them into Rust structs and assert field values.

**When to use:** As the final validation step. Tests use hard-coded JSON strings (not captured from live server) because the format is fully specified.

**Example:**
```rust
// tests/table_tests.rs — new test
#[test]
fn proven_tx_ts_fixture_deserializes() {
    // This is the exact format TS Date.toISOString() produces — 3ms digits + Z
    let json = r#"{
        "created_at": "2024-01-15T10:30:00.718Z",
        "updated_at": "2024-01-15T10:31:00.000Z",
        "provenTxId": 42,
        "txid": "abcd1234",
        "height": 800000,
        "index": 5,
        "merklePath": [1, 2, 3],
        "rawTx": [4, 5],
        "blockHash": "blockhash123",
        "merkleRoot": "merkleroot456"
    }"#;
    let pt: ProvenTx = serde_json::from_str(json).unwrap();
    assert_eq!(pt.proven_tx_id, 42);
    // binary fields deserialize as integers
    assert_eq!(pt.merkle_path, vec![1u8, 2, 3]);
}

#[test]
fn proven_tx_serializes_with_z_and_3ms_digits() {
    // Verify Rust -> TS server direction
    let json = serde_json::to_string(&sample_proven_tx_with_millis()).unwrap();
    // Must have trailing Z
    assert!(json.contains("\"created_at\":"), "field must be present");
    // The value must end in Z
    // Extract value and check format
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let ts = v["created_at"].as_str().unwrap();
    assert!(ts.ends_with('Z'), "timestamp must end with Z, got: {}", ts);
    // Must have exactly 3 ms digits before Z
    let ms_part = &ts[ts.len()-4..ts.len()-1]; // 3 chars before Z
    assert_eq!(ms_part.len(), 3);
    assert!(ms_part.chars().all(|c| c.is_ascii_digit()), "ms must be digits: {}", ms_part);
}
```

### Anti-Patterns to Avoid

- **Changing the deserialize FORMAT:** The existing `%.f` on the parse side correctly handles any precision input. Do not change `%.f` to `%.3f` for parsing — that would reject timestamps with 1, 2, or 6 digit ms from other sources.
- **Adding `#[serde(skip_serializing_if)]` to table struct Option fields:** Table struct optional primitives (`Option<i64>`, `Option<String>`) correctly serialize as `null` — this matches TS interface optional primitives that are sent as `null` not omitted. Only `SyncChunk` entity-list arrays should be omitted.
- **Using separate serialize/deserialize format strings for the option module:** The `option` submodule in `serde_datetime` duplicates the FORMAT constant — both the outer and inner FORMAT constants need updating consistently.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| 3-digit ms formatting | Custom ms-extraction + string building | `date.format("%Y-%m-%dT%H:%M:%S%.3f")` | chrono `%.3f` handles zero-padding correctly |
| Z suffix | Regex or string slicing | `format!("{}Z", date.format(...))` | Simple string append, no edge cases |
| Vec<u8> as integers | Custom serializer | serde default | Already produces integer arrays — this is correct behavior |
| Optional field omission | Custom Serialize impl | `#[serde(skip_serializing_if = "Option::is_none")]` | Standard serde attribute, already used in this codebase |

## Common Pitfalls

### Pitfall 1: Changing Parse Format to %.3f
**What goes wrong:** Tests that build NaiveDateTime from timestamps with 1-digit ms (e.g., `"10:30:00.7"`) will fail to parse because `%.3f` requires exactly 3 digits.
**Why it happens:** Conflating serialize format with deserialize format.
**How to avoid:** Keep two distinct format strings — `SERIALIZE_FORMAT` (uses `%.3f`) and `PARSE_FORMAT` (uses `%.f`).
**Warning signs:** Deserialization tests for edge-case timestamps fail.

### Pitfall 2: Forgetting the option module
**What goes wrong:** `serde_datetime::option::serialize` still uses the old format, so `Option<NaiveDateTime>` fields (like `EntitySyncMap.max_updated_at`) serialize without Z or with wrong ms precision.
**Why it happens:** The `option` submodule duplicates the FORMAT constant.
**How to avoid:** Update both the top-level `FORMAT` (for parsing) and add a `SERIALIZE_FORMAT` in both the outer module and the `option` submodule.
**Warning signs:** `max_updated_at` timestamps don't match expected format in tests.

### Pitfall 3: Dual rename attribute conflict
**What goes wrong:** Table structs use `#[serde(rename = "created_at", alias = "createdAt")]` — after fixing serialize to emit Z, the `rename = "created_at"` ensures the JSON key is `created_at` not `createdAt`. This is intentional — the TS interface uses `created_at: Date` (snake_case) at the field level even though type properties are camelCase overall.
**Why it happens:** TS `EntityTimeStamp` interface uses `created_at` and `updated_at` as snake_case field names (not converted to camelCase by the rename_all).
**How to avoid:** Do not change the field rename attributes. The `rename = "created_at"` is correct — TS sends these as `created_at` not `createdAt`.
**Warning signs:** Deserialization failures on `created_at` / `updated_at` from TS fixtures.

### Pitfall 4: Vec<u8> tests asserting array format
**What goes wrong:** A test like `assert!(json.contains("[1,2,3]"))` fails because serde outputs `[1, 2, 3]` with spaces or in a different order.
**Why it happens:** Whitespace/ordering assumptions in string matching.
**How to avoid:** Parse JSON and check as `serde_json::Value` array, not string matching.
**Warning signs:** Flaky tests depending on serializer whitespace.

## Code Examples

### Fixed serde_datetime.rs Serialize Pattern
```rust
// Source: chrono docs — %.3f produces exactly 3 decimal digits for milliseconds
// This changes only serialize output; parse remains %.f (variable, tolerant)

const PARSE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";
const SERIALIZE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";

pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = format!("{}Z", date.format(SERIALIZE_FORMAT));
    serializer.serialize_str(&s)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let trimmed = s.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(trimmed, PARSE_FORMAT).map_err(serde::de::Error::custom)
}
```

### SyncChunk with skip_serializing_if
```rust
// Source: serde docs — skip_serializing_if omits field from JSON when predicate is true
// This matches TS JSON.stringify behavior where undefined fields are absent

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChunk {
    pub from_storage_identity_key: String,
    pub to_storage_identity_key: String,
    pub user_identity_key: String,
    pub user: Option<User>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven_txs: Option<Vec<ProvenTx>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_baskets: Option<Vec<OutputBasket>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactions: Option<Vec<Transaction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Output>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_labels: Option<Vec<TxLabel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_label_maps: Option<Vec<TxLabelMap>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tags: Option<Vec<OutputTag>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tag_maps: Option<Vec<OutputTagMap>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificates: Option<Vec<Certificate>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_fields: Option<Vec<CertificateField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commissions: Option<Vec<Commission>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven_tx_reqs: Option<Vec<ProvenTxReq>>,
}
```

### TS-Fixture Test Pattern
```rust
// tests/table_tests.rs additions — verify Rust accepts TS-format JSON

#[test]
fn proven_tx_deserializes_ts_wire_format() {
    // TS Date.toISOString() always: YYYY-MM-DDTHH:MM:SS.mmmZ
    let json = r#"{
        "created_at": "2024-01-15T10:30:00.718Z",
        "updated_at": "2024-01-15T10:30:00.000Z",
        "provenTxId": 42,
        "txid": "deadbeef",
        "height": 800000,
        "index": 5,
        "merklePath": [1, 2, 3],
        "rawTx": [4, 5],
        "blockHash": "blockhash123",
        "merkleRoot": "merkleroot456"
    }"#;
    let pt: ProvenTx = serde_json::from_str(json).unwrap();
    assert_eq!(pt.proven_tx_id, 42);
    assert_eq!(pt.merkle_path, vec![1u8, 2, 3]);
    // Re-serialize and check format
    let out = serde_json::to_string(&pt).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ts = v["created_at"].as_str().unwrap();
    assert!(ts.ends_with('Z'), "serialized timestamp must end with Z, got: {}", ts);
    assert_eq!(ts, "2024-01-15T10:30:00.718Z");
}

#[test]
fn proven_tx_serialize_zero_millis_has_three_digits() {
    // Regression: %.f would produce "10:30:00" with no decimal;
    // %.3f must produce "10:30:00.000Z"
    let dt = NaiveDateTime::parse_from_str("2024-01-15T10:30:00", "%Y-%m-%dT%H:%M:%S").unwrap();
    let pt = ProvenTx { created_at: dt, updated_at: dt, /* ... */ };
    let v: serde_json::Value = serde_json::to_value(&pt).unwrap();
    let ts = v["created_at"].as_str().unwrap();
    assert!(ts.ends_with(".000Z"), "zero millis must be .000Z, got: {}", ts);
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `%.f` format (variable ms) | `%.3f` format (fixed 3 digits) | This phase | All 16 entities serialize to TS-compatible timestamps |
| No Z suffix | Append `Z` manually | This phase | TS server accepts ISO 8601 UTC strings |
| SyncChunk None -> null | SyncChunk None -> absent | This phase | SyncChunk matches TS interface where undefined means absent |

## Open Questions

1. **user field in SyncChunk — null vs absent**
   - What we know: `user: Option<User>` is a single object, not an array. TS `SyncChunk` defines `user?: TableUser`. The MergeEntity pattern only applies to arrays; user is handled separately with `if (chunk.user)`.
   - What's unclear: Should `user: None` serialize as absent or null? Both falsy in JS.
   - Recommendation: Apply `skip_serializing_if = "Option::is_none"` to `user` as well, for full consistency with the TS interface. The `if (chunk.user)` check handles both.

2. **SyncMap wire format (WIRE-03)**
   - What we know: SyncMap is stored as JSON in the `sync_states` table and also used as an internal tracking structure. The TS interface has its own `SyncMap` type.
   - What's unclear: Is SyncMap ever sent over the wire between Rust and TS, or is it only local?
   - Recommendation: SyncMap is internal state stored in `sync_states.sync_map` as a JSON string column. It is not part of the JSON-RPC wire format. The WIRE-03 requirement for SyncMap likely refers to correct serialization of SyncMap for database storage (where camelCase is already correct). No changes needed for SyncMap itself.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test (`cargo test`) |
| Config file | `Cargo.toml` — `[dev-dependencies]` tokio |
| Quick run command | `cargo test --test table_tests` |
| Full suite command | `cargo test --test table_tests --test sync_tests` (note: sync_tests has compile errors to fix) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| WIRE-01 | Timestamps serialize as `YYYY-MM-DDTHH:MM:SS.mmmZ` | unit | `cargo test --test table_tests timestamp_format` | ❌ Wave 0 |
| WIRE-01 | Zero-millisecond timestamps serialize as `.000Z` not no-decimal | unit | `cargo test --test table_tests zero_millis` | ❌ Wave 0 |
| WIRE-01 | Timestamps deserialize from TS fixture JSON (with Z) | unit | `cargo test --test table_tests ts_fixture_deserialize` | ❌ Wave 0 |
| WIRE-02 | Vec<u8> serializes as integer array | unit | `cargo test --test table_tests vec_u8_some_serializes_as_array` | ✅ already exists |
| WIRE-02 | Option<Vec<u8>> None serializes as null | unit | `cargo test --test table_tests vec_u8_optional_none_serializes_as_null` | ✅ already exists |
| WIRE-03 | SyncChunk None entity lists are absent from JSON (not null) | unit | `cargo test --test table_tests sync_chunk_absent_fields` | ❌ Wave 0 |
| WIRE-03 | SyncChunk present entity lists are present in JSON | unit | `cargo test --test table_tests sync_chunk_present_fields` | ❌ Wave 0 |
| WIRE-04 | ProvenTx round-trip with TS wire format fixture (binary + timestamp) | unit | `cargo test --test table_tests proven_tx_ts_fixture_roundtrip` | ❌ Wave 0 |
| WIRE-04 | Transaction round-trip with TS wire format fixture (optional Vec<u8>) | unit | `cargo test --test table_tests transaction_ts_fixture_roundtrip` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --test table_tests`
- **Per wave merge:** `cargo test --test table_tests`
- **Phase gate:** All table_tests green before proceeding to Phase 2

### Wave 0 Gaps
- [ ] `tests/table_tests.rs` — add timestamp format assertions (WIRE-01): tests for Z suffix and 3-digit ms
- [ ] `tests/table_tests.rs` — add SyncChunk optional-absent assertions (WIRE-03)
- [ ] `tests/table_tests.rs` — add TS-fixture round-trip tests (WIRE-04): ProvenTx and Transaction with exact TS-format JSON strings

*(No new test files or framework setup needed — `tests/table_tests.rs` already exists and passes 34 tests)*

## Sources

### Primary (HIGH confidence)
- Direct source code inspection: `/Users/elisjackson/Projects/music-streaming/key-server/node_modules/@bsv/wallet-toolbox/src/storage/schema/tables/TableProvenTx.ts` — confirmed `merklePath: number[]`, `rawTx: number[]`
- Direct source code inspection: `TableProvenTxReq.ts` — confirmed `rawTx: number[]`, `inputBEEF?: number[]`
- Direct source code inspection: `TableTransaction.ts` — confirmed `inputBEEF?: number[]`, `rawTx?: number[]`
- Direct source code inspection: `TableCommission.ts` — confirmed `lockingScript: number[]`
- Direct source code inspection: `TableOutput.ts` — confirmed `lockingScript?: number[]`
- Direct source code inspection: `StorageClient.ts` lines 520-536 — `validateEntity` converts `Uint8Array` to `Array.from(val)`, confirms integer array format
- Direct source code inspection: `StorageServer.ts` lines 226-229 — converts `Buffer.isBuffer(val)` to `Array.from(val)` before serializing, confirms integer array format
- Direct source code inspection: `WalletStorage.interfaces.ts` lines 536-554 — `SyncChunk` interface with optional (`?`) entity arrays
- Direct source code inspection: `EntitySyncState.ts` + `MergeEntity.ts` — confirms `if (!this.stateArray) return` guards against both undefined and null; absent fields are the canonical TS form
- Direct source code inspection: `src/serde_datetime.rs` — confirmed current format string `%Y-%m-%dT%H:%M:%S%.f` lacks Z and uses variable-precision ms
- Direct source code inspection: `src/storage/sync/sync_map.rs` — confirmed SyncChunk has no `skip_serializing_if`
- Test run: `cargo test --test table_tests` — 34 tests pass; existing Vec<u8> as integer array tests pass confirming WIRE-02 is already correct

### Secondary (MEDIUM confidence)
- Chrono crate docs (training knowledge, HIGH): `%.3f` = exactly 3 decimal places, `%.f` = variable precision stripping trailing zeros — standard chrono behavior verified against actual format string analysis

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all existing, no new dependencies
- Architecture: HIGH — read TS source directly, exact mismatches confirmed
- Pitfalls: HIGH — derived from direct TS source reading, not assumptions
- Test strategy: HIGH — existing test infrastructure is solid, gaps are additive

**Research date:** 2026-03-24
**Valid until:** Until TS wallet-toolbox wire format changes (stable API, unlikely to change frequently)
