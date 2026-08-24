//! listActions query translation from WalletInterface args to storage find args.
//!
//! Ported from wallet-toolbox/src/storage/methods/listActionsKnex.ts.
//! Translates bsv-sdk `ListActionsArgs` into low-level `find_*` / `count_*`
//! calls on `StorageReader`, returning `ListActionsResult`.

use bsv::wallet::interfaces::{
    Action, ActionInput, ActionOutput, ActionStatus, ListActionsArgs, ListActionsResult, QueryMode,
};
use std::collections::{HashMap, HashSet};

use crate::error::WalletResult;
use crate::status::TransactionStatus;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, FindTxLabelMapsArgs, FindTxLabelsArgs, OutputPartial,
    Paged, TransactionPartial, TxLabelMapPartial, TxLabelPartial,
};
use crate::storage::traits::reader::StorageReader;
use crate::storage::TrxToken;
use crate::tables::{Output, Transaction, TxLabelMap};
use crate::wallet::types::{SPEC_OP_FAILED_ACTIONS, SPEC_OP_NO_SEND_ACTIONS};

// ---------------------------------------------------------------------------
// SpecOp dispatch for list_actions
// ---------------------------------------------------------------------------

/// Special operations routed through list_actions via label overloading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListActionsSpecOp {
    /// Filter for nosend-status actions; optionally abort them.
    NoSendActions,
    /// Filter for failed-status actions; optionally unfail (reset to unprocessed).
    FailedActions,
}

/// Resolve whether a list_actions call is a specOp based on labels.
///
/// Returns (specOp, effective_labels). If no specOp is detected,
/// `specOp` is `None` and labels are returned unchanged.
pub fn resolve_list_actions_spec_op(labels: &[String]) -> (Option<ListActionsSpecOp>, Vec<String>) {
    for label in labels {
        if label == SPEC_OP_NO_SEND_ACTIONS {
            let filtered: Vec<String> = labels
                .iter()
                .filter(|l| l.as_str() != SPEC_OP_NO_SEND_ACTIONS)
                .cloned()
                .collect();
            return (Some(ListActionsSpecOp::NoSendActions), filtered);
        }
        if label == SPEC_OP_FAILED_ACTIONS {
            let filtered: Vec<String> = labels
                .iter()
                .filter(|l| l.as_str() != SPEC_OP_FAILED_ACTIONS)
                .cloned()
                .collect();
            return (Some(ListActionsSpecOp::FailedActions), filtered);
        }
    }
    (None, labels.to_vec())
}

/// Handle the NoSendActions specOp: list actions with nosend status.
async fn handle_no_send_actions(
    storage: &dyn StorageReader,
    user_id: i64,
    trx: Option<&TrxToken>,
) -> WalletResult<ListActionsResult> {
    let find_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            ..Default::default()
        },
        status: Some(vec![TransactionStatus::Nosend]),
        no_raw_tx: true,
        ..Default::default()
    };

    let txs = storage.find_transactions(&find_args, trx).await?;
    let total = txs.len() as u32;

    let actions: Vec<Action> = txs
        .iter()
        .map(|tx| Action {
            txid: tx.txid.clone().unwrap_or_default(),
            satoshis: tx.satoshis,
            status: ActionStatus::NoSend,
            is_outgoing: tx.is_outgoing,
            description: tx.description.clone(),
            version: tx.version.unwrap_or(0) as u32,
            lock_time: tx.lock_time.unwrap_or(0) as u32,
            labels: None,
            inputs: None,
            outputs: None,
        })
        .collect();

    Ok(ListActionsResult {
        total_actions: total,
        actions,
    })
}

/// Handle the FailedActions specOp: list actions with failed status.
async fn handle_failed_actions(
    storage: &dyn StorageReader,
    user_id: i64,
    trx: Option<&TrxToken>,
) -> WalletResult<ListActionsResult> {
    let find_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            ..Default::default()
        },
        status: Some(vec![TransactionStatus::Failed]),
        no_raw_tx: true,
        ..Default::default()
    };

    let txs = storage.find_transactions(&find_args, trx).await?;
    let total = txs.len() as u32;

    let actions: Vec<Action> = txs
        .iter()
        .map(|tx| Action {
            txid: tx.txid.clone().unwrap_or_default(),
            satoshis: tx.satoshis,
            status: tx_status_to_action_status(&tx.status),
            is_outgoing: tx.is_outgoing,
            description: tx.description.clone(),
            version: tx.version.unwrap_or(0) as u32,
            lock_time: tx.lock_time.unwrap_or(0) as u32,
            labels: None,
            inputs: None,
            outputs: None,
        })
        .collect();

    Ok(ListActionsResult {
        total_actions: total,
        actions,
    })
}

/// Execute a listActions query with specOp support.
///
/// Checks for specOp labels before falling through to the normal query path.
pub async fn list_actions_with_spec_ops(
    storage: &dyn StorageReader,
    auth: &str,
    user_id: i64,
    args: &ListActionsArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<ListActionsResult> {
    let (spec_op, _effective_labels) = resolve_list_actions_spec_op(&args.labels);

    if let Some(op) = spec_op {
        return match op {
            ListActionsSpecOp::NoSendActions => handle_no_send_actions(storage, user_id, trx).await,
            ListActionsSpecOp::FailedActions => handle_failed_actions(storage, user_id, trx).await,
        };
    }

    // Fall through to normal query path
    list_actions(storage, auth, user_id, args, trx).await
}

/// Default statuses shown when listing actions (no specOp override).
const DEFAULT_STATI: &[TransactionStatus] = &[
    TransactionStatus::Completed,
    TransactionStatus::Unprocessed,
    TransactionStatus::Sending,
    TransactionStatus::Unproven,
    TransactionStatus::Unsigned,
    TransactionStatus::Nosend,
    TransactionStatus::Nonfinal,
];

/// Keep batched enrichment safely below SQLite's conservative 999-variable
/// ceiling while leaving room for fixed filters. Server backends have much
/// larger protocol ceilings, so this bound is safe for all dialects.
const ENRICHMENT_IN_CHUNK_SIZE: usize = 500;

/// Execute a listActions query against the given storage reader.
///
/// Translates the bsv-sdk `ListActionsArgs` to low-level find/count calls,
/// applying label filtering, pagination, and optional include flags.
/// The wallet-provider path runs the base and four enrichment queries with `trx = None`, so the result is not a snapshot-consistent read.
pub async fn list_actions(
    storage: &dyn StorageReader,
    _auth: &str,
    user_id: i64,
    args: &ListActionsArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<ListActionsResult> {
    let limit = args.limit.unwrap_or(10) as i64;
    let offset = args.offset.map(|o| o as i64).unwrap_or(0);
    let include_labels = *args.include_labels;
    let include_inputs = *args.include_inputs;
    let include_outputs = *args.include_outputs;
    let include_input_source_locking_scripts = *args.include_input_source_locking_scripts;
    let include_output_locking_scripts = *args.include_output_locking_scripts;

    let is_query_mode_all = matches!(args.label_query_mode, Some(QueryMode::All));

    // Resolve label names to label IDs via storage
    let labels = &args.labels;
    let mut label_ids: Vec<i64> = Vec::new();

    if !labels.is_empty() {
        let found = storage
            .find_tx_labels(
                &FindTxLabelsArgs {
                    partial: TxLabelPartial {
                        user_id: Some(user_id),
                        is_deleted: Some(false),
                        ..Default::default()
                    },
                    labels: Some(labels.clone()),
                    ..Default::default()
                },
                trx,
            )
            .await?;
        for tl in found {
            label_ids.push(tl.tx_label_id);
        }
    }

    // Short-circuit: "all" mode requires all labels present
    if is_query_mode_all && label_ids.len() < labels.len() {
        return Ok(ListActionsResult {
            total_actions: 0,
            actions: vec![],
        });
    }

    // Short-circuit: "any" mode with no matching labels
    if !is_query_mode_all && label_ids.is_empty() && !labels.is_empty() {
        return Ok(ListActionsResult {
            total_actions: 0,
            actions: vec![],
        });
    }

    // Build the base transaction query with status filter
    let stati: Vec<TransactionStatus> = DEFAULT_STATI.to_vec();

    let find_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            ..Default::default()
        },
        status: Some(stati),
        paged: Some(Paged { limit, offset }),
        no_raw_tx: true,
        ..Default::default()
    };

    // Find transactions (base query)
    let txs = storage.find_transactions(&find_args, trx).await?;

    // Filter by labels if needed (post-query filtering)
    let filtered_txs: Vec<Transaction> = if label_ids.is_empty() {
        txs
    } else {
        let transaction_ids = txs.iter().map(|tx| tx.transaction_id).collect::<Vec<_>>();
        let mut maps_by_transaction = HashMap::<i64, Vec<TxLabelMap>>::new();
        for transaction_ids_chunk in transaction_ids.chunks(ENRICHMENT_IN_CHUNK_SIZE) {
            let maps = storage
                .find_tx_label_maps(
                    &FindTxLabelMapsArgs {
                        partial: TxLabelMapPartial {
                            is_deleted: Some(false),
                            ..Default::default()
                        },
                        transaction_ids: Some(transaction_ids_chunk.to_vec()),
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for map in maps {
                maps_by_transaction
                    .entry(map.transaction_id)
                    .or_default()
                    .push(map);
            }
        }

        txs.into_iter()
            .filter(|tx| {
                let matching_count = maps_by_transaction
                    .get(&tx.transaction_id)
                    .into_iter()
                    .flatten()
                    .filter(|map| label_ids.contains(&map.tx_label_id))
                    .count();

                if is_query_mode_all {
                    matching_count >= label_ids.len()
                } else {
                    matching_count > 0
                }
            })
            .collect()
    };

    // Get total count for pagination
    let total_actions = if filtered_txs.len() < limit as usize {
        (offset as u32) + filtered_txs.len() as u32
    } else {
        // Need to count all matching rows
        let count_args = FindTransactionsArgs {
            partial: TransactionPartial {
                user_id: Some(user_id),
                ..Default::default()
            },
            status: Some(DEFAULT_STATI.to_vec()),
            no_raw_tx: true,
            ..Default::default()
        };
        let total = storage.count_transactions(&count_args, trx).await?;
        total as u32
    };

    let transaction_ids = filtered_txs
        .iter()
        .map(|tx| tx.transaction_id)
        .collect::<Vec<_>>();

    let mut labels_by_transaction = HashMap::<i64, Vec<String>>::new();
    if include_labels && !transaction_ids.is_empty() {
        let mut label_maps = Vec::new();
        // Resulting label order is unspecified and deliberately not pinned, matching base behavior.
        for transaction_ids_chunk in transaction_ids.chunks(ENRICHMENT_IN_CHUNK_SIZE) {
            label_maps.extend(
                storage
                    .find_tx_label_maps(
                        &FindTxLabelMapsArgs {
                            partial: TxLabelMapPartial {
                                is_deleted: Some(false),
                                ..Default::default()
                            },
                            transaction_ids: Some(transaction_ids_chunk.to_vec()),
                            ..Default::default()
                        },
                        trx,
                    )
                    .await?,
            );
        }

        let mut seen_label_ids = HashSet::new();
        let referenced_label_ids = label_maps
            .iter()
            .map(|map| map.tx_label_id)
            .filter(|tx_label_id| seen_label_ids.insert(*tx_label_id))
            .collect::<Vec<_>>();
        let mut labels_by_id = HashMap::new();
        for label_ids_chunk in referenced_label_ids.chunks(ENRICHMENT_IN_CHUNK_SIZE) {
            for label in storage
                .find_tx_labels(
                    &FindTxLabelsArgs {
                        partial: TxLabelPartial {
                            user_id: Some(user_id),
                            is_deleted: Some(false),
                            ..Default::default()
                        },
                        tx_label_ids: Some(label_ids_chunk.to_vec()),
                        ..Default::default()
                    },
                    trx,
                )
                .await?
            {
                labels_by_id.insert(label.tx_label_id, label.label);
            }
        }

        for map in label_maps {
            if let Some(label) = labels_by_id.get(&map.tx_label_id) {
                labels_by_transaction
                    .entry(map.transaction_id)
                    .or_default()
                    .push(label.clone());
            }
        }
    }

    let mut outputs_by_transaction = HashMap::<i64, Vec<Output>>::new();
    if include_outputs && !transaction_ids.is_empty() {
        for transaction_ids_chunk in transaction_ids.chunks(ENRICHMENT_IN_CHUNK_SIZE) {
            let outputs = storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            // Deliberate TS divergence: enrichment is tenant-scoped so an
                            // output attached to another user's transaction cannot leak.
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        transaction_ids: Some(transaction_ids_chunk.to_vec()),
                        no_script: !include_output_locking_scripts,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for output in outputs {
                outputs_by_transaction
                    .entry(output.transaction_id)
                    .or_default()
                    .push(output);
            }
        }
    }

    let mut inputs_by_transaction = HashMap::<i64, Vec<Output>>::new();
    if include_inputs && !transaction_ids.is_empty() {
        // Resulting input order is unspecified and deliberately not pinned, matching base behavior.
        for transaction_ids_chunk in transaction_ids.chunks(ENRICHMENT_IN_CHUNK_SIZE) {
            let inputs = storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            // Deliberate TS divergence: source outputs are tenant-scoped;
                            // another user's spend metadata must not cross wallets.
                            user_id: Some(user_id),
                            ..Default::default()
                        },
                        spent_by_ids: Some(transaction_ids_chunk.to_vec()),
                        no_script: !include_input_source_locking_scripts,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for input in inputs {
                if let Some(spent_by) = input.spent_by {
                    inputs_by_transaction
                        .entry(spent_by)
                        .or_default()
                        .push(input);
                }
            }
        }
    }

    // Build Action results from the four set-based enrichment reads above.
    let mut actions: Vec<Action> = Vec::with_capacity(filtered_txs.len());
    for tx in &filtered_txs {
        let status = tx_status_to_action_status(&tx.status);
        let mut action = Action {
            txid: tx.txid.clone().unwrap_or_default(),
            satoshis: tx.satoshis,
            status,
            is_outgoing: tx.is_outgoing,
            description: tx.description.clone(),
            version: tx.version.unwrap_or(0) as u32,
            lock_time: tx.lock_time.unwrap_or(0) as u32,
            labels: None,
            inputs: None,
            outputs: None,
        };

        if include_labels {
            let labels = labels_by_transaction
                .remove(&tx.transaction_id)
                .unwrap_or_default();
            action.labels = (!labels.is_empty()).then_some(labels);
        }

        if include_outputs {
            for o in outputs_by_transaction
                .remove(&tx.transaction_id)
                .unwrap_or_default()
            {
                let ao = ActionOutput {
                    satoshis: o.satoshis as u64,
                    spendable: o.spendable,
                    locking_script: if include_output_locking_scripts {
                        o.locking_script
                    } else {
                        None
                    },
                    custom_instructions: None, // stripped for security
                    tags: vec![],
                    output_index: o.vout as u32,
                    output_description: o.output_description.unwrap_or_default(),
                    basket: None,
                };
                action.outputs.get_or_insert_with(Vec::new).push(ao);
            }
        }

        if include_inputs {
            for o in inputs_by_transaction
                .remove(&tx.transaction_id)
                .unwrap_or_default()
            {
                let ai = ActionInput {
                    source_outpoint: format!("{}.{}", o.txid.as_deref().unwrap_or(""), o.vout),
                    source_satoshis: o.satoshis as u64,
                    source_locking_script: if include_input_source_locking_scripts {
                        o.locking_script
                    } else {
                        None
                    },
                    unlocking_script: None,
                    input_description: o.output_description.unwrap_or_default(),
                    sequence_number: o.sequence_number.unwrap_or(0) as u32,
                };
                action.inputs.get_or_insert_with(Vec::new).push(ai);
            }
        }

        actions.push(action);
    }

    debug_assert!(labels_by_transaction.is_empty());
    debug_assert!(outputs_by_transaction.is_empty());
    debug_assert!(inputs_by_transaction.is_empty());

    Ok(ListActionsResult {
        total_actions,
        actions,
    })
}

/// Convert internal TransactionStatus to bsv-sdk ActionStatus.
fn tx_status_to_action_status(status: &TransactionStatus) -> ActionStatus {
    match status {
        TransactionStatus::Completed => ActionStatus::Completed,
        TransactionStatus::Unprocessed => ActionStatus::Unprocessed,
        TransactionStatus::Sending => ActionStatus::Sending,
        TransactionStatus::Unproven => ActionStatus::Unproven,
        TransactionStatus::Unsigned => ActionStatus::Unsigned,
        TransactionStatus::Nosend => ActionStatus::NoSend,
        TransactionStatus::Nonfinal => ActionStatus::NonFinal,
        // Failed and Unfail don't have direct ActionStatus equivalents
        _ => ActionStatus::Completed,
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use bsv::wallet::interfaces::ListActionsArgs;
    use chrono::NaiveDateTime;

    use super::*;
    use crate::storage::find_args::*;
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::StorageConfig;
    use crate::tables::*;
    use crate::types::{Chain, StorageProvidedBy};

    struct Fixture {
        storage: SqliteStorage,
        user_id: i64,
        transaction_ids: Vec<i64>,
        excluded_transaction_id: i64,
    }

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-08-23 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    async fn setup_fixture() -> Fixture {
        let storage = SqliteStorage::new_sqlite(
            StorageConfig {
                url: "sqlite::memory:".to_string(),
                ..Default::default()
            },
            Chain::Test,
        )
        .await
        .unwrap();
        storage.migrate_database().await.unwrap();

        let timestamp = now();
        let user_id = storage
            .insert_user(
                &User {
                    created_at: timestamp,
                    updated_at: timestamp,
                    user_id: 0,
                    identity_key: "list-actions-batch-user".to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap();

        let mut transaction_ids = Vec::new();
        for index in 0..5 {
            let transaction_id = storage
                .insert_transaction(
                    &Transaction {
                        created_at: timestamp,
                        updated_at: timestamp,
                        transaction_id: 0,
                        user_id,
                        proven_tx_id: None,
                        status: TransactionStatus::Completed,
                        reference: format!("reference-{index}"),
                        is_outgoing: index % 2 == 0,
                        satoshis: 1_000 + index as i64,
                        description: format!("action-{index}"),
                        version: Some(1),
                        lock_time: Some(index),
                        txid: Some(format!("{index:064x}")),
                        input_beef: None,
                        raw_tx: None,
                    },
                    None,
                )
                .await
                .unwrap();
            transaction_ids.push(transaction_id);
        }

        let excluded_transaction_id = storage
            .insert_transaction(
                &Transaction {
                    created_at: timestamp,
                    updated_at: timestamp,
                    transaction_id: 0,
                    user_id,
                    proven_tx_id: None,
                    status: TransactionStatus::Failed,
                    reference: "excluded-reference".to_string(),
                    is_outgoing: false,
                    satoshis: 999,
                    description: "excluded-action".to_string(),
                    version: Some(1),
                    lock_time: Some(0),
                    txid: Some(format!("{:064x}", 99)),
                    input_beef: None,
                    raw_tx: None,
                },
                None,
            )
            .await
            .unwrap();

        // Created first but deliberately mapped after label-0 so the fixture
        // distinguishes label creation order from the base map-row order.
        let early_mapped_late_label_id = storage
            .insert_tx_label(
                &TxLabel {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: 0,
                    user_id,
                    label: "early-mapped-late".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        let mut active_label_ids = Vec::new();
        for index in [0, 2, 4] {
            let label_id = storage
                .insert_tx_label(
                    &TxLabel {
                        created_at: timestamp,
                        updated_at: timestamp,
                        tx_label_id: 0,
                        user_id,
                        label: format!("label-{index}"),
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
            storage
                .insert_tx_label_map(
                    &TxLabelMap {
                        created_at: timestamp,
                        updated_at: timestamp,
                        tx_label_id: label_id,
                        transaction_id: transaction_ids[index],
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
            active_label_ids.push(label_id);
        }

        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: early_mapped_late_label_id,
                    transaction_id: transaction_ids[0],
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        // One active label is shared by two transactions.
        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: active_label_ids[0],
                    transaction_id: transaction_ids[4],
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        let deleted_label_id = storage
            .insert_tx_label(
                &TxLabel {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: 0,
                    user_id,
                    label: "deleted-label".to_string(),
                    is_deleted: true,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: deleted_label_id,
                    transaction_id: transaction_ids[3],
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: active_label_ids[0],
                    transaction_id: transaction_ids[2],
                    is_deleted: true,
                },
                None,
            )
            .await
            .unwrap();

        let output_specs = [
            (0, 2, Some(1)),
            (0, 0, None),
            (0, 1, None),
            (2, 1, Some(1)),
            (2, 0, Some(3)),
            (3, 0, Some(4)),
            (4, 0, None),
        ];
        for (source_index, vout, spent_by_index) in output_specs {
            storage
                .insert_output(
                    &Output {
                        created_at: timestamp,
                        updated_at: timestamp,
                        output_id: 0,
                        user_id,
                        transaction_id: transaction_ids[source_index],
                        basket_id: None,
                        spendable: spent_by_index.is_none(),
                        change: false,
                        output_description: Some(format!("output-{source_index}-{vout}")),
                        vout,
                        satoshis: 500 + source_index as i64 * 10 + vout as i64,
                        provided_by: StorageProvidedBy::Storage,
                        purpose: "test".to_string(),
                        output_type: "P2PKH".to_string(),
                        txid: Some(format!("{source_index:064x}")),
                        sender_identity_key: None,
                        derivation_prefix: None,
                        derivation_suffix: None,
                        custom_instructions: Some("must-not-leak".to_string()),
                        spent_by: spent_by_index.map(|index| transaction_ids[index]),
                        sequence_number: Some(10 + vout),
                        spending_description: None,
                        script_length: Some(2),
                        script_offset: None,
                        locking_script: Some(vec![source_index as u8, vout as u8]),
                    },
                    None,
                )
                .await
                .unwrap();
        }

        for (vout, spent_by) in [(0, Some(excluded_transaction_id)), (1, None)] {
            storage
                .insert_output(
                    &Output {
                        created_at: timestamp,
                        updated_at: timestamp,
                        output_id: 0,
                        user_id,
                        transaction_id: excluded_transaction_id,
                        basket_id: None,
                        spendable: spent_by.is_none(),
                        change: false,
                        output_description: Some(format!("excluded-output-{vout}")),
                        vout,
                        satoshis: 700 + i64::from(vout),
                        provided_by: StorageProvidedBy::Storage,
                        purpose: "test".to_string(),
                        output_type: "P2PKH".to_string(),
                        txid: Some(format!("{:064x}", 99)),
                        sender_identity_key: None,
                        derivation_prefix: None,
                        derivation_suffix: None,
                        custom_instructions: None,
                        spent_by,
                        sequence_number: Some(20 + vout),
                        spending_description: None,
                        script_length: Some(2),
                        script_offset: None,
                        locking_script: Some(vec![99, vout as u8]),
                    },
                    None,
                )
                .await
                .unwrap();
        }

        Fixture {
            storage,
            user_id,
            transaction_ids,
            excluded_transaction_id,
        }
    }

    async fn setup_chunk_boundary_fixture() -> Fixture {
        let storage = SqliteStorage::new_sqlite(
            StorageConfig {
                url: "sqlite::memory:".to_string(),
                ..Default::default()
            },
            Chain::Test,
        )
        .await
        .unwrap();
        storage.migrate_database().await.unwrap();

        let timestamp = now();
        let user_id = storage
            .insert_user(
                &User {
                    created_at: timestamp,
                    updated_at: timestamp,
                    user_id: 0,
                    identity_key: "chunk-boundary-user".to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap();
        let mut transaction_ids = Vec::with_capacity(ENRICHMENT_IN_CHUNK_SIZE + 1);
        for index in 0..=ENRICHMENT_IN_CHUNK_SIZE {
            transaction_ids.push(
                storage
                    .insert_transaction(
                        &Transaction {
                            created_at: timestamp,
                            updated_at: timestamp,
                            transaction_id: 0,
                            user_id,
                            proven_tx_id: None,
                            status: TransactionStatus::Completed,
                            reference: format!("chunk-reference-{index}"),
                            is_outgoing: false,
                            satoshis: index as i64,
                            description: format!("chunk-action-{index}"),
                            version: Some(1),
                            lock_time: Some(0),
                            txid: Some(format!("{index:064x}")),
                            input_beef: None,
                            raw_tx: None,
                        },
                        None,
                    )
                    .await
                    .unwrap(),
            );
        }

        let shared_label_id = storage
            .insert_tx_label(
                &TxLabel {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: 0,
                    user_id,
                    label: "boundary-shared-label".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        for transaction_id in [transaction_ids[0], *transaction_ids.last().unwrap()] {
            storage
                .insert_tx_label_map(
                    &TxLabelMap {
                        created_at: timestamp,
                        updated_at: timestamp,
                        tx_label_id: shared_label_id,
                        transaction_id,
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
        }

        // Keep the transaction-id and label-id chunk boundaries independent: all
        // additional labels belong to the final transaction.
        for index in 0..ENRICHMENT_IN_CHUNK_SIZE {
            let tx_label_id = storage
                .insert_tx_label(
                    &TxLabel {
                        created_at: timestamp,
                        updated_at: timestamp,
                        tx_label_id: 0,
                        user_id,
                        label: format!("boundary-label-{index}"),
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
            storage
                .insert_tx_label_map(
                    &TxLabelMap {
                        created_at: timestamp,
                        updated_at: timestamp,
                        tx_label_id,
                        transaction_id: *transaction_ids.last().unwrap(),
                        is_deleted: false,
                    },
                    None,
                )
                .await
                .unwrap();
        }

        for (source_index, spent_by_index) in [
            (0, Some(ENRICHMENT_IN_CHUNK_SIZE)),
            (ENRICHMENT_IN_CHUNK_SIZE, None),
        ] {
            storage
                .insert_output(
                    &Output {
                        created_at: timestamp,
                        updated_at: timestamp,
                        output_id: 0,
                        user_id,
                        transaction_id: transaction_ids[source_index],
                        basket_id: None,
                        spendable: spent_by_index.is_none(),
                        change: false,
                        output_description: Some(format!("boundary-output-{source_index}")),
                        vout: 0,
                        satoshis: 1,
                        provided_by: StorageProvidedBy::Storage,
                        purpose: "test".to_string(),
                        output_type: "P2PKH".to_string(),
                        txid: Some(format!("{source_index:064x}")),
                        sender_identity_key: None,
                        derivation_prefix: None,
                        derivation_suffix: None,
                        custom_instructions: None,
                        spent_by: spent_by_index.map(|index| transaction_ids[index]),
                        sequence_number: Some(1),
                        spending_description: None,
                        script_length: Some(2),
                        script_offset: None,
                        locking_script: Some(vec![source_index as u8, 0]),
                    },
                    None,
                )
                .await
                .unwrap();
        }

        Fixture {
            storage,
            user_id,
            excluded_transaction_id: 0,
            transaction_ids,
        }
    }

    fn list_args(include_scripts: bool) -> ListActionsArgs {
        serde_json::from_value(serde_json::json!({
            "labels": [],
            "includeLabels": true,
            "includeInputs": true,
            "includeInputSourceLockingScripts": include_scripts,
            "includeOutputs": true,
            "includeOutputLockingScripts": include_scripts,
            "limit": 100
        }))
        .unwrap()
    }

    async fn selected_transactions(storage: &dyn StorageReader, user_id: i64) -> Vec<Transaction> {
        storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(user_id),
                        ..Default::default()
                    },
                    status: Some(DEFAULT_STATI.to_vec()),
                    paged: Some(Paged {
                        limit: 100,
                        offset: 0,
                    }),
                    no_raw_tx: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap()
    }

    /// The pre-batching result assembly retained as a differential oracle.
    async fn assemble_per_transaction_reference(
        storage: &dyn StorageReader,
        _user_id: i64,
        transactions: &[Transaction],
        args: &ListActionsArgs,
    ) -> WalletResult<ListActionsResult> {
        let include_labels = *args.include_labels;
        let include_inputs = *args.include_inputs;
        let include_outputs = *args.include_outputs;
        let include_input_scripts = *args.include_input_source_locking_scripts;
        let include_output_scripts = *args.include_output_locking_scripts;
        let mut actions = Vec::with_capacity(transactions.len());

        for tx in transactions {
            let mut action = Action {
                txid: tx.txid.clone().unwrap_or_default(),
                satoshis: tx.satoshis,
                status: tx_status_to_action_status(&tx.status),
                is_outgoing: tx.is_outgoing,
                description: tx.description.clone(),
                version: tx.version.unwrap_or(0) as u32,
                lock_time: tx.lock_time.unwrap_or(0) as u32,
                labels: None,
                inputs: None,
                outputs: None,
            };

            if include_labels {
                let maps = storage
                    .find_tx_label_maps(
                        &FindTxLabelMapsArgs {
                            partial: TxLabelMapPartial {
                                transaction_id: Some(tx.transaction_id),
                                is_deleted: Some(false),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        None,
                    )
                    .await?;
                let mut labels = Vec::new();
                for map in maps {
                    if let Some(label) = storage.find_tx_label_by_id(map.tx_label_id, None).await? {
                        if !label.is_deleted {
                            labels.push(label.label);
                        }
                    }
                }
                action.labels = (!labels.is_empty()).then_some(labels);
            }

            if include_outputs {
                let outputs = storage
                    .find_outputs(
                        &FindOutputsArgs {
                            partial: OutputPartial {
                                transaction_id: Some(tx.transaction_id),
                                ..Default::default()
                            },
                            no_script: !include_output_scripts,
                            ..Default::default()
                        },
                        None,
                    )
                    .await?;
                for output in outputs {
                    action
                        .outputs
                        .get_or_insert_with(Vec::new)
                        .push(ActionOutput {
                            satoshis: output.satoshis as u64,
                            spendable: output.spendable,
                            locking_script: if include_output_scripts {
                                output.locking_script
                            } else {
                                None
                            },
                            custom_instructions: None,
                            tags: vec![],
                            output_index: output.vout as u32,
                            output_description: output.output_description.unwrap_or_default(),
                            basket: None,
                        });
                }
            }

            if include_inputs {
                let inputs = storage
                    .find_outputs(
                        &FindOutputsArgs {
                            partial: OutputPartial {
                                spent_by: Some(tx.transaction_id),
                                ..Default::default()
                            },
                            no_script: !include_input_scripts,
                            ..Default::default()
                        },
                        None,
                    )
                    .await?;
                for input in inputs {
                    action
                        .inputs
                        .get_or_insert_with(Vec::new)
                        .push(ActionInput {
                            source_outpoint: format!(
                                "{}.{}",
                                input.txid.as_deref().unwrap_or(""),
                                input.vout
                            ),
                            source_satoshis: input.satoshis as u64,
                            source_locking_script: if include_input_scripts {
                                input.locking_script
                            } else {
                                None
                            },
                            unlocking_script: None,
                            input_description: input.output_description.unwrap_or_default(),
                            sequence_number: input.sequence_number.unwrap_or(0) as u32,
                        });
                }
            }

            actions.push(action);
        }

        Ok(ListActionsResult {
            total_actions: transactions.len() as u32,
            actions,
        })
    }

    #[tokio::test]
    async fn enrichment_query_count_is_constant() {
        let fixture = setup_fixture().await;
        let args = list_args(true);
        let transactions = selected_transactions(&fixture.storage, fixture.user_id).await;

        fixture.storage.reset_sql_statement_count();
        assemble_per_transaction_reference(&fixture.storage, fixture.user_id, &transactions, &args)
            .await
            .unwrap();
        assert_eq!(fixture.storage.sql_statement_count(), 21);

        fixture.storage.reset_sql_statement_count();
        list_actions(&fixture.storage, "", fixture.user_id, &args, None)
            .await
            .unwrap();
        // One base transaction statement plus four enrichment statements.
        assert_eq!(fixture.storage.sql_statement_count(), 5);

        for (limit, expected_actions) in [(3, 1), (100, 2)] {
            let labelled_args: ListActionsArgs = serde_json::from_value(serde_json::json!({
                "labels": ["label-0", "early-mapped-late"],
                "includeLabels": true,
                "includeInputs": true,
                "includeOutputs": true,
                "limit": limit
            }))
            .unwrap();
            fixture.storage.reset_sql_statement_count();
            let result = list_actions(&fixture.storage, "", fixture.user_id, &labelled_args, None)
                .await
                .unwrap();
            assert_eq!(result.actions.len(), expected_actions);
            // One batched label-name resolution, one base read, one batched map
            // filter, and four enrichment reads: constant across page sizes.
            assert_eq!(fixture.storage.sql_statement_count(), 7);
        }

        let no_enrichment_args: ListActionsArgs =
            serde_json::from_value(serde_json::json!({ "labels": [], "limit": 100 })).unwrap();
        fixture.storage.reset_sql_statement_count();
        list_actions(
            &fixture.storage,
            "",
            fixture.user_id,
            &no_enrichment_args,
            None,
        )
        .await
        .unwrap();
        assert_eq!(fixture.storage.sql_statement_count(), 1);

        let empty_page_args: ListActionsArgs = serde_json::from_value(serde_json::json!({
            "labels": [],
            "includeLabels": true,
            "includeInputs": true,
            "includeOutputs": true,
            "limit": 100,
            "offset": 100
        }))
        .unwrap();
        fixture.storage.reset_sql_statement_count();
        list_actions(
            &fixture.storage,
            "",
            fixture.user_id,
            &empty_page_args,
            None,
        )
        .await
        .unwrap();
        assert_eq!(fixture.storage.sql_statement_count(), 1);
    }

    #[tokio::test]
    async fn enrichment_chunks_across_the_fixed_boundary() {
        let fixture = setup_chunk_boundary_fixture().await;
        for (limit, expected_actions, expected_statements) in [
            (ENRICHMENT_IN_CHUNK_SIZE as i64, ENRICHMENT_IN_CHUNK_SIZE, 6),
            (
                (ENRICHMENT_IN_CHUNK_SIZE + 2) as i64,
                ENRICHMENT_IN_CHUNK_SIZE + 1,
                9,
            ),
        ] {
            let args: ListActionsArgs = serde_json::from_value(serde_json::json!({
                "labels": [],
                "includeLabels": true,
                "includeInputs": true,
                "includeInputSourceLockingScripts": true,
                "includeOutputs": true,
                "includeOutputLockingScripts": true,
                "limit": limit
            }))
            .unwrap();
            let transactions = fixture
                .storage
                .find_transactions(
                    &FindTransactionsArgs {
                        partial: TransactionPartial {
                            user_id: Some(fixture.user_id),
                            ..Default::default()
                        },
                        status: Some(DEFAULT_STATI.to_vec()),
                        paged: Some(Paged { limit, offset: 0 }),
                        no_raw_tx: true,
                        ..Default::default()
                    },
                    None,
                )
                .await
                .unwrap();
            let mut expected = assemble_per_transaction_reference(
                &fixture.storage,
                fixture.user_id,
                &transactions,
                &args,
            )
            .await
            .unwrap();
            expected.total_actions = fixture.transaction_ids.len() as u32;

            fixture.storage.reset_sql_statement_count();
            let actual = list_actions(&fixture.storage, "", fixture.user_id, &args, None)
                .await
                .unwrap();

            assert_eq!(
                serde_json::to_value(&actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            assert_eq!(actual.actions.len(), expected_actions);
            assert_eq!(fixture.storage.sql_statement_count(), expected_statements);
        }

        let label_boundary_limit = 2;
        let label_boundary_offset = ENRICHMENT_IN_CHUNK_SIZE as i64;
        let label_boundary_args: ListActionsArgs = serde_json::from_value(serde_json::json!({
            "labels": [],
            "includeLabels": true,
            "limit": label_boundary_limit,
            "offset": label_boundary_offset
        }))
        .unwrap();
        let transactions = fixture
            .storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    status: Some(DEFAULT_STATI.to_vec()),
                    paged: Some(Paged {
                        limit: label_boundary_limit,
                        offset: label_boundary_offset,
                    }),
                    no_raw_tx: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let mut expected = assemble_per_transaction_reference(
            &fixture.storage,
            fixture.user_id,
            &transactions,
            &label_boundary_args,
        )
        .await
        .unwrap();
        expected.total_actions = fixture.transaction_ids.len() as u32;

        fixture.storage.reset_sql_statement_count();
        let actual = list_actions(
            &fixture.storage,
            "",
            fixture.user_id,
            &label_boundary_args,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(actual.actions.len(), 1);
        // One base statement, one transaction-id map chunk, and two label-id chunks.
        assert_eq!(fixture.storage.sql_statement_count(), 4);
    }

    #[tokio::test]
    async fn batched_assembly_matches_per_transaction_reference() {
        let fixture = setup_fixture().await;
        let transactions = selected_transactions(&fixture.storage, fixture.user_id).await;

        let all_user_outputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let selected_outputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    transaction_ids: Some(fixture.transaction_ids.clone()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let selected_inputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    spent_by_ids: Some(fixture.transaction_ids.clone()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(all_user_outputs.len(), 9);
        assert_eq!(selected_outputs.len(), 7);
        assert_eq!(selected_inputs.len(), 4);
        assert!(selected_outputs
            .iter()
            .all(|output| output.transaction_id != fixture.excluded_transaction_id));
        assert!(selected_inputs
            .iter()
            .all(|output| output.spent_by != Some(fixture.excluded_transaction_id)));

        for include_scripts in [false, true] {
            let args = list_args(include_scripts);
            let expected = assemble_per_transaction_reference(
                &fixture.storage,
                fixture.user_id,
                &transactions,
                &args,
            )
            .await
            .unwrap();
            let actual = list_actions(&fixture.storage, "", fixture.user_id, &args, None)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(&actual).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );

            assert_eq!(
                actual.actions[0].labels,
                Some(vec!["label-0".to_string(), "early-mapped-late".to_string()])
            );
            assert_eq!(actual.actions[1].labels, None);
            assert_eq!(actual.actions[2].labels, Some(vec!["label-2".to_string()]));
            assert_eq!(actual.actions[3].labels, None);
            assert_eq!(
                actual.actions[4].labels,
                Some(vec!["label-4".to_string(), "label-0".to_string()])
            );
            assert!(actual.actions[1].outputs.is_none());
            assert!(actual.actions[0].inputs.is_none());
            assert_eq!(actual.actions[1].inputs.as_ref().unwrap().len(), 2);
            assert_eq!(
                actual.actions[0]
                    .outputs
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|output| output.output_index)
                    .collect::<Vec<_>>(),
                vec![0, 1, 2]
            );
            assert!(actual.actions.iter().all(|action| action
                .outputs
                .iter()
                .flatten()
                .all(|output| output.custom_instructions.is_none())));

            let first_output_script = actual.actions[0]
                .outputs
                .as_ref()
                .unwrap()
                .iter()
                .find(|output| output.output_index == 0)
                .unwrap()
                .locking_script
                .clone();
            let first_input_script = actual.actions[1]
                .inputs
                .as_ref()
                .unwrap()
                .iter()
                .find(|input| input.source_outpoint == format!("{:064x}.2", 0))
                .unwrap()
                .source_locking_script
                .clone();
            if include_scripts {
                assert_eq!(first_output_script, Some(vec![0, 0]));
                assert_eq!(first_input_script, Some(vec![0, 2]));
            } else {
                assert_eq!(first_output_script, None);
                assert_eq!(first_input_script, None);
            }
        }
    }

    #[tokio::test]
    async fn empty_output_in_lists_match_nothing() {
        let fixture = setup_fixture().await;
        let by_transaction = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    transaction_ids: Some(Vec::new()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let by_spender = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    spent_by_ids: Some(Vec::new()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(by_transaction.is_empty());
        assert!(by_spender.is_empty());
        assert_eq!(fixture.transaction_ids.len(), 5);
    }

    #[tokio::test]
    async fn output_transaction_id_in_list_filters_rows() {
        let fixture = setup_fixture().await;
        let all_user_outputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let selected_transaction_id = fixture.transaction_ids[0];
        let selected_outputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    transaction_ids: Some(vec![selected_transaction_id]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        assert!(!selected_outputs.is_empty());
        assert!(selected_outputs.len() < all_user_outputs.len());
        assert!(selected_outputs
            .iter()
            .all(|output| output.transaction_id == selected_transaction_id));
    }

    #[tokio::test]
    async fn output_spent_by_in_list_filters_rows() {
        let fixture = setup_fixture().await;
        let all_user_outputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let selected_spender_id = fixture.transaction_ids[1];
        let selected_inputs = fixture
            .storage
            .find_outputs(
                &FindOutputsArgs {
                    partial: OutputPartial {
                        user_id: Some(fixture.user_id),
                        ..Default::default()
                    },
                    spent_by_ids: Some(vec![selected_spender_id]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(selected_inputs.len(), 2);
        assert!(selected_inputs.len() < all_user_outputs.len());
        assert!(selected_inputs
            .iter()
            .all(|output| output.spent_by == Some(selected_spender_id)));
    }

    #[tokio::test]
    async fn label_in_lists_filter_rows_and_empty_lists_match_nothing() {
        let fixture = setup_fixture().await;
        let selected_transaction_id = fixture.transaction_ids[0];
        let selected_maps = fixture
            .storage
            .find_tx_label_maps(
                &FindTxLabelMapsArgs {
                    transaction_ids: Some(vec![selected_transaction_id]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(!selected_maps.is_empty());
        assert!(selected_maps
            .iter()
            .all(|map| map.transaction_id == selected_transaction_id));

        let selected_label_id = selected_maps[0].tx_label_id;
        let selected_labels = fixture
            .storage
            .find_tx_labels(
                &FindTxLabelsArgs {
                    tx_label_ids: Some(vec![selected_label_id]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(selected_labels.len(), 1);
        assert_eq!(selected_labels[0].tx_label_id, selected_label_id);

        let empty_maps = fixture
            .storage
            .find_tx_label_maps(
                &FindTxLabelMapsArgs {
                    transaction_ids: Some(Vec::new()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let empty_labels = fixture
            .storage
            .find_tx_labels(
                &FindTxLabelsArgs {
                    tx_label_ids: Some(Vec::new()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(empty_maps.is_empty());
        assert!(empty_labels.is_empty());
    }

    #[tokio::test]
    async fn base_reference_leaks_cross_tenant_rows_batched_path_is_scoped() {
        let fixture = setup_fixture().await;
        let timestamp = now();
        let other_user_id = fixture
            .storage
            .insert_user(
                &User {
                    created_at: timestamp,
                    updated_at: timestamp,
                    user_id: 0,
                    identity_key: "other-list-actions-user".to_string(),
                    active_storage: String::new(),
                },
                None,
            )
            .await
            .unwrap();
        let foreign_label_id = fixture
            .storage
            .insert_tx_label(
                &TxLabel {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: 0,
                    user_id: other_user_id,
                    label: "foreign-label".to_string(),
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();
        fixture
            .storage
            .insert_tx_label_map(
                &TxLabelMap {
                    created_at: timestamp,
                    updated_at: timestamp,
                    tx_label_id: foreign_label_id,
                    transaction_id: fixture.transaction_ids[0],
                    is_deleted: false,
                },
                None,
            )
            .await
            .unwrap();

        for (description, transaction_id, spent_by, vout) in [
            ("foreign-output", fixture.transaction_ids[0], None, 90),
            (
                "foreign-input",
                fixture.excluded_transaction_id,
                Some(fixture.transaction_ids[1]),
                91,
            ),
        ] {
            fixture
                .storage
                .insert_output(
                    &Output {
                        created_at: timestamp,
                        updated_at: timestamp,
                        output_id: 0,
                        user_id: other_user_id,
                        transaction_id,
                        basket_id: None,
                        spendable: spent_by.is_none(),
                        change: false,
                        output_description: Some(description.to_string()),
                        vout,
                        satoshis: 1,
                        provided_by: StorageProvidedBy::Storage,
                        purpose: "test".to_string(),
                        output_type: "P2PKH".to_string(),
                        txid: Some(format!("{:064x}", 98)),
                        sender_identity_key: None,
                        derivation_prefix: None,
                        derivation_suffix: None,
                        custom_instructions: None,
                        spent_by,
                        sequence_number: Some(1),
                        spending_description: None,
                        script_length: Some(2),
                        script_offset: None,
                        locking_script: Some(vec![98, vout as u8]),
                    },
                    None,
                )
                .await
                .unwrap();
        }

        let transactions = selected_transactions(&fixture.storage, fixture.user_id).await;
        let reference = assemble_per_transaction_reference(
            &fixture.storage,
            fixture.user_id,
            &transactions,
            &list_args(true),
        )
        .await
        .unwrap();
        assert!(reference.actions.iter().any(|action| action
            .labels
            .iter()
            .flatten()
            .any(|label| label == "foreign-label")));
        assert!(reference.actions.iter().any(|action| action
            .outputs
            .iter()
            .flatten()
            .any(|output| output.output_description == "foreign-output")));
        assert!(reference.actions.iter().any(|action| action
            .inputs
            .iter()
            .flatten()
            .any(|input| input.input_description == "foreign-input")));

        let actual = list_actions(
            &fixture.storage,
            "",
            fixture.user_id,
            &list_args(true),
            None,
        )
        .await
        .unwrap();
        assert!(actual.actions.iter().all(|action| action
            .labels
            .iter()
            .flatten()
            .all(|label| label != "foreign-label")));
        assert!(actual.actions.iter().all(|action| action
            .outputs
            .iter()
            .flatten()
            .all(|output| output.output_description != "foreign-output")));
        assert!(actual.actions.iter().all(|action| action
            .inputs
            .iter()
            .flatten()
            .all(|input| input.input_description != "foreign-input")));
    }
}
