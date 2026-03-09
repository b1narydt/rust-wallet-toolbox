//! listActions query translation from WalletInterface args to storage find args.
//!
//! Ported from wallet-toolbox/src/storage/methods/listActionsKnex.ts.
//! Translates bsv-sdk `ListActionsArgs` into low-level `find_*` / `count_*`
//! calls on `StorageReader`, returning `ListActionsResult`.

use bsv::wallet::interfaces::{
    Action, ActionInput, ActionOutput, ActionStatus, ListActionsArgs, ListActionsResult, QueryMode,
};

use crate::error::WalletResult;
use crate::status::TransactionStatus;
use crate::storage::find_args::{
    FindOutputsArgs, FindTransactionsArgs, FindTxLabelMapsArgs, FindTxLabelsArgs,
    OutputPartial, Paged, TransactionPartial, TxLabelMapPartial, TxLabelPartial,
};
use crate::storage::traits::reader::StorageReader;
use crate::storage::TrxToken;
use crate::tables::Transaction;
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
pub fn resolve_list_actions_spec_op(
    labels: &[String],
) -> (Option<ListActionsSpecOp>, Vec<String>) {
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
            labels: vec![],
            inputs: vec![],
            outputs: vec![],
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
            labels: vec![],
            inputs: vec![],
            outputs: vec![],
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
            ListActionsSpecOp::NoSendActions => {
                handle_no_send_actions(storage, user_id, trx).await
            }
            ListActionsSpecOp::FailedActions => {
                handle_failed_actions(storage, user_id, trx).await
            }
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

/// Execute a listActions query against the given storage reader.
///
/// Translates the bsv-sdk `ListActionsArgs` to low-level find/count calls,
/// applying label filtering, pagination, and optional include flags.
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
    let include_input_source_locking_scripts =
        *args.include_input_source_locking_scripts;
    let include_output_locking_scripts = *args.include_output_locking_scripts;

    let is_query_mode_all = matches!(args.label_query_mode, Some(QueryMode::All));

    // Resolve label names to label IDs via storage
    let labels = &args.labels;
    let mut label_ids: Vec<i64> = Vec::new();

    if !labels.is_empty() {
        for label in labels {
            let found = storage
                .find_tx_labels(
                    &FindTxLabelsArgs {
                        partial: TxLabelPartial {
                            user_id: Some(user_id),
                            label: Some(label.clone()),
                            is_deleted: Some(false),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for tl in found {
                label_ids.push(tl.tx_label_id);
            }
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
        let mut result = Vec::new();
        for tx in txs {
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
                    trx,
                )
                .await?;

            let matching_count = maps
                .iter()
                .filter(|m| label_ids.contains(&m.tx_label_id))
                .count();

            if is_query_mode_all {
                if matching_count >= label_ids.len() {
                    result.push(tx);
                }
            } else if matching_count > 0 {
                result.push(tx);
            }
        }
        result
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

    // Build Action results
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
            labels: vec![],
            inputs: vec![],
            outputs: vec![],
        };

        if include_labels {
            action.labels = get_labels_for_transaction(storage, user_id, tx.transaction_id, trx).await?;
        }

        if include_outputs {
            let outputs = storage
                .find_outputs(
                    &FindOutputsArgs {
                        partial: OutputPartial {
                            transaction_id: Some(tx.transaction_id),
                            ..Default::default()
                        },
                        no_script: !include_output_locking_scripts,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            for o in &outputs {
                let ao = ActionOutput {
                    satoshis: o.satoshis as u64,
                    spendable: o.spendable,
                    locking_script: if include_output_locking_scripts {
                        o.locking_script.clone()
                    } else {
                        None
                    },
                    custom_instructions: None, // stripped for security
                    tags: vec![],
                    output_index: o.vout as u32,
                    output_description: o.output_description.clone().unwrap_or_default(),
                    basket: None,
                };
                action.outputs.push(ao);
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
                        no_script: !include_input_source_locking_scripts,
                        ..Default::default()
                    },
                    trx,
                )
                .await?;

            for o in &inputs {
                let ai = ActionInput {
                    source_outpoint: format!(
                        "{}.{}",
                        o.txid.as_deref().unwrap_or(""),
                        o.vout
                    ),
                    source_satoshis: o.satoshis as u64,
                    source_locking_script: if include_input_source_locking_scripts {
                        o.locking_script.clone()
                    } else {
                        None
                    },
                    unlocking_script: None,
                    input_description: o.output_description.clone().unwrap_or_default(),
                    sequence_number: o.sequence_number.unwrap_or(0) as u32,
                };
                action.inputs.push(ai);
            }
        }

        actions.push(action);
    }

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

/// Fetch label names for a transaction by joining tx_label_maps and tx_labels.
async fn get_labels_for_transaction(
    storage: &dyn StorageReader,
    _user_id: i64,
    transaction_id: i64,
    trx: Option<&TrxToken>,
) -> WalletResult<Vec<String>> {
    let maps = storage
        .find_tx_label_maps(
            &FindTxLabelMapsArgs {
                partial: TxLabelMapPartial {
                    transaction_id: Some(transaction_id),
                    is_deleted: Some(false),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    let mut labels = Vec::new();
    for m in &maps {
        if let Some(tl) = storage.find_tx_label_by_id(m.tx_label_id, trx).await? {
            if !tl.is_deleted {
                labels.push(tl.label);
            }
        }
    }
    Ok(labels)
}
