//! Sync types: SyncChunk, SyncMap, EntitySyncMap.
//!
//! These types drive the incremental sync protocol between storage providers.
//! SyncMap is stored as JSON in the sync_states table for persistent tracking.
//! SyncChunk is the wire format for sending incremental entity updates.

use std::collections::HashMap;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::tables::*;

/// An incremental chunk of entities that changed since the last sync.
///
/// Sent from one storage provider to another during synchronization.
/// Each entity list is optional -- only populated entity types are included.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncChunk {
    /// Identity key of the source storage.
    pub from_storage_identity_key: String,
    /// Identity key of the destination storage.
    pub to_storage_identity_key: String,
    /// Identity key of the user being synced.
    pub user_identity_key: String,
    /// User record, if changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    /// Changed proven transaction records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven_txs: Option<Vec<ProvenTx>>,
    /// Changed output basket records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_baskets: Option<Vec<OutputBasket>>,
    /// Changed transaction records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transactions: Option<Vec<Transaction>>,
    /// Changed output records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Output>>,
    /// Changed transaction label records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_labels: Option<Vec<TxLabel>>,
    /// Changed transaction label mapping records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_label_maps: Option<Vec<TxLabelMap>>,
    /// Changed output tag records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tags: Option<Vec<OutputTag>>,
    /// Changed output tag mapping records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tag_maps: Option<Vec<OutputTagMap>>,
    /// Changed certificate records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificates: Option<Vec<Certificate>>,
    /// Changed certificate field records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_fields: Option<Vec<CertificateField>>,
    /// Changed commission records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commissions: Option<Vec<Commission>>,
    /// Changed proven transaction request records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proven_tx_reqs: Option<Vec<ProvenTxReq>>,
}

/// Per-entity sync tracking: ID mappings, max timestamp, and cumulative count.
///
/// `id_map` maps foreign IDs to local IDs so that foreign key references
/// in dependent entities can be remapped during processSyncChunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitySyncMap {
    /// Name of the entity type (e.g., "provenTx", "transaction").
    pub entity_name: String,
    /// Maps foreign entity IDs to local entity IDs.
    pub id_map: HashMap<i64, i64>,
    /// The maximum updated_at value seen for this entity over all chunks received.
    #[serde(default, with = "crate::serde_datetime::option")]
    pub max_updated_at: Option<NaiveDateTime>,
    /// Cumulative count of items received since the last `since` update.
    pub count: i64,
}

impl EntitySyncMap {
    /// Create a new EntitySyncMap for the given entity name.
    pub fn new(entity_name: &str) -> Self {
        Self {
            entity_name: entity_name.to_string(),
            id_map: HashMap::new(),
            max_updated_at: None,
            count: 0,
        }
    }

    /// Update the max_updated_at to the greater of current and incoming.
    pub fn update_max(&mut self, updated_at: NaiveDateTime) {
        match self.max_updated_at {
            Some(current) if current >= updated_at => {}
            _ => {
                self.max_updated_at = Some(updated_at);
            }
        }
    }

    /// Record a foreign-to-local ID mapping. Returns an error if a conflicting
    /// mapping already exists.
    pub fn map_id(&mut self, foreign_id: i64, local_id: i64) -> Result<(), String> {
        if let Some(existing) = self.id_map.get(&foreign_id) {
            if *existing != local_id {
                return Err(format!(
                    "EntitySyncMap[{}]: cannot override mapping {}=>{} with {}",
                    self.entity_name, foreign_id, existing, local_id
                ));
            }
        }
        self.id_map.insert(foreign_id, local_id);
        Ok(())
    }

    /// Look up the local ID for a foreign ID. Returns None if not mapped.
    pub fn get_local_id(&self, foreign_id: i64) -> Option<i64> {
        self.id_map.get(&foreign_id).copied()
    }
}

/// Tracks per-entity sync state for all syncable entity types.
///
/// One SyncMap exists per storage-pair sync relationship. It is serialized
/// to JSON and stored in the sync_states table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncMap {
    /// Sync state for proven transactions.
    pub proven_tx: EntitySyncMap,
    /// Sync state for output baskets.
    pub output_basket: EntitySyncMap,
    /// Sync state for transactions.
    pub transaction: EntitySyncMap,
    /// Sync state for outputs.
    pub output: EntitySyncMap,
    /// Sync state for transaction labels.
    pub tx_label: EntitySyncMap,
    /// Sync state for transaction label mappings.
    pub tx_label_map: EntitySyncMap,
    /// Sync state for output tags.
    pub output_tag: EntitySyncMap,
    /// Sync state for output tag mappings.
    pub output_tag_map: EntitySyncMap,
    /// Sync state for certificates.
    pub certificate: EntitySyncMap,
    /// Sync state for certificate fields.
    pub certificate_field: EntitySyncMap,
    /// Sync state for commissions.
    pub commission: EntitySyncMap,
    /// Sync state for proven transaction requests.
    pub proven_tx_req: EntitySyncMap,
}

impl SyncMap {
    /// Create a new SyncMap with all EntitySyncMaps initialized to empty state.
    pub fn new() -> Self {
        Self {
            proven_tx: EntitySyncMap::new("provenTx"),
            output_basket: EntitySyncMap::new("outputBasket"),
            transaction: EntitySyncMap::new("transaction"),
            output: EntitySyncMap::new("output"),
            tx_label: EntitySyncMap::new("txLabel"),
            tx_label_map: EntitySyncMap::new("txLabelMap"),
            output_tag: EntitySyncMap::new("outputTag"),
            output_tag_map: EntitySyncMap::new("outputTagMap"),
            certificate: EntitySyncMap::new("certificate"),
            certificate_field: EntitySyncMap::new("certificateField"),
            commission: EntitySyncMap::new("commission"),
            proven_tx_req: EntitySyncMap::new("provenTxReq"),
        }
    }
}

impl Default for SyncMap {
    fn default() -> Self {
        Self::new()
    }
}
