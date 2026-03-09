//! listOutputs query with BEEF construction and specOp handling.
//!
//! Ported from wallet-toolbox/src/storage/methods/listOutputsKnex.ts.
//! Translates bsv-sdk `ListOutputsArgs` into low-level storage queries,
//! returning `ListOutputsResult` with optional BEEF provenance.

use bsv::wallet::interfaces::{
    ListOutputsArgs, ListOutputsResult, Output as SdkOutput, OutputInclude, QueryMode,
};

use crate::error::WalletResult;
use crate::status::TransactionStatus;
use crate::storage::find_args::{
    FindOutputBasketsArgs, FindOutputTagMapsArgs, FindOutputTagsArgs, FindOutputsArgs,
    FindTxLabelMapsArgs, OutputBasketPartial, OutputPartial,
    OutputTagMapPartial, OutputTagPartial, Paged, TxLabelMapPartial,
};
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::TrxToken;
use crate::tables::Output;
use crate::wallet::types::{
    SPEC_OP_INVALID_CHANGE, SPEC_OP_SET_WALLET_CHANGE_PARAMS, SPEC_OP_WALLET_BALANCE,
};

// ---------------------------------------------------------------------------
// SpecOp dispatch for list_outputs
// ---------------------------------------------------------------------------

/// Special operations routed through list_outputs via basket name overloading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOutputsSpecOp {
    /// Sum spendable satoshis in the effective basket.
    WalletBalance,
    /// Check UTXOs against network and tag invalid ones.
    InvalidChange,
    /// Update change output parameters on the default basket.
    SetChangeParams,
}

/// Resolve whether a list_outputs call is a specOp based on basket name or tags.
///
/// Returns (specOp, effective_basket, effective_tags). If no specOp is detected,
/// `specOp` is `None` and the basket/tags are returned unchanged.
pub fn resolve_list_outputs_spec_op(
    basket: &str,
    tags: &[String],
) -> (Option<ListOutputsSpecOp>, String, Vec<String>) {
    // Check basket-based specOps first
    match basket {
        b if b == SPEC_OP_WALLET_BALANCE => {
            return (
                Some(ListOutputsSpecOp::WalletBalance),
                "default".to_string(),
                tags.to_vec(),
            );
        }
        b if b == SPEC_OP_INVALID_CHANGE => {
            return (
                Some(ListOutputsSpecOp::InvalidChange),
                "default".to_string(),
                tags.to_vec(),
            );
        }
        b if b == SPEC_OP_SET_WALLET_CHANGE_PARAMS => {
            return (
                Some(ListOutputsSpecOp::SetChangeParams),
                "default".to_string(),
                tags.to_vec(),
            );
        }
        _ => {}
    }

    // Check tag-based specOps
    if tags.iter().any(|t| t == SPEC_OP_WALLET_BALANCE) {
        let filtered: Vec<String> = tags
            .iter()
            .filter(|t| t.as_str() != SPEC_OP_WALLET_BALANCE)
            .cloned()
            .collect();
        return (
            Some(ListOutputsSpecOp::WalletBalance),
            basket.to_string(),
            filtered,
        );
    }

    (None, basket.to_string(), tags.to_vec())
}

/// Handle the WalletBalance specOp: sum satoshis of all spendable outputs.
async fn handle_wallet_balance(
    storage: &dyn StorageReader,
    user_id: i64,
    basket_name: &str,
    trx: Option<&TrxToken>,
) -> WalletResult<ListOutputsResult> {
    let baskets = storage
        .find_output_baskets(
            &FindOutputBasketsArgs {
                partial: OutputBasketPartial {
                    user_id: Some(user_id),
                    name: Some(basket_name.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if baskets.is_empty() {
        return Ok(ListOutputsResult {
            total_outputs: 0,
            beef: None,
            outputs: vec![],
        });
    }

    let basket_id = baskets[0].basket_id;

    let outputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_id),
                    basket_id: Some(basket_id),
                    spendable: Some(true),
                    ..Default::default()
                },
                tx_status: Some(TX_STATUS_ALLOWED.to_vec()),
                no_script: true,
                ..Default::default()
            },
            trx,
        )
        .await?;

    let total_satoshis: u64 = outputs.iter().map(|o| o.satoshis as u64).sum();

    let sdk_outputs: Vec<SdkOutput> = outputs
        .iter()
        .map(|o| SdkOutput {
            satoshis: o.satoshis as u64,
            spendable: o.spendable,
            outpoint: format!("{}.{}", o.txid.as_deref().unwrap_or(""), o.vout),
            locking_script: None,
            custom_instructions: None,
            tags: vec![],
            labels: vec![],
        })
        .collect();

    Ok(ListOutputsResult {
        total_outputs: total_satoshis as u32,
        beef: None,
        outputs: sdk_outputs,
    })
}

/// Handle the InvalidChange specOp: identify and optionally release invalid outputs.
///
/// This is a simplified version -- full network UTXO checking would require
/// WalletServices access. Returns all change outputs for review.
async fn handle_invalid_change(
    storage: &dyn StorageReader,
    user_id: i64,
    basket_name: &str,
    tags: &[String],
    trx: Option<&TrxToken>,
) -> WalletResult<ListOutputsResult> {
    let _release = tags.iter().any(|t| t == "release");
    let _check_all = tags.iter().any(|t| t == "all");

    let baskets = storage
        .find_output_baskets(
            &FindOutputBasketsArgs {
                partial: OutputBasketPartial {
                    user_id: Some(user_id),
                    name: Some(basket_name.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if baskets.is_empty() {
        return Ok(ListOutputsResult {
            total_outputs: 0,
            beef: None,
            outputs: vec![],
        });
    }

    let basket_id = baskets[0].basket_id;

    let outputs = storage
        .find_outputs(
            &FindOutputsArgs {
                partial: OutputPartial {
                    user_id: Some(user_id),
                    basket_id: Some(basket_id),
                    spendable: Some(true),
                    ..Default::default()
                },
                tx_status: Some(TX_STATUS_ALLOWED.to_vec()),
                no_script: true,
                ..Default::default()
            },
            trx,
        )
        .await?;

    let sdk_outputs: Vec<SdkOutput> = outputs
        .iter()
        .map(|o| SdkOutput {
            satoshis: o.satoshis as u64,
            spendable: o.spendable,
            outpoint: format!("{}.{}", o.txid.as_deref().unwrap_or(""), o.vout),
            locking_script: None,
            custom_instructions: None,
            tags: vec![],
            labels: vec![],
        })
        .collect();

    Ok(ListOutputsResult {
        total_outputs: sdk_outputs.len() as u32,
        beef: None,
        outputs: sdk_outputs,
    })
}

/// Handle the SetChangeParams specOp: update change parameters on the default basket.
async fn handle_set_change_params(
    storage: &dyn StorageReaderWriter,
    user_id: i64,
    tags: &[String],
    trx: Option<&TrxToken>,
) -> WalletResult<ListOutputsResult> {
    let count: i64 = tags
        .first()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let satoshis: i64 = tags
        .get(1)
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);

    let baskets = storage
        .find_output_baskets(
            &FindOutputBasketsArgs {
                partial: OutputBasketPartial {
                    user_id: Some(user_id),
                    name: Some("default".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if let Some(basket) = baskets.first() {
        storage
            .update_output_basket(
                basket.basket_id,
                &OutputBasketPartial {
                    number_of_desired_utxos: Some(count),
                    minimum_desired_utxo_value: Some(satoshis),
                    ..Default::default()
                },
                trx,
            )
            .await?;
    }

    Ok(ListOutputsResult {
        total_outputs: 0,
        beef: None,
        outputs: vec![],
    })
}

/// Execute a listOutputs query with specOp support against a read-write storage backend.
///
/// This version takes `StorageReaderWriter` to support SetChangeParams specOp
/// which needs write access.
pub async fn list_outputs_rw(
    storage: &dyn StorageReaderWriter,
    auth: &str,
    user_id: i64,
    args: &ListOutputsArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<ListOutputsResult> {
    let (spec_op, effective_basket, effective_tags) =
        resolve_list_outputs_spec_op(&args.basket, &args.tags);

    if let Some(op) = spec_op {
        return match op {
            ListOutputsSpecOp::WalletBalance => {
                handle_wallet_balance(storage, user_id, &effective_basket, trx).await
            }
            ListOutputsSpecOp::InvalidChange => {
                handle_invalid_change(storage, user_id, &effective_basket, &effective_tags, trx)
                    .await
            }
            ListOutputsSpecOp::SetChangeParams => {
                handle_set_change_params(storage, user_id, &effective_tags, trx).await
            }
        };
    }

    // Fall through to normal query path
    list_outputs(storage, auth, user_id, args, trx).await
}

/// Allowed transaction statuses for outputs to be visible in list results.
const TX_STATUS_ALLOWED: &[TransactionStatus] = &[
    TransactionStatus::Completed,
    TransactionStatus::Unproven,
    TransactionStatus::Nosend,
    TransactionStatus::Sending,
];

/// Execute a listOutputs query against the given storage reader.
///
/// Translates the bsv-sdk `ListOutputsArgs` to low-level find/count calls,
/// applying basket filtering, tag filtering, pagination, and optional includes.
pub async fn list_outputs(
    storage: &dyn StorageReader,
    _auth: &str,
    user_id: i64,
    args: &ListOutputsArgs,
    trx: Option<&TrxToken>,
) -> WalletResult<ListOutputsResult> {
    let limit = args.limit.unwrap_or(10) as i64;
    let raw_offset = args.offset.map(|o| o as i64).unwrap_or(0);
    let offset = if raw_offset < 0 {
        (-raw_offset) - 1
    } else {
        raw_offset
    };

    let include_locking_scripts = matches!(args.include, Some(OutputInclude::LockingScripts))
        || matches!(args.include, Some(OutputInclude::EntireTransactions));
    let include_transactions = matches!(args.include, Some(OutputInclude::EntireTransactions));
    let include_custom_instructions = *args.include_custom_instructions;
    let include_tags_flag = *args.include_tags;
    let include_labels_flag = *args.include_labels;

    let is_query_mode_all = matches!(args.tag_query_mode, Some(QueryMode::All));

    // Resolve basket name to basket_id
    let basket_name = &args.basket;
    let mut basket_id: Option<i64> = None;

    if !basket_name.is_empty() {
        let baskets = storage
            .find_output_baskets(
                &FindOutputBasketsArgs {
                    partial: OutputBasketPartial {
                        user_id: Some(user_id),
                        name: Some(basket_name.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trx,
            )
            .await?;

        if baskets.len() != 1 {
            // Basket not found -- return empty result
            return Ok(ListOutputsResult {
                total_outputs: 0,
                beef: None,
                outputs: vec![],
            });
        }
        basket_id = Some(baskets[0].basket_id);
    }

    // Resolve tag names to tag IDs
    let tags = &args.tags;
    let mut tag_ids: Vec<i64> = Vec::new();

    if !tags.is_empty() {
        for tag in tags {
            let found = storage
                .find_output_tags(
                    &FindOutputTagsArgs {
                        partial: OutputTagPartial {
                            user_id: Some(user_id),
                            tag: Some(tag.clone()),
                            is_deleted: Some(false),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    trx,
                )
                .await?;
            for ot in found {
                tag_ids.push(ot.output_tag_id);
            }
        }
    }

    // Short-circuit: "all" mode requires all tags present
    if is_query_mode_all && tag_ids.len() < tags.len() {
        return Ok(ListOutputsResult {
            total_outputs: 0,
            beef: None,
            outputs: vec![],
        });
    }

    // Short-circuit: "any" mode with no matching tags
    if !is_query_mode_all && tag_ids.is_empty() && !tags.is_empty() {
        return Ok(ListOutputsResult {
            total_outputs: 0,
            beef: None,
            outputs: vec![],
        });
    }

    // Build the base output query
    let find_args = FindOutputsArgs {
        partial: OutputPartial {
            user_id: Some(user_id),
            basket_id,
            spendable: Some(true),
            ..Default::default()
        },
        tx_status: Some(TX_STATUS_ALLOWED.to_vec()),
        paged: Some(Paged { limit, offset }),
        no_script: !include_locking_scripts,
        ..Default::default()
    };

    let all_outputs = storage.find_outputs(&find_args, trx).await?;

    // Filter by tags if needed (post-query filtering)
    let filtered_outputs: Vec<Output> = if tag_ids.is_empty() {
        all_outputs
    } else {
        let mut result = Vec::new();
        for o in all_outputs {
            let maps = storage
                .find_output_tag_maps(
                    &FindOutputTagMapsArgs {
                        partial: OutputTagMapPartial {
                            output_id: Some(o.output_id),
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
                .filter(|m| tag_ids.contains(&m.output_tag_id))
                .count();

            if is_query_mode_all {
                if matching_count >= tag_ids.len() {
                    result.push(o);
                }
            } else if matching_count > 0 {
                result.push(o);
            }
        }
        result
    };

    // Get total count for pagination
    let total_outputs = if filtered_outputs.len() < limit as usize {
        (offset as u32) + filtered_outputs.len() as u32
    } else {
        let count_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                basket_id,
                spendable: Some(true),
                ..Default::default()
            },
            tx_status: Some(TX_STATUS_ALLOWED.to_vec()),
            no_script: true,
            ..Default::default()
        };
        let total = storage.count_outputs(&count_args, trx).await?;
        total as u32
    };

    // Build SdkOutput results
    let mut outputs: Vec<SdkOutput> = Vec::with_capacity(filtered_outputs.len());

    for o in &filtered_outputs {
        let outpoint = format!("{}.{}", o.txid.as_deref().unwrap_or(""), o.vout);

        let mut sdk_output = SdkOutput {
            satoshis: o.satoshis as u64,
            spendable: o.spendable,
            outpoint,
            locking_script: if include_locking_scripts {
                o.locking_script.clone()
            } else {
                None
            },
            custom_instructions: if include_custom_instructions {
                o.custom_instructions.clone()
            } else {
                None
            },
            tags: vec![],
            labels: vec![],
        };

        if include_tags_flag {
            sdk_output.tags = get_tags_for_output(storage, o.output_id, trx).await?;
        }

        if include_labels_flag {
            sdk_output.labels =
                get_labels_for_transaction_id(storage, user_id, o.transaction_id, trx).await?;
        }

        outputs.push(sdk_output);
    }

    // BEEF construction (placeholder -- requires full BEEF assembly from beef.rs)
    let beef = if include_transactions {
        // TODO: Full BEEF assembly using beef.rs getBeefForTransaction
        // For now we return None; the Wallet layer can fill this in later
        None
    } else {
        None
    };

    Ok(ListOutputsResult {
        total_outputs,
        beef,
        outputs,
    })
}

/// Fetch tag names for an output by joining output_tag_maps and output_tags.
async fn get_tags_for_output(
    storage: &dyn StorageReader,
    output_id: i64,
    trx: Option<&TrxToken>,
) -> WalletResult<Vec<String>> {
    let maps = storage
        .find_output_tag_maps(
            &FindOutputTagMapsArgs {
                partial: OutputTagMapPartial {
                    output_id: Some(output_id),
                    is_deleted: Some(false),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    let mut tags = Vec::new();
    for m in &maps {
        let found = storage
            .find_output_tags(
                &FindOutputTagsArgs {
                    partial: OutputTagPartial {
                        output_tag_id: Some(m.output_tag_id),
                        is_deleted: Some(false),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trx,
            )
            .await?;
        for t in found {
            tags.push(t.tag);
        }
    }
    Ok(tags)
}

/// Fetch label names for a transaction by joining tx_label_maps and tx_labels.
async fn get_labels_for_transaction_id(
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
