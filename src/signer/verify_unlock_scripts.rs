//! Post-signing unlock script verification.
//!
//! Validates that every input's unlocking script correctly satisfies
//! its corresponding locking script using the SDK's Spend interpreter.

use bsv::script::spend::{Spend, SpendParams};
use bsv::transaction::beef::Beef;

use crate::error::{WalletError, WalletResult};

/// Verify all unlock scripts in a signed transaction within a BEEF.
///
/// For each input, constructs a `Spend` and runs `validate()`.
/// If any source tx is missing from the BEEF (e.g. knownTxids), returns Ok
/// (skip validation, matching TS behavior).
pub fn verify_unlock_scripts(txid: &str, beef: &Beef) -> WalletResult<()> {
    let beef_tx = beef
        .find_txid(txid)
        .ok_or_else(|| WalletError::InvalidParameter {
            parameter: "txid".to_string(),
            must_be: format!("contained in beef, txid {}", txid),
        })?;

    let tx = beef_tx
        .tx
        .as_ref()
        .ok_or_else(|| WalletError::InvalidParameter {
            parameter: "txid".to_string(),
            must_be: "a full transaction, not txid-only".to_string(),
        })?;

    // Phase 1: Collect source transactions, bail if any missing.
    // This matches TS behavior: if any source tx is not in the BEEF
    // (knownTxids), we skip validation entirely.
    let mut source_txs = Vec::new();
    for (i, input) in tx.inputs.iter().enumerate() {
        let source_txid =
            input
                .source_txid
                .as_ref()
                .ok_or_else(|| WalletError::InvalidParameter {
                    parameter: format!("inputs[{}].sourceTXID", i),
                    must_be: "valid".to_string(),
                })?;

        if input.unlocking_script.is_none() {
            return Err(WalletError::InvalidParameter {
                parameter: format!("inputs[{}].unlockingScript", i),
                must_be: "valid".to_string(),
            });
        }

        let source_beef_tx = match beef.find_txid(source_txid) {
            Some(btx) => btx,
            None => return Ok(()), // Source not in BEEF (knownTxids) — skip all validation
        };

        let source_tx = match &source_beef_tx.tx {
            Some(stx) => stx,
            None => return Ok(()), // txid-only — skip
        };

        source_txs.push(source_tx.clone());
    }

    // Phase 2: Validate each input
    for (i, input) in tx.inputs.iter().enumerate() {
        let source_tx = &source_txs[i];
        let source_output_index = input.source_output_index as usize;

        if source_output_index >= source_tx.outputs.len() {
            return Err(WalletError::InvalidParameter {
                parameter: format!("inputs[{}].sourceOutputIndex", i),
                must_be: format!("< {}", source_tx.outputs.len()),
            });
        }

        let source_output = &source_tx.outputs[source_output_index];
        let source_satoshis = source_output.satoshis.unwrap_or(0);

        let other_inputs: Vec<_> = tx
            .inputs
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != i)
            .map(|(_, inp)| inp.clone())
            .collect();

        let params = SpendParams {
            locking_script: source_output.locking_script.clone(),
            unlocking_script: input.unlocking_script.clone().unwrap(),
            source_txid: input.source_txid.clone().unwrap(),
            source_output_index,
            source_satoshis,
            transaction_version: tx.version,
            transaction_lock_time: tx.lock_time,
            transaction_sequence: input.sequence,
            other_inputs,
            other_outputs: tx.outputs.clone(),
            input_index: i,
        };

        let mut spend = Spend::new(params);
        match spend.validate() {
            Ok(true) => {} // Valid
            Ok(false) => {
                return Err(WalletError::InvalidParameter {
                    parameter: format!("inputs[{}].unlockScript", i),
                    must_be: "valid".to_string(),
                });
            }
            Err(e) => {
                return Err(WalletError::InvalidParameter {
                    parameter: format!("inputs[{}].unlockScript", i),
                    must_be: format!("valid. {}", e),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::transaction::beef::BEEF_V1;

    #[test]
    fn test_verify_empty_inputs_passes() {
        // A transaction with no inputs should pass verification
        use bsv::script::locking_script::LockingScript;
        use bsv::transaction::transaction::Transaction;
        use bsv::transaction::transaction_output::TransactionOutput;

        let mut tx = Transaction::new();
        tx.add_output(TransactionOutput {
            satoshis: Some(1000),
            locking_script: LockingScript::from_binary(&[
                0x76, 0xa9, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x88,
                0xac,
            ]),
            change: false,
        });

        let txid = tx.id().unwrap();
        let mut beef = Beef::new(BEEF_V1);
        let mut raw = Vec::new();
        tx.to_binary(&mut raw).unwrap();
        beef.merge_raw_tx(&raw, None).unwrap();

        let result = verify_unlock_scripts(&txid, &beef);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_missing_txid_in_beef_errors() {
        let beef = Beef::new(BEEF_V1);
        let result = verify_unlock_scripts("nonexistent", &beef);
        assert!(result.is_err());
    }
}
