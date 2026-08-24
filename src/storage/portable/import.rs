//! BRC-38 import (TS `importBRC38`, `restoreBRC38`, `mergeBRC38`).
//!
//! Two modes with very different trust postures:
//! - **restore**: an exact copy into an *empty* target (settings only),
//!   preserving primary keys and timestamps. Refuses non-empty targets.
//! - **merge**: feeds the document through the sync merge machinery
//!   (`process_sync_chunk`) against a live target, remapping IDs, then merges
//!   the document's own syncStates with their id-maps rewritten through the
//!   import's id-map.
//!
//! Both modes only run after full document validation (`validate_brc38`) and
//! a chain check against the target.

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::NaiveDateTime;
use rand::RngCore;
use serde_json::{Map, Value};

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::*;
use crate::storage::sync::process_sync_chunk::process_sync_chunk;
use crate::storage::sync::sync_map::{SyncChunk, SyncMap};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;
use crate::tables::*;

use super::row::from_portable_row;
use super::validate::{validate_brc38, Brc38WalletData};

/// BRC-38 import mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Merge into a live target through the sync machinery, remapping IDs.
    Merge,
    /// Exact copy into an empty target, preserving IDs.
    Restore,
}

impl std::fmt::Display for ImportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImportMode::Merge => "merge",
            ImportMode::Restore => "restore",
        })
    }
}

/// Options for [`import_brc38`] (TS `BRC38ImportOptions`).
#[derive(Debug, Clone)]
pub struct Brc38ImportOptions {
    /// The import mode.
    pub mode: ImportMode,
}

/// Result of a BRC-38 import (TS `BRC38ImportResult`).
#[derive(Debug, Clone)]
pub struct Brc38ImportResult {
    /// The mode that ran.
    pub mode: ImportMode,
    /// The imported user's identity key.
    pub identity_key: String,
    /// The user's id in the *target* storage.
    pub user_id: i64,
    /// Rows inserted.
    pub inserts: i64,
    /// Rows updated.
    pub updates: i64,
}

/// A BRC-38 document decoded into table structs (TS `DecodedBRC38`).
///
/// Fields the Rust storage schema does not model (`provenTxReq.wasBroadcast`,
/// `provenTxReq.rebroadcastAttempts` -- tracked by the storage parity audit,
/// rust-wallet-toolbox#34) are dropped here; everything else round-trips.
#[derive(Debug, Clone)]
pub struct DecodedBrc38 {
    /// The exporting storage's settings row.
    pub source_storage: Settings,
    /// The exported user.
    pub user: User,
    /// provenTxs rows.
    pub proven_txs: Vec<ProvenTx>,
    /// provenTxReqs rows.
    pub proven_tx_reqs: Vec<ProvenTxReq>,
    /// outputBaskets rows.
    pub output_baskets: Vec<OutputBasket>,
    /// transactions rows.
    pub transactions: Vec<Transaction>,
    /// commissions rows.
    pub commissions: Vec<Commission>,
    /// outputs rows.
    pub outputs: Vec<Output>,
    /// outputTags rows.
    pub output_tags: Vec<OutputTag>,
    /// outputTagMaps rows.
    pub output_tag_maps: Vec<OutputTagMap>,
    /// txLabels rows.
    pub tx_labels: Vec<TxLabel>,
    /// txLabelMaps rows.
    pub tx_label_maps: Vec<TxLabelMap>,
    /// certificates rows.
    pub certificates: Vec<Certificate>,
    /// certificateFields rows.
    pub certificate_fields: Vec<CertificateField>,
    /// syncStates rows.
    pub sync_states: Vec<SyncState>,
}

/// Storage that can serve as a BRC-38 import target.
///
/// Merge mode needs only [`StorageProvider`]. Restore mode additionally needs
/// id-preserving inserts, which the generic trait surface cannot express
/// (its inserts always autoincrement), so each backend overrides
/// `restore_brc38_rows`. Only SQLite implements it today; MySQL/Postgres
/// fall back to a clear error.
#[async_trait]
pub trait PortableStorage: StorageProvider {
    /// Insert every decoded row with its original primary key, inside `trx`.
    /// The target is guaranteed empty (checked by the caller).
    async fn restore_brc38_rows(
        &self,
        _decoded: &DecodedBrc38,
        _trx: &TrxToken,
    ) -> WalletResult<()> {
        Err(WalletError::NotImplemented(
            "BRC-38 restore requires id-preserving inserts, which this storage backend does not implement yet (merge mode is available)"
                .to_string(),
        ))
    }
}

#[cfg(feature = "mysql")]
impl PortableStorage for crate::storage::sqlx_impl::MysqlStorage {}
#[cfg(feature = "postgres")]
impl PortableStorage for crate::storage::sqlx_impl::PgStorage {}

/// Parse and validate a BRC-38 JSON string (TS `parseBRC38Json`).
pub fn parse_brc38_json(json: &str) -> WalletResult<Brc38WalletData> {
    let value: Value = serde_json::from_str(json)
        .map_err(|e| WalletError::BadRequest(format!("Invalid BRC-38 JSON: {e}")))?;
    validate_brc38(value)
}

/// Import a validated BRC-38 document (TS `importBRC38`).
pub async fn import_brc38<S: PortableStorage>(
    storage: &S,
    data: &Brc38WalletData,
    options: &Brc38ImportOptions,
) -> WalletResult<Brc38ImportResult> {
    let target_settings = storage.make_available().await?;
    let decoded = decode_brc38(data)?;
    if decoded.source_storage.chain != target_settings.chain {
        return Err(WalletError::BadRequest(format!(
            "BRC-38 chain mismatch: payload is {}, target is {}",
            decoded.source_storage.chain, target_settings.chain
        )));
    }
    match options.mode {
        ImportMode::Restore => restore_brc38(storage, &decoded).await,
        ImportMode::Merge => merge_brc38(storage, &decoded, &target_settings).await,
    }
}

/// Decode the document's rows into table structs (TS `decodeBRC38`).
fn decode_brc38(data: &Brc38WalletData) -> WalletResult<DecodedBrc38> {
    fn table<T: serde::de::DeserializeOwned>(
        data: &Brc38WalletData,
        kind: &str,
        name: &str,
    ) -> WalletResult<Vec<T>> {
        data.table(name)
            .into_iter()
            .map(|row| from_portable_row(kind, row))
            .collect()
    }
    Ok(DecodedBrc38 {
        source_storage: from_portable_row("settings", data.source_storage())?,
        user: from_portable_row("user", data.user())?,
        proven_txs: table(data, "provenTx", "provenTxs")?,
        proven_tx_reqs: table(data, "provenTxReq", "provenTxReqs")?,
        output_baskets: table(data, "outputBasket", "outputBaskets")?,
        transactions: table(data, "transaction", "transactions")?,
        commissions: table(data, "commission", "commissions")?,
        outputs: table(data, "output", "outputs")?,
        output_tags: table(data, "outputTag", "outputTags")?,
        output_tag_maps: table(data, "outputTagMap", "outputTagMaps")?,
        tx_labels: table(data, "txLabel", "txLabels")?,
        tx_label_maps: table(data, "txLabelMap", "txLabelMaps")?,
        certificates: table(data, "certificate", "certificates")?,
        certificate_fields: table(data, "certificateField", "certificateFields")?,
        sync_states: table(data, "syncState", "syncStates")?,
    })
}

/// Restore into an empty target, preserving IDs (TS `restoreBRC38`).
async fn restore_brc38<S: PortableStorage>(
    storage: &S,
    decoded: &DecodedBrc38,
) -> WalletResult<Brc38ImportResult> {
    assert_restore_target_empty(storage).await?;
    let trx = storage.begin_transaction().await?;
    match storage.restore_brc38_rows(decoded, &trx).await {
        Ok(()) => storage.commit_transaction(trx).await?,
        Err(e) => return Err(fail_with_rollback(storage, trx, "restore", e).await),
    }
    Ok(Brc38ImportResult {
        mode: ImportMode::Restore,
        identity_key: decoded.user.identity_key.clone(),
        user_id: decoded.user.user_id,
        inserts: 1 + count_decoded_rows(decoded),
        updates: 0,
    })
}

/// TS `assertRestoreTargetEmpty`: every table (including monitor events)
/// must be empty; only settings may exist.
async fn assert_restore_target_empty<S: StorageProvider + ?Sized>(storage: &S) -> WalletResult<()> {
    let counts = [
        storage.count_users(&Default::default(), None).await?,
        storage.count_proven_txs(&Default::default(), None).await?,
        storage
            .count_proven_tx_reqs(&Default::default(), None)
            .await?,
        storage
            .count_output_baskets(&Default::default(), None)
            .await?,
        storage
            .count_transactions(&Default::default(), None)
            .await?,
        storage.count_commissions(&Default::default(), None).await?,
        storage.count_outputs(&Default::default(), None).await?,
        storage.count_output_tags(&Default::default(), None).await?,
        storage
            .count_output_tag_maps(&Default::default(), None)
            .await?,
        storage.count_tx_labels(&Default::default(), None).await?,
        storage
            .count_tx_label_maps(&Default::default(), None)
            .await?,
        storage
            .count_certificates(&Default::default(), None)
            .await?,
        storage
            .count_certificate_fields(&Default::default(), None)
            .await?,
        storage.count_sync_states(&Default::default(), None).await?,
        storage
            .count_monitor_events(&Default::default(), None)
            .await?,
    ];
    if counts.iter().any(|c| *c > 0) {
        return Err(WalletError::BadRequest(
            "BRC-38 restore requires an empty target storage except settings".to_string(),
        ));
    }
    Ok(())
}

fn count_decoded_rows(d: &DecodedBrc38) -> i64 {
    (d.proven_txs.len()
        + d.proven_tx_reqs.len()
        + d.output_baskets.len()
        + d.transactions.len()
        + d.commissions.len()
        + d.outputs.len()
        + d.output_tags.len()
        + d.output_tag_maps.len()
        + d.tx_labels.len()
        + d.tx_label_maps.len()
        + d.certificates.len()
        + d.certificate_fields.len()
        + d.sync_states.len()) as i64
}

/// Merge into a live target through the sync machinery (TS `mergeBRC38`).
/// Merge runs in ONE transaction, like restore.
///
/// It previously did not: every write passed `None` for the transaction token
/// and autocommitted, so a failure part-way left the target PARTIALLY MERGED —
/// no rollback, no marker, and no `Brc38ImportResult`, so the caller could not
/// learn how far it got. That is the dangerous shape, because unlike restore
/// (which refuses a non-empty target) merge runs against a store that already
/// holds data, so there was nothing to distinguish a half-merged wallet from a
/// correctly merged one.
///
/// The ordering made it worse: the id-map was persisted AFTER the rows it
/// describes, so a crash between them left a map that `normalize_sync_map`
/// would later read as empty — silently re-inserting rows it should have
/// matched.
async fn merge_brc38<S: StorageProvider>(
    storage: &S,
    decoded: &DecodedBrc38,
    target_settings: &Settings,
) -> WalletResult<Brc38ImportResult> {
    let trx = storage.begin_transaction().await?;
    match merge_brc38_in_trx(storage, decoded, target_settings, &trx).await {
        Ok(result) => {
            storage.commit_transaction(trx).await?;
            Ok(result)
        }
        Err(e) => Err(fail_with_rollback(storage, trx, "merge", e).await),
    }
}

/// Roll back `trx` after `cause` failed the import, keeping `cause` as the
/// primary error. If the rollback itself also fails, the target's state is
/// unknown, so both errors surface in the returned error — the rollback
/// failure must neither be dropped nor mask the reason for rolling back.
async fn fail_with_rollback<S: StorageProvider + ?Sized>(
    storage: &S,
    trx: TrxToken,
    context: &str,
    cause: WalletError,
) -> WalletError {
    match storage.rollback_transaction(trx).await {
        Ok(()) => cause,
        Err(rollback_err) => WalletError::Internal(format!(
            "BRC-38 {context} failed: {cause}; rollback ALSO failed: {rollback_err} — the \
             target may be partially written; inspect it before retrying"
        )),
    }
}

async fn merge_brc38_in_trx<S: StorageProvider>(
    storage: &S,
    decoded: &DecodedBrc38,
    target_settings: &Settings,
    trx: &TrxToken,
) -> WalletResult<Brc38ImportResult> {
    let (target_user, _) = storage
        .find_or_insert_user(&decoded.user.identity_key, Some(trx))
        .await?;

    // Find or create the sync state row tracking imports from the source
    // storage (TS findOrInsertSyncStateAuth).
    let import_state = find_or_insert_sync_state(
        storage,
        target_user.user_id,
        &decoded.source_storage.storage_identity_key,
        &decoded.source_storage.storage_name,
        Some(trx),
    )
    .await?;

    let mut user = decoded.user.clone();
    user.active_storage = target_user.active_storage.clone();
    let chunk = SyncChunk {
        from_storage_identity_key: decoded.source_storage.storage_identity_key.clone(),
        to_storage_identity_key: target_settings.storage_identity_key.clone(),
        user_identity_key: decoded.user.identity_key.clone(),
        user: Some(user),
        proven_txs: Some(decoded.proven_txs.clone()),
        output_baskets: Some(decoded.output_baskets.clone()),
        output_tags: Some(decoded.output_tags.clone()),
        tx_labels: Some(decoded.tx_labels.clone()),
        transactions: Some(decoded.transactions.clone()),
        outputs: Some(decoded.outputs.clone()),
        tx_label_maps: Some(decoded.tx_label_maps.clone()),
        output_tag_maps: Some(decoded.output_tag_maps.clone()),
        certificates: Some(decoded.certificates.clone()),
        certificate_fields: Some(decoded.certificate_fields.clone()),
        commissions: Some(decoded.commissions.clone()),
        proven_tx_reqs: Some(decoded.proven_tx_reqs.clone()),
    };

    let mut import_map = normalize_sync_map(&parse_sync_map_json(
        &import_state.sync_map,
        "the import-tracking sync state",
    )?);
    let chunk_result = process_sync_chunk(
        storage as &dyn StorageProvider,
        &target_user.identity_key,
        chunk,
        &mut import_map,
        Some(trx),
    )
    .await?;

    // Persist the updated id-maps with counts reset and `when` advanced to
    // the latest updated_at merged, as TS does once a sync round completes
    // (EntitySyncState.processSyncChunk done-handling: `this.when =
    // maxUpdated_at` over the entity maps). `when` is the `since` a later
    // sync round resumes from, so it must reflect what this import already
    // transferred.
    for esm in import_map.entity_maps_mut() {
        esm.count = 0;
    }
    let when = import_map
        .entity_maps()
        .iter()
        .filter_map(|esm| esm.max_updated_at)
        .max();
    storage
        .update_sync_state(
            import_state.sync_state_id,
            &SyncStatePartial {
                sync_map: Some(sync_map_to_ts_json(&import_map)),
                when,
                ..Default::default()
            },
            Some(trx),
        )
        .await?;

    let sync_state_result = merge_imported_sync_states(
        storage,
        &decoded.sync_states,
        target_user.user_id,
        &import_map,
        &decoded.source_storage,
        Some(trx),
    )
    .await?;

    Ok(Brc38ImportResult {
        mode: ImportMode::Merge,
        identity_key: decoded.user.identity_key.clone(),
        user_id: target_user.user_id,
        inserts: chunk_result.inserts + sync_state_result.0,
        updates: chunk_result.updates + sync_state_result.1,
    })
}

async fn find_or_insert_sync_state<S: StorageProvider + ?Sized>(
    storage: &S,
    user_id: i64,
    storage_identity_key: &str,
    storage_name: &str,
    trx: Option<&TrxToken>,
) -> WalletResult<SyncState> {
    let existing = storage
        .find_sync_states(
            &FindSyncStatesArgs {
                partial: SyncStatePartial {
                    user_id: Some(user_id),
                    storage_identity_key: Some(storage_identity_key.to_string()),
                    storage_name: Some(storage_name.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;
    if existing.len() > 1 {
        return Err(WalletError::Internal(
            "multiple sync states for one (user, storage) pair".to_string(),
        ));
    }
    if let Some(state) = existing.into_iter().next() {
        return Ok(state);
    }
    let now = chrono::Utc::now().naive_utc();
    let mut ref_num_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut ref_num_bytes);
    let mut state = SyncState {
        created_at: now,
        updated_at: now,
        sync_state_id: 0,
        user_id,
        storage_identity_key: storage_identity_key.to_string(),
        storage_name: storage_name.to_string(),
        status: crate::status::SyncStatus::Unknown,
        init: false,
        ref_num: BASE64.encode(ref_num_bytes),
        sync_map: sync_map_to_ts_json(&SyncMap::new()),
        when: None,
        satoshis: None,
        error_local: None,
        error_other: None,
    };
    state.sync_state_id = storage.insert_sync_state(&state, trx).await?;
    Ok(state)
}

/// Merge the document's own syncStates rows into the target
/// (TS `mergeImportedSyncStates`): each imported sync map's local-id values
/// are rewritten through the import id-map before landing.
async fn merge_imported_sync_states<S: StorageProvider + ?Sized>(
    storage: &S,
    sync_states: &[SyncState],
    user_id: i64,
    import_map: &SyncMap,
    source_storage: &Settings,
    trx: Option<&TrxToken>,
) -> WalletResult<(i64, i64)> {
    let mut inserts = 0;
    let mut updates = 0;
    for source in sync_states {
        let remapped = remap_sync_map(
            &parse_sync_map_json(&source.sync_map, "an imported syncStates row")?,
            import_map,
        );
        let mut row = source.clone();
        row.user_id = user_id;
        row.sync_map = sync_map_to_ts_json(&remapped);

        let existing = storage
            .find_sync_states(
                &FindSyncStatesArgs {
                    partial: SyncStatePartial {
                        user_id: Some(user_id),
                        storage_identity_key: Some(row.storage_identity_key.clone()),
                        storage_name: Some(row.storage_name.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trx,
            )
            .await?;
        if existing.len() > 1 {
            return Err(WalletError::Internal(
                "multiple sync states for one (user, storage) pair".to_string(),
            ));
        }
        match existing.into_iter().next() {
            None => {
                row.sync_state_id = 0;
                storage.insert_sync_state(&row, trx).await?;
                inserts += 1;
            }
            Some(existing) => {
                // The import-tracking row for the source storage keeps the
                // id-map just built by the merge; the imported copy is stale.
                let sync_map = if row.storage_identity_key == source_storage.storage_identity_key
                    && row.storage_name == source_storage.storage_name
                {
                    existing.sync_map.clone()
                } else {
                    row.sync_map.clone()
                };
                storage
                    .update_sync_state(
                        existing.sync_state_id,
                        &SyncStatePartial {
                            status: Some(row.status),
                            init: Some(row.init),
                            sync_map: Some(sync_map),
                            when: row.when,
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                updates += 1;
            }
        }
    }
    Ok((inserts, updates))
}

/// Rewrite a sync map's local-id values through the import id-map
/// (TS `remapSyncMap`). Map tables (`txLabelMap`, `outputTagMap`) have no
/// ids of their own and are not remapped, matching TS.
fn remap_sync_map(source: &Value, import_map: &SyncMap) -> SyncMap {
    let mut copy = normalize_sync_map(source);
    for (entity, import_entity) in [
        (&mut copy.proven_tx, &import_map.proven_tx),
        (&mut copy.output_basket, &import_map.output_basket),
        (&mut copy.transaction, &import_map.transaction),
        (&mut copy.proven_tx_req, &import_map.proven_tx_req),
        (&mut copy.tx_label, &import_map.tx_label),
        (&mut copy.output, &import_map.output),
        (&mut copy.output_tag, &import_map.output_tag),
        (&mut copy.certificate, &import_map.certificate),
        (&mut copy.commission, &import_map.commission),
    ] {
        for local_id in entity.id_map.values_mut() {
            if let Some(target_id) = import_entity.id_map.get(local_id) {
                *local_id = *target_id;
            }
        }
    }
    copy
}

/// Parse a stored sync map JSON string, failing the import on corruption.
///
/// TS calls bare `JSON.parse` at both call sites, so unparseable JSON aborts
/// the import there too. Shape leniency stays in `normalize_sync_map`; only
/// JSON that does not parse at all is fatal. Both call sites run inside the
/// merge transaction, so failing rolls back cleanly — whereas substituting an
/// empty map would overwrite the stored id-map and erase the record of what
/// was already synced, silently duplicating rows on the next round.
fn parse_sync_map_json(json: &str, context: &str) -> WalletResult<Value> {
    serde_json::from_str::<Value>(json).map_err(|e| {
        WalletError::Internal(format!(
            "BRC-38 merge: corrupt syncMap JSON in {context}: {e}"
        ))
    })
}

/// Lenient sync map parsing (TS `normalizeSyncMap`): start from a fresh map
/// and copy over only well-formed pieces of the incoming JSON.
fn normalize_sync_map(source: &Value) -> SyncMap {
    let mut normalized = SyncMap::new();
    let map = match source.as_object() {
        Some(map) => map,
        None => return normalized,
    };
    for (key, entity) in [
        ("provenTx", &mut normalized.proven_tx),
        ("outputBasket", &mut normalized.output_basket),
        ("transaction", &mut normalized.transaction),
        ("output", &mut normalized.output),
        ("txLabel", &mut normalized.tx_label),
        ("txLabelMap", &mut normalized.tx_label_map),
        ("outputTag", &mut normalized.output_tag),
        ("outputTagMap", &mut normalized.output_tag_map),
        ("certificate", &mut normalized.certificate),
        ("certificateField", &mut normalized.certificate_field),
        ("commission", &mut normalized.commission),
        ("provenTxReq", &mut normalized.proven_tx_req),
    ] {
        if let Some(incoming) = map.get(key).and_then(Value::as_object) {
            merge_sync_map_entry(entity, incoming);
        }
    }
    normalized
}

fn merge_sync_map_entry(
    target: &mut crate::storage::sync::sync_map::EntitySyncMap,
    incoming: &Map<String, Value>,
) {
    if let Some(name) = incoming.get("entityName").and_then(Value::as_str) {
        target.entity_name = name.to_string();
    }
    if let Some(count) = incoming.get("count").and_then(Value::as_i64) {
        target.count = count;
    }
    if let Some(id_map) = incoming.get("idMap").and_then(Value::as_object) {
        target.id_map = id_map
            .iter()
            .filter_map(|(remote, local)| Some((remote.parse::<i64>().ok()?, local.as_i64()?)))
            .collect();
    }
    if let Some(when) = incoming.get("maxUpdated_at").and_then(Value::as_str) {
        if let Some(parsed) = parse_js_date(when) {
            target.max_updated_at = Some(parsed);
        }
    }
}

fn parse_js_date(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f").ok()
}

/// Serialize a sync map in the TS on-disk shape: `maxUpdated_at` key (ISO
/// string, omitted when unset), so a wallet merged by Rust and later exported
/// by TS reads its own sync maps.
fn sync_map_to_ts_json(map: &SyncMap) -> String {
    fn entity(esm: &crate::storage::sync::sync_map::EntitySyncMap) -> Value {
        let mut out = Map::new();
        out.insert(
            "idMap".to_string(),
            Value::Object(
                esm.id_map
                    .iter()
                    .map(|(remote, local)| (remote.to_string(), Value::from(*local)))
                    .collect(),
            ),
        );
        out.insert(
            "entityName".to_string(),
            Value::String(esm.entity_name.clone()),
        );
        if let Some(at) = &esm.max_updated_at {
            out.insert(
                "maxUpdated_at".to_string(),
                Value::String(super::canonical::iso_date(at)),
            );
        }
        out.insert("count".to_string(), Value::from(esm.count));
        Value::Object(out)
    }
    let mut out = Map::new();
    out.insert("provenTx".to_string(), entity(&map.proven_tx));
    out.insert("outputBasket".to_string(), entity(&map.output_basket));
    out.insert("transaction".to_string(), entity(&map.transaction));
    out.insert("provenTxReq".to_string(), entity(&map.proven_tx_req));
    out.insert("txLabel".to_string(), entity(&map.tx_label));
    out.insert("txLabelMap".to_string(), entity(&map.tx_label_map));
    out.insert("output".to_string(), entity(&map.output));
    out.insert("outputTag".to_string(), entity(&map.output_tag));
    out.insert("outputTagMap".to_string(), entity(&map.output_tag_map));
    out.insert("certificate".to_string(), entity(&map.certificate));
    out.insert(
        "certificateField".to_string(),
        entity(&map.certificate_field),
    );
    out.insert("commission".to_string(), entity(&map.commission));
    serde_json::to_string(&Value::Object(out)).expect("sync map serialization is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sync_map_json_accepts_valid_json() {
        assert!(parse_sync_map_json("{}", "test").is_ok());
        // Wrong shape is tolerated here; normalize_sync_map handles leniency.
        assert!(parse_sync_map_json("null", "test").is_ok());
        assert!(parse_sync_map_json("[1,2]", "test").is_ok());
    }

    #[test]
    fn parse_sync_map_json_fails_on_corrupt_json() {
        let err = parse_sync_map_json("not-json{{{", "the import-tracking sync state")
            .unwrap_err()
            .to_string();
        assert!(err.contains("corrupt syncMap JSON"), "{err}");
        assert!(err.contains("the import-tracking sync state"), "{err}");
    }

    #[cfg(feature = "sqlite")]
    mod rollback {
        use crate::storage::sqlx_impl::{SqliteStorage, SqliteTrxInner};
        use crate::storage::traits::reader_writer::StorageReaderWriter;
        use crate::storage::StorageConfig;
        use crate::types::Chain;

        use super::*;

        async fn storage() -> SqliteStorage {
            let config = StorageConfig {
                url: "sqlite::memory:".to_string(),
                ..Default::default()
            };
            SqliteStorage::new_sqlite(config, Chain::Test)
                .await
                .unwrap()
        }

        #[tokio::test]
        async fn fail_with_rollback_keeps_cause_when_rollback_succeeds() {
            let storage = storage().await;
            let trx = storage.begin_transaction().await.unwrap();
            let cause = WalletError::BadRequest("original failure".to_string());
            let err = fail_with_rollback(&storage, trx, "restore", cause).await;
            assert!(
                matches!(&err, WalletError::BadRequest(m) if m == "original failure"),
                "cause must pass through unchanged: {err}"
            );
        }

        #[tokio::test]
        async fn fail_with_rollback_surfaces_both_errors_when_rollback_fails() {
            let storage = storage().await;
            // A token whose transaction is already consumed makes
            // rollback_transaction fail deterministically.
            let consumed: SqliteTrxInner = std::sync::Arc::new(tokio::sync::Mutex::new(None));
            let trx = TrxToken::new(consumed);
            let cause = WalletError::BadRequest("original failure".to_string());
            let err = fail_with_rollback(&storage, trx, "restore", cause)
                .await
                .to_string();
            assert!(err.contains("original failure"), "{err}");
            assert!(err.contains("rollback ALSO failed"), "{err}");
            assert!(err.contains("Transaction already consumed"), "{err}");
        }
    }
}
