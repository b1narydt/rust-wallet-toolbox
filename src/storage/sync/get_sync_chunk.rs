//! getSyncChunk implementation -- extracts changed entities since last sync.
//!
//! Queries each entity type from storage filtered by the max_updated_at
//! timestamp in the SyncMap, collecting up to max_items_per_entity items.
//! This implements the incremental sync protocol's "pull" side.

use std::collections::HashSet;

use crate::error::WalletResult;
use crate::storage::find_args::*;
use crate::storage::sync::sync_map::{SyncChunk, SyncMap};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

/// Per-entity row offsets for sync chunk pagination.
///
/// When a sync chunk can't return all changed entities in one call,
/// the client re-sends with incremented offsets to paginate through
/// the remaining rows at the same `since` timestamp.
#[derive(Debug, Default, Clone)]
pub struct SyncChunkOffsets {
    pub proven_tx: i64,
    pub output_basket: i64,
    pub output_tag: i64,
    pub tx_label: i64,
    pub transaction: i64,
    pub output: i64,
    pub tx_label_map: i64,
    pub output_tag_map: i64,
    pub certificate: i64,
    pub certificate_field: i64,
    pub commission: i64,
    pub proven_tx_req: i64,
}

/// Arguments for getSyncChunk.
pub struct GetSyncChunkArgs<'a> {
    /// Identity key of the storage being read from.
    pub from_storage_identity_key: String,
    /// Identity key of the storage being synced to.
    pub to_storage_identity_key: String,
    /// The user's identity key.
    pub user_identity_key: String,
    /// Current sync state with per-entity max_updated_at timestamps.
    pub sync_map: &'a SyncMap,
    /// Maximum number of items to return per entity type. Defaults to 1000.
    pub max_items_per_entity: i64,
    /// Per-entity row offsets for pagination within a sync window.
    pub offsets: SyncChunkOffsets,
}

/// Get the next incremental sync chunk of entities that changed since the last sync.
///
/// For each entity type, queries storage using the sync_map's max_updated_at
/// as the `since` filter, limited to max_items_per_entity results.
pub async fn get_sync_chunk(
    storage: &dyn StorageProvider,
    args: GetSyncChunkArgs<'_>,
    trx: Option<&TrxToken>,
) -> WalletResult<SyncChunk> {
    let max = args.max_items_per_entity;

    // Look up the user to get their user_id
    let user = storage
        .find_user_by_identity_key(&args.user_identity_key, trx)
        .await?;

    let user_id = match &user {
        Some(u) => u.user_id,
        None => {
            // No user found -- return empty chunk
            return Ok(SyncChunk {
                from_storage_identity_key: args.from_storage_identity_key,
                to_storage_identity_key: args.to_storage_identity_key,
                user_identity_key: args.user_identity_key,
                user: None,
                proven_txs: None,
                output_baskets: None,
                transactions: None,
                outputs: None,
                tx_labels: None,
                tx_label_maps: None,
                output_tags: None,
                output_tag_maps: None,
                certificates: None,
                certificate_fields: None,
                commissions: None,
                proven_tx_reqs: None,
            });
        }
    };

    // Include user if updated since last sync
    let sync_user = match &user {
        Some(u) => {
            let dominated = args
                .sync_map
                .proven_tx
                .max_updated_at
                .map_or(false, |since| u.updated_at <= since);
            if dominated {
                None
            } else {
                Some(u.clone())
            }
        }
        None => None,
    };

    let paged = |limit: i64, offset: i64| Some(Paged { limit, offset });

    // ProvenTx
    let proven_txs = {
        let items = storage
            .get_proven_txs_for_user(
                &FindForUserSincePagedArgs {
                    user_id,
                    since: args.sync_map.proven_tx.max_updated_at,
                    paged: paged(max, args.offsets.proven_tx),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // OutputBasket
    let output_baskets = {
        let items = storage
            .find_output_baskets(
                &FindOutputBasketsArgs {
                    partial: OutputBasketPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.output_basket.max_updated_at,
                    paged: paged(max, args.offsets.output_basket),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // OutputTag
    let output_tags = {
        let items = storage
            .find_output_tags(
                &FindOutputTagsArgs {
                    partial: OutputTagPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.output_tag.max_updated_at,
                    paged: paged(max, args.offsets.output_tag),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // TxLabel
    let tx_labels = {
        let items = storage
            .find_tx_labels(
                &FindTxLabelsArgs {
                    partial: TxLabelPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.tx_label.max_updated_at,
                    paged: paged(max, args.offsets.tx_label),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // Transaction
    let transactions = {
        let items = storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.transaction.max_updated_at,
                    paged: paged(max, args.offsets.transaction),
                    ..Default::default()
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // Output
    let outputs = {
        let items = storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.output.max_updated_at,
                    paged: paged(max, args.offsets.output),
                    ..Default::default()
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // TxLabelMap
    let tx_label_maps = {
        let items = storage
            .get_tx_label_maps_for_user(
                &FindForUserSincePagedArgs {
                    user_id,
                    since: args.sync_map.tx_label_map.max_updated_at,
                    paged: paged(max, args.offsets.tx_label_map),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // OutputTagMap
    let output_tag_maps = {
        let items = storage
            .get_output_tag_maps_for_user(
                &FindForUserSincePagedArgs {
                    user_id,
                    since: args.sync_map.output_tag_map.max_updated_at,
                    paged: paged(max, args.offsets.output_tag_map),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // Certificate
    let certificates = {
        let items = storage
            .find_certificates(
                &FindCertificatesArgs {
                    partial: CertificatePartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.certificate.max_updated_at,
                    paged: paged(max, args.offsets.certificate),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // CertificateField
    let certificate_fields = {
        let items = storage
            .find_certificate_fields(
                &FindCertificateFieldsArgs {
                    partial: CertificateFieldPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.certificate_field.max_updated_at,
                    paged: paged(max, args.offsets.certificate_field),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // Commission
    let commissions = {
        let items = storage
            .find_commissions(
                &FindCommissionsArgs {
                    partial: CommissionPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    since: args.sync_map.commission.max_updated_at,
                    paged: paged(max, args.offsets.commission),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // ProvenTxReq
    let proven_tx_reqs = {
        let items = storage
            .get_proven_tx_reqs_for_user(
                &FindForUserSincePagedArgs {
                    user_id,
                    since: args.sync_map.proven_tx_req.max_updated_at,
                    paged: paged(max, args.offsets.proven_tx_req),
                },
                trx,
            )
            .await?;
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    };

    // -----------------------------------------------------------------------
    // Filter dependent entities so they only reference parent IDs in this chunk.
    // When max_items_per_entity limits a parent entity list, dependent entities
    // that reference parents outside this chunk would cause undefined ID lookups
    // during processSyncChunk on the receiver.
    // -----------------------------------------------------------------------

    let output_ids: HashSet<i64> = outputs
        .as_ref()
        .map(|v| v.iter().map(|o| o.output_id).collect())
        .unwrap_or_default();
    let transaction_ids: HashSet<i64> = transactions
        .as_ref()
        .map(|v| v.iter().map(|t| t.transaction_id).collect())
        .unwrap_or_default();
    let tx_label_ids: HashSet<i64> = tx_labels
        .as_ref()
        .map(|v| v.iter().map(|t| t.tx_label_id).collect())
        .unwrap_or_default();
    let output_tag_ids: HashSet<i64> = output_tags
        .as_ref()
        .map(|v| v.iter().map(|t| t.output_tag_id).collect())
        .unwrap_or_default();
    let certificate_ids: HashSet<i64> = certificates
        .as_ref()
        .map(|v| v.iter().map(|c| c.certificate_id).collect())
        .unwrap_or_default();
    let proven_tx_ids: HashSet<i64> = proven_txs
        .as_ref()
        .map(|v| v.iter().map(|p| p.proven_tx_id).collect())
        .unwrap_or_default();

    // Filter outputTagMaps: must reference outputs AND outputTags in this chunk
    let output_tag_maps = output_tag_maps.map(|v| {
        v.into_iter()
            .filter(|m| {
                output_ids.contains(&m.output_id) && output_tag_ids.contains(&m.output_tag_id)
            })
            .collect::<Vec<_>>()
    });

    // Filter txLabelMaps: must reference transactions AND txLabels in this chunk
    let tx_label_maps = tx_label_maps.map(|v| {
        v.into_iter()
            .filter(|m| {
                transaction_ids.contains(&m.transaction_id) && tx_label_ids.contains(&m.tx_label_id)
            })
            .collect::<Vec<_>>()
    });

    // Filter certificateFields: must reference certificates in this chunk
    let certificate_fields = certificate_fields.map(|v| {
        v.into_iter()
            .filter(|f| certificate_ids.contains(&f.certificate_id))
            .collect::<Vec<_>>()
    });

    // Filter commissions: must reference transactions in this chunk
    let commissions = commissions.map(|v| {
        v.into_iter()
            .filter(|c| transaction_ids.contains(&c.transaction_id))
            .collect::<Vec<_>>()
    });

    // Filter provenTxReqs: must reference provenTxs in this chunk (if they have one)
    let proven_tx_reqs = proven_tx_reqs.map(|v| {
        v.into_iter()
            .filter(|r| match r.proven_tx_id {
                Some(id) => proven_tx_ids.contains(&id),
                None => true, // no FK dependency, always include
            })
            .collect::<Vec<_>>()
    });

    Ok(SyncChunk {
        from_storage_identity_key: args.from_storage_identity_key,
        to_storage_identity_key: args.to_storage_identity_key,
        user_identity_key: args.user_identity_key,
        user: sync_user,
        proven_txs,
        output_baskets,
        transactions,
        outputs,
        tx_labels,
        tx_label_maps,
        output_tags,
        output_tag_maps,
        certificates,
        certificate_fields,
        commissions,
        proven_tx_reqs,
    })
}
