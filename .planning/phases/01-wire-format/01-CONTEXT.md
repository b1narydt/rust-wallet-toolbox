# Phase 1: Wire Format - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Ensure all Rust serialization (timestamps, binary data, optional fields, entity structs) produces JSON that is byte-for-byte compatible with what the TypeScript StorageClient sends to the TypeScript StorageServer. This is the foundation for every subsequent phase — if the wire format doesn't match, nothing works.

</domain>

<decisions>
## Implementation Decisions

### Overall Approach
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

</decisions>

<specifics>
## Specific Ideas

- "The whole point of BRC-100 is that everything is interoperable" — TS, Go, and Rust clients should all talk to any server identically
- The TS StorageClient at wallet-toolbox/src/storage/remoting/StorageClient.ts is the reference for what the wire format should look like
- The TS StorageServer is the authority — match what it expects, not what we think it should expect
- If the Rust storage server also needs changes for interop, that's fine — but the TS server is the primary target

</specifics>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/serde_datetime.rs`: Custom serialize/deserialize for NaiveDateTime — currently strips trailing "Z" on deserialize but doesn't add it on serialize. May need updating.
- All 16 table structs in `src/tables/`: Already have `#[serde(rename_all = "camelCase")]` and `#[serde(with = "crate::serde_datetime")]` on timestamp fields
- `src/storage/sync/sync_map.rs`: SyncChunk and SyncMap already derive Serialize/Deserialize with camelCase renaming

### Established Patterns
- Table structs use `#[serde(rename = "created_at", alias = "createdAt")]` dual pattern for timestamps
- Binary fields (`raw_tx`, `merkle_path`, `input_beef`) are plain `Vec<u8>` — serde default serialization
- Optional fields use `Option<T>` — serde behavior (null vs absent) needs verification against TS

### Integration Points
- Fixing serde_datetime propagates automatically to all 16 entity types via the `with` attribute
- SyncChunk serialization affects the sync protocol (Phase 5 testing)
- Any changes must not break the existing SQLx integration (dual FromRow + Serialize/Deserialize)

</code_context>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 01-wire-format*
*Context gathered: 2026-03-24*
