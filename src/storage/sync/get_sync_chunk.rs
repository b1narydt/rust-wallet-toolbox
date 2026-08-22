//! getSyncChunk implementation -- extracts changed entities since last sync.
//!
//! Ports the TS chunker (`getSyncChunk.ts`) faithfully: ONE global item
//! budget (`maxItems`) and one rough byte budget (`maxRoughSize`) are spent
//! across the 12 entity types in dependency order, and each entity is
//! exhausted for the current window before the next entity emits a single
//! row. That ordering is load-bearing: it guarantees that within a sync
//! round every parent row (transaction, label, tag, certificate) has been
//! sent before any child row (map, field, commission) that references it,
//! so the consumer's foreign-key remap always finds the parent — either in
//! this round's id-map or, for parents updated in an earlier round, in the
//! persisted syncMap.
//!
//! An entity's array is present (possibly empty) once the chunker has
//! queried it, and absent when the budget ran out before reaching it. The
//! consumer reads absence as "no attempt" and only declares the round done
//! when all 12 arrays are present AND empty (BRC-40).

use crate::error::{WalletError, WalletResult};
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
    /// Global item budget for the whole chunk (TS `maxItems`). This is NOT
    /// per-entity: the chunker spends it across all 12 entity types in
    /// dependency order.
    pub max_items: i64,
    /// Rough byte budget for the whole chunk, measured as summed JSON length
    /// of the items (TS `maxRoughSize`).
    pub max_rough_size: i64,
    /// Per-entity row offsets for pagination within a sync window.
    pub offsets: SyncChunkOffsets,
}

/// Get the next incremental sync chunk of entities that changed since the last sync.
///
/// Entities are visited in the fixed TS dependency order (provenTx,
/// outputBasket, outputTag, txLabel, transaction, output, txLabelMap,
/// outputTagMap, certificate, certificateField, commission, provenTxReq),
/// each exhausted for the window before the next starts, until the shared
/// budgets run out.
pub async fn get_sync_chunk(
    storage: &dyn StorageProvider,
    args: GetSyncChunkArgs<'_>,
    trx: Option<&TrxToken>,
) -> WalletResult<SyncChunk> {
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
                proven_txs: Some(vec![]),
                output_baskets: Some(vec![]),
                transactions: Some(vec![]),
                outputs: Some(vec![]),
                tx_labels: Some(vec![]),
                tx_label_maps: Some(vec![]),
                output_tags: Some(vec![]),
                output_tag_maps: Some(vec![]),
                certificates: Some(vec![]),
                certificate_fields: Some(vec![]),
                commissions: Some(vec![]),
                proven_tx_reqs: Some(vec![]),
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
                .is_some_and(|since| u.updated_at <= since);
            if dominated {
                None
            } else {
                Some(u.clone())
            }
        }
        None => None,
    };

    let mut item_count = args.max_items;
    let mut rough_size = args.max_rough_size;
    let mut done = false;

    let mut proven_txs = None;
    let mut output_baskets = None;
    let mut output_tags = None;
    let mut tx_labels = None;
    let mut transactions = None;
    let mut outputs = None;
    let mut tx_label_maps = None;
    let mut output_tag_maps = None;
    let mut certificates = None;
    let mut certificate_fields = None;
    let mut commissions = None;
    let mut proven_tx_reqs = None;

    // One entity's contribution to the chunk (TS `addItems`): page through the
    // window at this entity's offset until it is exhausted or a budget runs
    // out. The target array becomes present (even if empty) as soon as the
    // entity has been queried once; entities never reached stay absent.
    macro_rules! chunk_entity {
        ($target:ident, $offset:expr, $divider:literal, $fetch:expr) => {
            if !done {
                let mut offset = $offset;
                loop {
                    // TS: Math.min(itemCount, Math.max(10, maxItems / maxDivider))
                    let limit = item_count.min((args.max_items / $divider).max(10));
                    if limit <= 0 {
                        break;
                    }
                    let items = ($fetch)(limit, offset).await?;
                    if $target.is_none() {
                        $target = Some(Vec::new());
                    }
                    if items.is_empty() {
                        break;
                    }
                    let dest = $target.as_mut().expect("initialized above");
                    for item in items {
                        offset += 1;
                        rough_size -= serde_json::to_string(&item)
                            .map_err(|e| {
                                WalletError::Internal(format!(
                                    "getSyncChunk: failed to size entity row: {e}"
                                ))
                            })?
                            .len() as i64;
                        dest.push(item);
                        item_count -= 1;
                        if item_count <= 0 || rough_size < 0 {
                            done = true;
                            break;
                        }
                    }
                    if done {
                        break;
                    }
                }
            }
        };
    }

    chunk_entity!(
        proven_txs,
        args.offsets.proven_tx,
        100,
        |limit, offset| async move {
            storage
                .get_proven_txs_for_user(
                    &FindForUserSincePagedArgs {
                        user_id,
                        since: args.sync_map.proven_tx.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        output_baskets,
        args.offsets.output_basket,
        1,
        |limit, offset| async move {
            storage
                .find_output_baskets(
                    &FindOutputBasketsArgs {
                        partial: OutputBasketPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.output_basket.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        output_tags,
        args.offsets.output_tag,
        1,
        |limit, offset| async move {
            storage
                .find_output_tags(
                    &FindOutputTagsArgs {
                        partial: OutputTagPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.output_tag.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        tx_labels,
        args.offsets.tx_label,
        1,
        |limit, offset| async move {
            storage
                .find_tx_labels(
                    &FindTxLabelsArgs {
                        partial: TxLabelPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.tx_label.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        transactions,
        args.offsets.transaction,
        25,
        |limit, offset| async move {
            storage
                .find_transactions(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.transaction.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                        ..Default::default()
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        outputs,
        args.offsets.output,
        25,
        |limit, offset| async move {
            storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.output.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                        ..Default::default()
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        tx_label_maps,
        args.offsets.tx_label_map,
        1,
        |limit, offset| async move {
            storage
                .get_tx_label_maps_for_user(
                    &FindForUserSincePagedArgs {
                        user_id,
                        since: args.sync_map.tx_label_map.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        output_tag_maps,
        args.offsets.output_tag_map,
        1,
        |limit, offset| async move {
            storage
                .get_output_tag_maps_for_user(
                    &FindForUserSincePagedArgs {
                        user_id,
                        since: args.sync_map.output_tag_map.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        certificates,
        args.offsets.certificate,
        25,
        |limit, offset| async move {
            storage
                .find_certificates(
                    &FindCertificatesArgs {
                        partial: CertificatePartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.certificate.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        certificate_fields,
        args.offsets.certificate_field,
        25,
        |limit, offset| async move {
            storage
                .find_certificate_fields(
                    &FindCertificateFieldsArgs {
                        partial: CertificateFieldPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.certificate_field.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        commissions,
        args.offsets.commission,
        25,
        |limit, offset| async move {
            storage
                .find_commissions(
                    &FindCommissionsArgs {
                        partial: CommissionPartial {
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        since: args.sync_map.commission.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    chunk_entity!(
        proven_tx_reqs,
        args.offsets.proven_tx_req,
        100,
        |limit, offset| async move {
            storage
                .get_proven_tx_reqs_for_user(
                    &FindForUserSincePagedArgs {
                        user_id,
                        since: args.sync_map.proven_tx_req.max_updated_at,
                        paged: Some(Paged { limit, offset }),
                    },
                    trx,
                )
                .await
        }
    );

    // No dependent-entity filtering. The previous implementation dropped
    // child rows (maps, certificate fields, commissions) whose parent was not
    // in the SAME chunk — but on an incremental round a parent updated long
    // ago is never re-sent, so a new child of an old parent was dropped on
    // every round while offsets advanced past it: silent permanent loss. TS
    // has no such filter; cross-round parents resolve through the consumer's
    // persisted syncMap id-map, and a genuinely unknown parent fails the
    // remap loudly instead of losing the row.

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
