//! processSyncChunk implementation -- applies incoming entities with ID remapping.
//!
//! For each entity type in the incoming chunk:
//! 1. Check if the entity exists locally (by natural key, not ID).
//! 2. If exists: merge using MergeEntity, update if changed, map foreign->local ID.
//! 3. If not exists: insert, map foreign->local ID.
//! 4. Update sync_map max_updated_at to the latest timestamp seen.
//!
//! Entities are processed in dependency order so that foreign key remapping
//! has all needed mappings available when processing dependent entities.

use chrono::NaiveDateTime;
use serde::Serialize;

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::*;
use crate::storage::sync::merge::MergeEntity;
use crate::storage::sync::sync_map::{SyncChunk, SyncMap};
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

/// Process an incoming SyncChunk by merging its entities into local storage.
///
/// Entities are processed in dependency order:
/// 1. User (no dependencies)
/// 2. ProvenTx, OutputBasket, TxLabel, OutputTag (depend on User)
/// 3. Transaction (depends on ProvenTx)
/// 4. Output (depends on Transaction, OutputBasket)
/// 5. TxLabelMap (depends on TxLabel, Transaction)
/// 6. OutputTagMap (depends on OutputTag, Output)
/// 7. Certificate (depends on User)
/// 8. CertificateField (depends on Certificate)
/// 9. Commission (depends on Transaction)
/// 10. ProvenTxReq (depends on ProvenTx)
pub async fn process_sync_chunk(
    storage: &dyn StorageProvider,
    chunk: SyncChunk,
    sync_map: &mut SyncMap,
    trx: Option<&TrxToken>,
) -> WalletResult<ProcessSyncChunkResult> {
    let mut result = ProcessSyncChunkResult::default();

    // 1. User
    if let Some(ref user) = chunk.user {
        let (local_user, _created) = storage.find_or_insert_user(&user.identity_key, trx).await?;

        // Update if incoming is newer
        if user.updated_at > local_user.updated_at {
            storage
                .update_user(
                    local_user.user_id,
                    &UserPartial {
                        active_storage: Some(user.active_storage.clone()),
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            result.updates += 1;
        }
    }

    // Look up local user_id for the chunk's identity key
    let local_user = storage
        .find_user_by_identity_key(&chunk.user_identity_key, trx)
        .await?;
    let local_user_id = match local_user {
        Some(u) => u.user_id,
        None => {
            return Err(WalletError::Internal(format!(
                "processSyncChunk: user not found for identity key {}",
                chunk.user_identity_key
            )));
        }
    };

    // 2. ProvenTx (no FK dependencies beyond user)
    if let Some(proven_txs) = chunk.proven_txs {
        for incoming in &proven_txs {
            sync_map.proven_tx.update_max(incoming.updated_at);
            sync_map.proven_tx.count += 1;

            // Find by natural key: txid
            let existing = storage
                .find_proven_txs(
                    &FindProvenTxsArgs {
                        partial: ProvenTxPartial {
                            txid: Some(incoming.txid.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            let foreign_id = incoming.proven_tx_id;
            if let Some(local) = existing.into_iter().next() {
                // Existing: merge
                if local.should_update(incoming) {
                    storage
                        .update_proven_tx(
                            local.proven_tx_id,
                            &ProvenTxPartial {
                                height: Some(incoming.height),
                                block_hash: Some(incoming.block_hash.clone()),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .proven_tx
                    .map_id(foreign_id, local.proven_tx_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                // New: insert
                let local_id = storage.insert_proven_tx(incoming, trx).await?;
                sync_map
                    .proven_tx
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    // 3. OutputBasket
    if let Some(output_baskets) = chunk.output_baskets {
        for incoming in &output_baskets {
            sync_map.output_basket.update_max(incoming.updated_at);
            sync_map.output_basket.count += 1;

            let foreign_id = incoming.basket_id;
            let local = storage
                .find_or_insert_output_basket(local_user_id, &incoming.name, trx)
                .await?;

            if local.should_update(incoming) {
                storage
                    .update_output_basket(
                        local.basket_id,
                        &OutputBasketPartial {
                            is_deleted: Some(incoming.is_deleted),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                result.updates += 1;
            }
            sync_map
                .output_basket
                .map_id(foreign_id, local.basket_id)
                .map_err(|e| WalletError::Internal(e))?;
        }
    }

    // 4. TxLabel
    if let Some(tx_labels) = chunk.tx_labels {
        for incoming in &tx_labels {
            sync_map.tx_label.update_max(incoming.updated_at);
            sync_map.tx_label.count += 1;

            let foreign_id = incoming.tx_label_id;
            let local = storage
                .find_or_insert_tx_label(local_user_id, &incoming.label, trx)
                .await?;

            if local.should_update(incoming) {
                storage
                    .update_tx_label(
                        local.tx_label_id,
                        &TxLabelPartial {
                            is_deleted: Some(incoming.is_deleted),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                result.updates += 1;
            }
            sync_map
                .tx_label
                .map_id(foreign_id, local.tx_label_id)
                .map_err(|e| WalletError::Internal(e))?;
        }
    }

    // 5. OutputTag
    if let Some(output_tags) = chunk.output_tags {
        for incoming in &output_tags {
            sync_map.output_tag.update_max(incoming.updated_at);
            sync_map.output_tag.count += 1;

            let foreign_id = incoming.output_tag_id;
            let local = storage
                .find_or_insert_output_tag(local_user_id, &incoming.tag, trx)
                .await?;

            if local.should_update(incoming) {
                storage
                    .update_output_tag(
                        local.output_tag_id,
                        &OutputTagPartial {
                            is_deleted: Some(incoming.is_deleted),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?;
                result.updates += 1;
            }
            sync_map
                .output_tag
                .map_id(foreign_id, local.output_tag_id)
                .map_err(|e| WalletError::Internal(e))?;
        }
    }

    // 6. Transaction (depends on ProvenTx for proven_tx_id FK)
    if let Some(transactions) = chunk.transactions {
        for incoming in &transactions {
            sync_map.transaction.update_max(incoming.updated_at);
            sync_map.transaction.count += 1;

            let foreign_id = incoming.transaction_id;

            // Remap proven_tx_id FK if present
            let local_proven_tx_id = match incoming.proven_tx_id {
                Some(fk) => sync_map.proven_tx.get_local_id(fk).or(Some(fk)),
                None => None,
            };

            // Find by natural key: reference + user_id (unique per user)
            let existing = storage
                .find_transactions(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            user_id: Some(local_user_id),
                            reference: Some(incoming.reference.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_transaction(
                            local.transaction_id,
                            &TransactionPartial {
                                proven_tx_id: local_proven_tx_id,
                                status: Some(incoming.status.clone()),
                                txid: incoming.txid.clone(),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .transaction
                    .map_id(foreign_id, local.transaction_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                // Insert with remapped user_id and proven_tx_id
                let mut to_insert = incoming.clone();
                to_insert.user_id = local_user_id;
                to_insert.proven_tx_id = local_proven_tx_id;
                to_insert.transaction_id = 0; // auto-increment
                let local_id = storage.insert_transaction(&to_insert, trx).await?;
                sync_map
                    .transaction
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    // 7. Output (depends on Transaction and OutputBasket for FKs)
    if let Some(outputs) = chunk.outputs {
        for incoming in &outputs {
            sync_map.output.update_max(incoming.updated_at);
            sync_map.output.count += 1;

            let foreign_id = incoming.output_id;

            // Remap transaction_id FK
            let local_tx_id = sync_map
                .transaction
                .get_local_id(incoming.transaction_id)
                .unwrap_or(incoming.transaction_id);

            // Remap basket_id FK if present
            let local_basket_id = match incoming.basket_id {
                Some(fk) => sync_map.output_basket.get_local_id(fk).or(Some(fk)),
                None => None,
            };

            // Remap spent_by FK if present
            let local_spent_by = match incoming.spent_by {
                Some(fk) => sync_map.transaction.get_local_id(fk).or(Some(fk)),
                None => None,
            };

            // Find by natural key: transaction_id + vout
            let existing = storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            transaction_id: Some(local_tx_id),
                            vout: Some(incoming.vout),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_output(
                            local.output_id,
                            &OutputPartial {
                                basket_id: local_basket_id,
                                spendable: Some(incoming.spendable),
                                spent_by: local_spent_by,
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .output
                    .map_id(foreign_id, local.output_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                let mut to_insert = incoming.clone();
                to_insert.user_id = local_user_id;
                to_insert.transaction_id = local_tx_id;
                to_insert.basket_id = local_basket_id;
                to_insert.spent_by = local_spent_by;
                to_insert.output_id = 0; // auto-increment
                let local_id = storage.insert_output(&to_insert, trx).await?;
                sync_map
                    .output
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    // 8. TxLabelMap (depends on TxLabel and Transaction)
    if let Some(tx_label_maps) = chunk.tx_label_maps {
        for incoming in &tx_label_maps {
            sync_map.tx_label_map.update_max(incoming.updated_at);
            sync_map.tx_label_map.count += 1;

            // Remap FKs
            let local_tx_label_id = sync_map
                .tx_label
                .get_local_id(incoming.tx_label_id)
                .unwrap_or(incoming.tx_label_id);
            let local_tx_id = sync_map
                .transaction
                .get_local_id(incoming.transaction_id)
                .unwrap_or(incoming.transaction_id);

            // Find by natural key: tx_label_id + transaction_id
            let existing = storage
                .find_tx_label_maps(
                    &FindTxLabelMapsArgs {
                        partial: TxLabelMapPartial {
                            tx_label_id: Some(local_tx_label_id),
                            transaction_id: Some(local_tx_id),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_tx_label_map(
                            local.transaction_id,
                            local.tx_label_id,
                            &TxLabelMapPartial {
                                is_deleted: Some(incoming.is_deleted),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
            } else {
                let mut to_insert = incoming.clone();
                to_insert.tx_label_id = local_tx_label_id;
                to_insert.transaction_id = local_tx_id;
                storage.insert_tx_label_map(&to_insert, trx).await?;
                result.inserts += 1;
            }
        }
    }

    // 9. OutputTagMap (depends on OutputTag and Output)
    if let Some(output_tag_maps) = chunk.output_tag_maps {
        for incoming in &output_tag_maps {
            sync_map.output_tag_map.update_max(incoming.updated_at);
            sync_map.output_tag_map.count += 1;

            // Remap FKs
            let local_tag_id = sync_map
                .output_tag
                .get_local_id(incoming.output_tag_id)
                .unwrap_or(incoming.output_tag_id);
            let local_output_id = sync_map
                .output
                .get_local_id(incoming.output_id)
                .unwrap_or(incoming.output_id);

            // Find by natural key: output_tag_id + output_id
            let existing = storage
                .find_output_tag_maps(
                    &FindOutputTagMapsArgs {
                        partial: OutputTagMapPartial {
                            output_tag_id: Some(local_tag_id),
                            output_id: Some(local_output_id),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_output_tag_map(
                            local.output_id,
                            local.output_tag_id,
                            &OutputTagMapPartial {
                                is_deleted: Some(incoming.is_deleted),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
            } else {
                let mut to_insert = incoming.clone();
                to_insert.output_tag_id = local_tag_id;
                to_insert.output_id = local_output_id;
                storage.insert_output_tag_map(&to_insert, trx).await?;
                result.inserts += 1;
            }
        }
    }

    // 10. Certificate (depends on User)
    if let Some(certificates) = chunk.certificates {
        for incoming in &certificates {
            sync_map.certificate.update_max(incoming.updated_at);
            sync_map.certificate.count += 1;

            let foreign_id = incoming.certificate_id;

            // Find by natural key: serial_number + certifier (unique per certificate)
            let existing = storage
                .find_certificates(
                    &FindCertificatesArgs {
                        partial: CertificatePartial {
                            user_id: Some(local_user_id),
                            serial_number: Some(incoming.serial_number.clone()),
                            certifier: Some(incoming.certifier.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_certificate(
                            local.certificate_id,
                            &CertificatePartial {
                                subject: Some(incoming.subject.clone()),
                                verifier: incoming.verifier.clone().map(Some).unwrap_or(None),
                                revocation_outpoint: Some(incoming.revocation_outpoint.clone()),
                                signature: Some(incoming.signature.clone()),
                                is_deleted: Some(incoming.is_deleted),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .certificate
                    .map_id(foreign_id, local.certificate_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                let mut to_insert = incoming.clone();
                to_insert.user_id = local_user_id;
                to_insert.certificate_id = 0;
                let local_id = storage.insert_certificate(&to_insert, trx).await?;
                sync_map
                    .certificate
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    // 11. CertificateField (depends on Certificate)
    if let Some(certificate_fields) = chunk.certificate_fields {
        for incoming in &certificate_fields {
            sync_map.certificate_field.update_max(incoming.updated_at);
            sync_map.certificate_field.count += 1;

            // Remap certificate_id FK
            let local_cert_id = sync_map
                .certificate
                .get_local_id(incoming.certificate_id)
                .unwrap_or(incoming.certificate_id);

            // Find by natural key: certificate_id + field_name
            let existing = storage
                .find_certificate_fields(
                    &FindCertificateFieldsArgs {
                        partial: CertificateFieldPartial {
                            certificate_id: Some(local_cert_id),
                            field_name: Some(incoming.field_name.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_certificate_field(
                            local.certificate_id,
                            &local.field_name,
                            &CertificateFieldPartial {
                                field_value: Some(incoming.field_value.clone()),
                                master_key: Some(incoming.master_key.clone()),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
            } else {
                let mut to_insert = incoming.clone();
                to_insert.user_id = local_user_id;
                to_insert.certificate_id = local_cert_id;
                storage.insert_certificate_field(&to_insert, trx).await?;
                result.inserts += 1;
            }
        }
    }

    // 12. Commission (depends on Transaction)
    if let Some(commissions) = chunk.commissions {
        for incoming in &commissions {
            sync_map.commission.update_max(incoming.updated_at);
            sync_map.commission.count += 1;

            let foreign_id = incoming.commission_id;

            // Remap transaction_id FK
            let local_tx_id = sync_map
                .transaction
                .get_local_id(incoming.transaction_id)
                .unwrap_or(incoming.transaction_id);

            // Find by natural key: transaction_id + key_offset
            let existing = storage
                .find_commissions(
                    &FindCommissionsArgs {
                        partial: CommissionPartial {
                            user_id: Some(local_user_id),
                            transaction_id: Some(local_tx_id),
                            key_offset: Some(incoming.key_offset.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_commission(
                            local.commission_id,
                            &CommissionPartial {
                                satoshis: Some(incoming.satoshis),
                                is_redeemed: Some(incoming.is_redeemed),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .commission
                    .map_id(foreign_id, local.commission_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                let mut to_insert = incoming.clone();
                to_insert.user_id = local_user_id;
                to_insert.transaction_id = local_tx_id;
                to_insert.commission_id = 0;
                let local_id = storage.insert_commission(&to_insert, trx).await?;
                sync_map
                    .commission
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    // 13. ProvenTxReq (depends on ProvenTx)
    if let Some(proven_tx_reqs) = chunk.proven_tx_reqs {
        for incoming in &proven_tx_reqs {
            sync_map.proven_tx_req.update_max(incoming.updated_at);
            sync_map.proven_tx_req.count += 1;

            let foreign_id = incoming.proven_tx_req_id;

            // Remap proven_tx_id FK if present
            let local_proven_tx_id = match incoming.proven_tx_id {
                Some(fk) => sync_map.proven_tx.get_local_id(fk).or(Some(fk)),
                None => None,
            };

            // Find by natural key: txid
            let existing = storage
                .find_proven_tx_reqs(
                    &FindProvenTxReqsArgs {
                        partial: ProvenTxReqPartial {
                            txid: Some(incoming.txid.clone()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            if let Some(local) = existing.into_iter().next() {
                if local.should_update(incoming) {
                    storage
                        .update_proven_tx_req(
                            local.proven_tx_req_id,
                            &ProvenTxReqPartial {
                                proven_tx_id: local_proven_tx_id,
                                status: Some(incoming.status.clone()),
                                notified: Some(incoming.notified),
                                ..Default::default()
                            },
                            trx,
                        )
                        .await?;
                    result.updates += 1;
                }
                sync_map
                    .proven_tx_req
                    .map_id(foreign_id, local.proven_tx_req_id)
                    .map_err(|e| WalletError::Internal(e))?;
            } else {
                let mut to_insert = incoming.clone();
                to_insert.proven_tx_id = local_proven_tx_id;
                to_insert.proven_tx_req_id = 0;
                let local_id = storage.insert_proven_tx_req(&to_insert, trx).await?;
                sync_map
                    .proven_tx_req
                    .map_id(foreign_id, local_id)
                    .map_err(|e| WalletError::Internal(e))?;
                result.inserts += 1;
            }
        }
    }

    Ok(result)
}

/// Result of processing a sync chunk.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSyncChunkResult {
    /// Whether the sync is complete (no more chunks pending).
    pub done: bool,
    /// Latest updated_at timestamp seen in this chunk.
    #[serde(
        rename = "maxUpdated_at",
        default,
        with = "crate::serde_datetime::option"
    )]
    pub max_updated_at: Option<NaiveDateTime>,
    /// Number of entities inserted.
    pub inserts: i64,
    /// Number of entities updated.
    pub updates: i64,
}
