//! Change generation algorithm (generateChangeSdk) ported from TypeScript.
//!
//! Ported from wallet-toolbox/src/storage/methods/generateChange.ts.
//! Handles UTXO selection, change output creation, starvation loops,
//! fee calculation, and excess distribution.

use crate::error::{WalletError, WalletResult};
use crate::storage::action_types::{
    AllocatedChangeInputRef, ChangeOutput, GenerateChangeSdkArgs, GenerateChangeSdkResult,
    MaxPossibleSatoshisAdjustment,
};
use crate::utility::tx_size::transaction_size;

/// Sentinel value: an output with this satoshis amount will be adjusted
/// to the largest fundable amount.
pub const MAX_POSSIBLE_SATOSHIS: u64 = 2_099_999_999_999_999;

/// A pre-fetched available change UTXO for the allocator.
#[derive(Debug, Clone)]
pub struct AvailableChange {
    /// Storage output ID.
    pub output_id: i64,
    /// Value in satoshis.
    pub satoshis: u64,
    /// Whether this output is currently spendable.
    pub spendable: bool,
}

/// In-memory UTXO allocator matching the TS `generateChangeSdkMakeStorage`.
/// Sorts by satoshis ascending, then output_id ascending (prefer smaller).
pub struct ChangeStorage {
    change: Vec<AvailableChange>,
}

impl ChangeStorage {
    /// Create a new ChangeStorage from available change UTXOs.
    pub fn new(available: Vec<AvailableChange>) -> Self {
        let mut change: Vec<AvailableChange> = available
            .into_iter()
            .map(|c| AvailableChange {
                output_id: c.output_id,
                satoshis: c.satoshis,
                spendable: true,
            })
            .collect();
        change.sort_by(|a, b| {
            a.satoshis
                .cmp(&b.satoshis)
                .then(a.output_id.cmp(&b.output_id))
        });
        Self { change }
    }

    /// Allocate a change input. Tries exact match first, then smallest >= target,
    /// then largest available (fallback).
    pub fn allocate(
        &mut self,
        target_satoshis: u64,
        exact_satoshis: Option<u64>,
    ) -> Option<AllocatedChangeInputRef> {
        // Try exact match first
        if let Some(exact) = exact_satoshis {
            if let Some(idx) = self
                .change
                .iter()
                .position(|c| c.spendable && c.satoshis == exact)
            {
                self.change[idx].spendable = false;
                return Some(AllocatedChangeInputRef {
                    output_id: self.change[idx].output_id,
                    satoshis: self.change[idx].satoshis,
                });
            }
        }

        // Try smallest >= target
        if let Some(idx) = self
            .change
            .iter()
            .position(|c| c.spendable && c.satoshis >= target_satoshis)
        {
            self.change[idx].spendable = false;
            return Some(AllocatedChangeInputRef {
                output_id: self.change[idx].output_id,
                satoshis: self.change[idx].satoshis,
            });
        }

        // Fallback: largest available (iterate backwards)
        for i in (0..self.change.len()).rev() {
            if self.change[i].spendable {
                self.change[i].spendable = false;
                return Some(AllocatedChangeInputRef {
                    output_id: self.change[i].output_id,
                    satoshis: self.change[i].satoshis,
                });
            }
        }

        None
    }

    /// Release a previously allocated change input back to the pool.
    pub fn release(&mut self, output_id: i64) {
        if let Some(c) = self.change.iter_mut().find(|c| c.output_id == output_id) {
            c.spendable = true;
        }
    }
}

/// Validate the params for generate_change_sdk.
/// Returns the index of a fixedOutput with MAX_POSSIBLE_SATOSHIS if any.
fn validate_params(args: &GenerateChangeSdkArgs) -> WalletResult<Option<usize>> {
    if args.fee_model.model != "sat/kb" {
        return Err(WalletError::InvalidParameter {
            parameter: "fee_model.model".to_string(),
            must_be: "'sat/kb'".to_string(),
        });
    }

    let mut has_max_possible_output: Option<usize> = None;
    for (i, o) in args.fixed_outputs.iter().enumerate() {
        if o.satoshis == MAX_POSSIBLE_SATOSHIS {
            if has_max_possible_output.is_some() {
                return Err(WalletError::InvalidParameter {
                    parameter: format!("fixed_outputs[{}].satoshis", i),
                    must_be: "valid satoshis amount. Only one 'maxPossibleSatoshis' output allowed."
                        .to_string(),
                });
            }
            has_max_possible_output = Some(i);
        }
    }

    Ok(has_max_possible_output)
}

/// Compute transaction size given current state.
fn compute_size(
    args: &GenerateChangeSdkArgs,
    allocated_len: usize,
    change_len: usize,
    added_inputs: usize,
    added_outputs: usize,
) -> usize {
    let input_script_lengths: Vec<usize> = args
        .fixed_inputs
        .iter()
        .map(|x| x.unlocking_script_length)
        .chain(
            std::iter::repeat(args.change_unlocking_script_length)
                .take(allocated_len + added_inputs),
        )
        .collect();
    let output_script_lengths: Vec<usize> = args
        .fixed_outputs
        .iter()
        .map(|x| x.locking_script_length)
        .chain(
            std::iter::repeat(args.change_locking_script_length)
                .take(change_len + added_outputs),
        )
        .collect();
    transaction_size(&input_script_lengths, &output_script_lengths)
}

/// Compute the target fee for a given state.
fn fee_target(
    args: &GenerateChangeSdkArgs,
    allocated_len: usize,
    change_len: usize,
    added_inputs: usize,
    added_outputs: usize,
) -> u64 {
    let sz = compute_size(args, allocated_len, change_len, added_inputs, added_outputs);
    // ceil(size * sats_per_kb / 1000)
    ((sz as u64) * args.fee_model.value + 999) / 1000
}

/// Sum of fixed input satoshis plus allocated change input satoshis.
fn funding(args: &GenerateChangeSdkArgs, allocated: &[AllocatedChangeInputRef]) -> u64 {
    let fixed_sum: u64 = args.fixed_inputs.iter().map(|i| i.satoshis).sum();
    let change_sum: u64 = allocated.iter().map(|i| i.satoshis).sum();
    fixed_sum + change_sum
}

/// Sum of fixed output satoshis.
fn spending(fixed_output_satoshis: &[u64]) -> u64 {
    fixed_output_satoshis.iter().sum()
}

/// Sum of change output satoshis.
fn change_total(outputs: &[ChangeOutput]) -> u64 {
    outputs.iter().map(|o| o.satoshis).sum()
}

/// Compute the excess fee (positive = overfunded, negative = underfunded).
fn fee_excess(
    args: &GenerateChangeSdkArgs,
    allocated: &[AllocatedChangeInputRef],
    change_outputs: &[ChangeOutput],
    fixed_output_satoshis: &[u64],
    added_inputs: usize,
    added_outputs: usize,
) -> i64 {
    let f = funding(args, allocated) as i64;
    let s = spending(fixed_output_satoshis) as i64;
    let c = change_total(change_outputs) as i64;
    let ft = fee_target(
        args,
        allocated.len(),
        change_outputs.len(),
        added_inputs,
        added_outputs,
    ) as i64;
    f - s - c - ft
}

/// Release all allocated change inputs back to the storage pool.
fn release_all(allocated: &mut Vec<AllocatedChangeInputRef>, storage: &mut ChangeStorage) {
    while let Some(input) = allocated.pop() {
        storage.release(input.output_id);
    }
}

/// Core change generation algorithm.
///
/// Synchronous pure computation function. All storage interactions
/// (fetching available UTXOs) must happen before calling this function.
///
/// Faithfully ported from TS `generateChangeSdk` in generateChange.ts.
pub fn generate_change_sdk(
    args: &GenerateChangeSdkArgs,
    available_change: &[AvailableChange],
) -> WalletResult<GenerateChangeSdkResult> {
    let has_max_possible_output = validate_params(args)?;

    let sats_per_kb = args.fee_model.value;
    let mut storage = ChangeStorage::new(available_change.to_vec());

    let mut allocated_change_inputs: Vec<AllocatedChangeInputRef> = Vec::new();
    let mut change_outputs: Vec<ChangeOutput> = Vec::new();

    // Mutable copy of fixed output satoshis for max_possible adjustment
    let mut fixed_output_satoshis: Vec<u64> =
        args.fixed_outputs.iter().map(|o| o.satoshis).collect();

    // Compute initial fee excess
    let mut fee_excess_now = fee_excess(
        args,
        &allocated_change_inputs,
        &change_outputs,
        &fixed_output_satoshis,
        0,
        0,
    );

    let has_target_net_count = args.target_net_count.is_some();
    let target_net_count = args.target_net_count.unwrap_or(0);

    // Net change count = change outputs created - change inputs consumed
    let net_change_count =
        |co_len: usize, ai_len: usize| -> i64 { co_len as i64 - ai_len as i64 };

    // Whether we should add a change output to balance a new input
    let should_add_output = |has_tnc: bool, co_len: usize, ai_len: usize| -> bool {
        if !has_tnc {
            return false;
        }
        net_change_count(co_len, ai_len) - 1 < target_net_count
    };

    // If we want more change outputs, create them now.
    // They may be removed if we cannot fund them.
    while (has_target_net_count
        && target_net_count
            > net_change_count(change_outputs.len(), allocated_change_inputs.len()))
        || (change_outputs.is_empty()
            && fee_excess(
                args,
                &allocated_change_inputs,
                &change_outputs,
                &fixed_output_satoshis,
                0,
                0,
            ) > 0)
    {
        let sats = if change_outputs.is_empty() {
            args.change_first_satoshis
        } else {
            args.change_initial_satoshis
        };
        change_outputs.push(ChangeOutput { satoshis: sats });
    }

    // === STARVATION LOOP ===
    // Outer loop: releases all inputs, funds, drops outputs if needed, retries.
    // NOTE: removing_outputs declared OUTSIDE the loop (matches TS behavior).
    // Once true, it stays true for all subsequent iterations.
    let mut removing_outputs = false;
    loop {
        release_all(&mut allocated_change_inputs, &mut storage);
        fee_excess_now = fee_excess(
            args,
            &allocated_change_inputs,
            &change_outputs,
            &fixed_output_satoshis,
            0,
            0,
        );

        // Inner funding loop: add one change input at a time
        while fee_excess(
            args,
            &allocated_change_inputs,
            &change_outputs,
            &fixed_output_satoshis,
            0,
            0,
        ) < 0
        {
            // Attempt to fund: compute target satoshis needed
            let mut exact_satoshis: Option<u64> = None;
            if !has_target_net_count && change_outputs.is_empty() {
                let deficit = -fee_excess(
                    args,
                    &allocated_change_inputs,
                    &change_outputs,
                    &fixed_output_satoshis,
                    1,
                    0,
                );
                if deficit > 0 {
                    exact_satoshis = Some(deficit as u64);
                }
            }

            let ao: usize = if should_add_output(
                has_target_net_count,
                change_outputs.len(),
                allocated_change_inputs.len(),
            ) {
                1
            } else {
                0
            };

            let target_satoshis = {
                let deficit = -fee_excess(
                    args,
                    &allocated_change_inputs,
                    &change_outputs,
                    &fixed_output_satoshis,
                    1,
                    ao,
                );
                let extra = if ao == 1 {
                    2 * args.change_initial_satoshis
                } else {
                    0
                };
                if deficit > 0 {
                    deficit as u64 + extra
                } else {
                    extra
                }
            };

            match storage.allocate(target_satoshis, exact_satoshis) {
                None => break, // No more funding available
                Some(input) => {
                    allocated_change_inputs.push(input);

                    let current_excess = fee_excess(
                        args,
                        &allocated_change_inputs,
                        &change_outputs,
                        &fixed_output_satoshis,
                        0,
                        0,
                    );

                    if !removing_outputs && current_excess > 0 {
                        if ao == 1 || change_outputs.is_empty() {
                            let sats = std::cmp::min(
                                current_excess as u64,
                                if change_outputs.is_empty() {
                                    args.change_first_satoshis
                                } else {
                                    args.change_initial_satoshis
                                },
                            );
                            change_outputs.push(ChangeOutput { satoshis: sats });
                        }
                    }
                }
            }
        }

        // Done if balanced/overbalanced or impossible
        let current_fe = fee_excess(
            args,
            &allocated_change_inputs,
            &change_outputs,
            &fixed_output_satoshis,
            0,
            0,
        );
        if current_fe >= 0 || change_outputs.is_empty() {
            break;
        }

        removing_outputs = true;

        // Drop change outputs one at a time until funded or none remain
        while !change_outputs.is_empty()
            && fee_excess(
                args,
                &allocated_change_inputs,
                &change_outputs,
                &fixed_output_satoshis,
                0,
                0,
            ) < 0
        {
            change_outputs.pop();
        }

        if fee_excess(
            args,
            &allocated_change_inputs,
            &change_outputs,
            &fixed_output_satoshis,
            0,
            0,
        ) < 0
        {
            // Not enough funding even without change outputs
            break;
        }

        // Remove change inputs that funded only a single change output
        // to reduce pointless churn
        let mut temp_inputs: Vec<AllocatedChangeInputRef> = allocated_change_inputs.clone();
        while temp_inputs.len() > 1 && change_outputs.len() > 1 {
            let last_output_sats = change_outputs.last().unwrap().satoshis;
            match temp_inputs
                .iter()
                .position(|ci| ci.satoshis <= last_output_sats)
            {
                None => break,
                Some(i) => {
                    change_outputs.pop();
                    temp_inputs.remove(i);
                }
            }
        }
        // and try again...
    }

    // Update fee_excess_now
    fee_excess_now = fee_excess(
        args,
        &allocated_change_inputs,
        &change_outputs,
        &fixed_output_satoshis,
        0,
        0,
    );

    // Handle maxPossibleSatoshis adjustment
    let mut max_possible_adjustment: Option<MaxPossibleSatoshisAdjustment> = None;
    if fee_excess_now < 0 {
        if let Some(idx) = has_max_possible_output {
            if fixed_output_satoshis[idx] != MAX_POSSIBLE_SATOSHIS {
                return Err(WalletError::Internal(
                    "maxPossibleSatoshis output changed unexpectedly".to_string(),
                ));
            }
            let adjusted = (fixed_output_satoshis[idx] as i64 + fee_excess_now) as u64;
            fixed_output_satoshis[idx] = adjusted;
            max_possible_adjustment = Some(MaxPossibleSatoshisAdjustment {
                fixed_output_index: idx,
                satoshis: adjusted,
            });
            fee_excess_now = fee_excess(
                args,
                &allocated_change_inputs,
                &change_outputs,
                &fixed_output_satoshis,
                0,
                0,
            );
        }
    }

    // Insufficient funds check
    if fee_excess_now < 0 {
        let total_needed = spending(&fixed_output_satoshis) as i64
            + fee_target(
                args,
                allocated_change_inputs.len(),
                change_outputs.len(),
                0,
                0,
            ) as i64;
        let more_needed = -fee_excess_now;
        release_all(&mut allocated_change_inputs, &mut storage);
        return Err(WalletError::InsufficientFunds {
            message: format!("Insufficient funds: need {} more satoshis", more_needed),
            total_satoshis_needed: total_needed,
            more_satoshis_needed: more_needed,
        });
    }

    // If no change outputs but excess > 0, need a change output to recapture
    if change_outputs.is_empty() && fee_excess_now > 0 {
        let total_needed = spending(&fixed_output_satoshis) as i64
            + fee_target(
                args,
                allocated_change_inputs.len(),
                change_outputs.len(),
                0,
                0,
            ) as i64;
        release_all(&mut allocated_change_inputs, &mut storage);
        return Err(WalletError::InsufficientFunds {
            message: "Insufficient funds: need change output".to_string(),
            total_satoshis_needed: total_needed,
            more_satoshis_needed: args.change_first_satoshis as i64,
        });
    }

    // === EXCESS DISTRIBUTION ===
    // Distribute excess fees across change outputs (matching TS behavior)
    while !change_outputs.is_empty() && fee_excess_now > 0 {
        if change_outputs.len() == 1 {
            change_outputs[0].satoshis += fee_excess_now as u64;
            fee_excess_now = 0;
        } else if change_outputs[0].satoshis < args.change_initial_satoshis {
            let sats = std::cmp::min(
                fee_excess_now as u64,
                args.change_initial_satoshis - change_outputs[0].satoshis,
            );
            fee_excess_now -= sats as i64;
            change_outputs[0].satoshis += sats;
        } else {
            // In TS this uses random distribution. For determinism in the
            // pure function we distribute 50% to the first output at a time.
            let sats = std::cmp::max(1, fee_excess_now as u64 / 2);
            fee_excess_now -= sats as i64;
            change_outputs[0].satoshis += sats;
        }
    }

    // Compute final size and fee
    let final_size = compute_size(args, allocated_change_inputs.len(), change_outputs.len(), 0, 0);
    let actual_fee = {
        let f = funding(args, &allocated_change_inputs);
        let s = spending(&fixed_output_satoshis);
        let c = change_total(&change_outputs);
        f - s - c
    };

    // Validate result: fee must equal target fee
    let expected_fee = fee_target(
        args,
        allocated_change_inputs.len(),
        change_outputs.len(),
        0,
        0,
    );
    if actual_fee != expected_fee {
        return Err(WalletError::Internal(format!(
            "generateChangeSdk error: required fee error {} !== {}",
            expected_fee, actual_fee
        )));
    }

    Ok(GenerateChangeSdkResult {
        change_outputs,
        allocated_change_inputs,
        size: final_size,
        fee: actual_fee,
        sats_per_kb: sats_per_kb,
        max_possible_satoshis_adjustment: max_possible_adjustment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::action_types::{FixedInput, FixedOutput, StorageFeeModel};

    fn make_args(
        fixed_inputs: Vec<FixedInput>,
        fixed_outputs: Vec<FixedOutput>,
        target_net_count: Option<i64>,
    ) -> GenerateChangeSdkArgs {
        GenerateChangeSdkArgs {
            fixed_inputs,
            fixed_outputs,
            fee_model: StorageFeeModel {
                model: "sat/kb".to_string(),
                value: 100,
            },
            change_initial_satoshis: 1000,
            change_first_satoshis: 285,
            change_locking_script_length: 25,
            change_unlocking_script_length: 107,
            target_net_count,
        }
    }

    fn make_utxos(satoshis_list: &[u64]) -> Vec<AvailableChange> {
        satoshis_list
            .iter()
            .enumerate()
            .map(|(i, &s)| AvailableChange {
                output_id: (i + 1) as i64,
                satoshis: s,
                spendable: true,
            })
            .collect()
    }

    #[test]
    fn test_zero_inputs_zero_outputs_zero_fee_rate() {
        // Given 0 available UTXOs and 0 fee rate, returns empty result with 0 fee
        let mut args = make_args(vec![], vec![], None);
        args.fee_model.value = 0;
        let available: Vec<AvailableChange> = vec![];
        let result = generate_change_sdk(&args, &available).unwrap();
        assert_eq!(result.change_outputs.len(), 0);
        assert_eq!(result.allocated_change_inputs.len(), 0);
        assert_eq!(result.fee, 0);
    }

    #[test]
    fn test_sufficient_utxos_target_count_1() {
        // Given sufficient UTXOs and target_net_count=1, returns 1 change output
        let args = make_args(
            vec![FixedInput {
                satoshis: 10_000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 5_000,
                locking_script_length: 25,
            }],
            Some(1),
        );
        let available = make_utxos(&[2000, 5000, 10000]);
        let result = generate_change_sdk(&args, &available).unwrap();
        assert!(!result.change_outputs.is_empty(), "should have at least 1 change output");
        assert!(result.fee > 0, "fee should be positive");
        // Verify: funding - spending - change = fee
        let total_funding: u64 =
            10_000 + result.allocated_change_inputs.iter().map(|i| i.satoshis).sum::<u64>();
        let total_spending: u64 =
            5_000 + result.change_outputs.iter().map(|o| o.satoshis).sum::<u64>();
        assert_eq!(total_funding - total_spending, result.fee);
    }

    #[test]
    fn test_insufficient_utxos_error() {
        // Given insufficient UTXOs, returns WERR_INSUFFICIENT_FUNDS
        let args = make_args(
            vec![],
            vec![FixedOutput {
                satoshis: 100_000,
                locking_script_length: 25,
            }],
            None,
        );
        let available = make_utxos(&[100, 200]);
        let result = generate_change_sdk(&args, &available);
        assert!(result.is_err());
        match result.unwrap_err() {
            WalletError::InsufficientFunds { .. } => {}
            other => panic!("Expected InsufficientFunds, got {:?}", other),
        }
    }

    #[test]
    fn test_starvation_loop_drops_change_outputs() {
        // When unable to fund all requested change outputs, drops them one at a time
        let args = make_args(
            vec![FixedInput {
                satoshis: 2000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 500,
                locking_script_length: 25,
            }],
            Some(3),
        );
        // Small UTXOs: cannot fund 3 net change outputs
        let available = make_utxos(&[300, 400, 500]);
        let result = generate_change_sdk(&args, &available).unwrap();
        assert!(result.fee > 0);
        // Verify balance
        let total_funding: u64 =
            2000 + result.allocated_change_inputs.iter().map(|i| i.satoshis).sum::<u64>();
        let total_spending: u64 =
            500 + result.change_outputs.iter().map(|o| o.satoshis).sum::<u64>();
        assert_eq!(total_funding - total_spending, result.fee);
    }

    #[test]
    fn test_excess_distributed_to_change() {
        // Excess satoshis are distributed to change outputs
        let args = make_args(
            vec![FixedInput {
                satoshis: 50_000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 1_000,
                locking_script_length: 25,
            }],
            Some(1),
        );
        let available = make_utxos(&[5000]);
        let result = generate_change_sdk(&args, &available).unwrap();
        let change_sum: u64 = result.change_outputs.iter().map(|o| o.satoshis).sum();
        let total_funding: u64 =
            50_000 + result.allocated_change_inputs.iter().map(|i| i.satoshis).sum::<u64>();
        assert_eq!(total_funding - 1_000 - result.fee, change_sum);
    }

    #[test]
    fn test_fee_calculation_matches_formula() {
        // Fee = ceil(transaction_size * fee_rate / 1000)
        let args = make_args(
            vec![FixedInput {
                satoshis: 10_000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 5_000,
                locking_script_length: 25,
            }],
            None,
        );
        let available = make_utxos(&[5000]);
        let result = generate_change_sdk(&args, &available).unwrap();
        let expected_fee = ((result.size as u64) * 100 + 999) / 1000;
        assert_eq!(result.fee, expected_fee);
    }

    #[test]
    fn test_configurable_initial_satoshis() {
        // Change outputs use configurable initial satoshis
        let mut args = make_args(
            vec![FixedInput {
                satoshis: 100_000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 1_000,
                locking_script_length: 25,
            }],
            Some(2),
        );
        args.change_initial_satoshis = 2000;
        args.change_first_satoshis = 500;
        let available = make_utxos(&[5000, 5000]);
        let result = generate_change_sdk(&args, &available).unwrap();
        assert!(!result.change_outputs.is_empty());
        let total_funding: u64 =
            100_000 + result.allocated_change_inputs.iter().map(|i| i.satoshis).sum::<u64>();
        let total_change: u64 = result.change_outputs.iter().map(|o| o.satoshis).sum();
        assert_eq!(total_funding - 1_000 - result.fee, total_change);
    }

    #[test]
    fn test_max_possible_satoshis_adjustment() {
        // maxPossibleSatoshis correctly limits output to actual funding
        let args = make_args(
            vec![FixedInput {
                satoshis: 5_000,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: MAX_POSSIBLE_SATOSHIS,
                locking_script_length: 25,
            }],
            None,
        );
        let available: Vec<AvailableChange> = vec![];
        let result = generate_change_sdk(&args, &available).unwrap();
        assert!(result.max_possible_satoshis_adjustment.is_some());
        let adj = result.max_possible_satoshis_adjustment.unwrap();
        assert_eq!(adj.fixed_output_index, 0);
        assert_eq!(adj.satoshis, 5_000 - result.fee);
    }

    #[test]
    fn test_no_change_exact_funding() {
        // When funding exactly covers spending + fee, no change output needed
        // 1 input (107 unlock) + 1 output (25 lock):
        //   size = 4 + 1 + (32+4+1+107+4) + 1 + (1+25+8) + 4 = 4+1+148+1+34+4 = 192
        //   fee = ceil(192 * 100 / 1000) = ceil(19.2) = 20
        // So we need exactly 5000 + 20 = 5020 in the input
        let args = make_args(
            vec![FixedInput {
                satoshis: 5_020,
                unlocking_script_length: 107,
            }],
            vec![FixedOutput {
                satoshis: 5_000,
                locking_script_length: 25,
            }],
            None,
        );
        let available: Vec<AvailableChange> = vec![];
        let result = generate_change_sdk(&args, &available).unwrap();
        // Should succeed with no change outputs and fee = 20
        assert_eq!(result.change_outputs.len(), 0);
        assert_eq!(result.allocated_change_inputs.len(), 0);
        // Size: 4 + 1 + 148 + 1 + 34 + 4 = 192
        assert_eq!(result.size, 192);
        assert_eq!(result.fee, 20);
    }
}
