//! BeefParty verification helper functions.
//!
//! Standalone functions that verify and merge txid-only transactions in BEEF data.
//! Port of the TS `verifyReturnedTxidOnly*` methods from Wallet.ts and
//! `getKnownTxids` helper.

use std::io::Cursor;

use bsv::transaction::beef::Beef;
use bsv::transaction::beef_party::BeefParty;
use bsv::wallet::error::WalletError;

/// Verify and resolve txid-only entries in a BEEF.
///
/// If `return_txid_only` is true, returns the beef unchanged (txid-only entries
/// are permitted). Otherwise, iterates over txid-only entries and attempts to
/// merge the full transaction data from `wallet_beef`. Errors if any txid-only
/// entries remain unresolved.
///
/// # Arguments
///
/// * `beef` - The BEEF to verify and potentially modify.
/// * `wallet_beef` - The wallet's BeefParty containing known full transactions.
/// * `return_txid_only` - If true, skip verification (txid-only is acceptable).
/// * `known_txids` - Optional list of txids the recipient already knows about
///   (these are allowed to remain txid-only).
pub fn verify_returned_txid_only(
    beef: &mut Beef,
    wallet_beef: &BeefParty,
    return_txid_only: bool,
    known_txids: Option<&[String]>,
) -> Result<(), WalletError> {
    if return_txid_only {
        return Ok(());
    }

    // Collect txid-only entries that need resolving
    let txid_only: Vec<String> = beef
        .txs
        .iter()
        .filter(|btx| btx.is_txid_only())
        .map(|btx| btx.txid.clone())
        .collect();

    for txid in &txid_only {
        // Skip txids the recipient already knows about
        if let Some(known) = known_txids {
            if known.contains(txid) {
                continue;
            }
        }

        // Try to find the full transaction in the wallet's beef
        if let Some(full_btx) = wallet_beef.beef.find_txid(txid) {
            if !full_btx.is_txid_only() {
                // Merge the full transaction data from wallet_beef into beef
                // We do this by merging the whole wallet beef (merge_beef handles dedup)
                if let Err(e) = beef.merge_beef(&wallet_beef.beef) {
                    return Err(WalletError::Internal(format!(
                        "unable to merge txid {} into beef: {}",
                        txid, e
                    )));
                }
                continue;
            }
        }

        return Err(WalletError::Internal(format!(
            "unable to merge txid {} into beef",
            txid
        )));
    }

    // Final check: ensure no unresolved txid-only entries remain
    for btx in &beef.txs {
        if btx.is_txid_only() {
            // Allow known txids to remain txid-only
            if let Some(known) = known_txids {
                if known.contains(&btx.txid) {
                    continue;
                }
            }
            return Err(WalletError::Internal(format!(
                "remaining txidOnly {} is not known",
                btx.txid
            )));
        }
    }

    Ok(())
}

/// Verify and resolve txid-only entries in an Atomic BEEF (binary format).
///
/// Parses the Atomic BEEF bytes, runs `verify_returned_txid_only`, and
/// re-serializes as Atomic BEEF.
///
/// # Arguments
///
/// * `beef_bytes` - Raw Atomic BEEF binary data.
/// * `wallet_beef` - The wallet's BeefParty.
/// * `return_txid_only` - If true, return input unchanged.
/// * `known_txids` - Optional known txids for the recipient.
pub fn verify_returned_txid_only_atomic_beef(
    beef_bytes: &[u8],
    wallet_beef: &BeefParty,
    return_txid_only: bool,
    known_txids: Option<&[String]>,
) -> Result<Vec<u8>, WalletError> {
    if return_txid_only {
        return Ok(beef_bytes.to_vec());
    }

    let mut cursor = Cursor::new(beef_bytes);
    let mut beef = Beef::from_binary(&mut cursor)
        .map_err(|e| WalletError::Internal(format!("failed to parse AtomicBEEF: {}", e)))?;

    let atomic_txid = beef
        .atomic_txid
        .clone()
        .ok_or_else(|| WalletError::Internal("AtomicBEEF missing atomic txid".to_string()))?;

    verify_returned_txid_only(&mut beef, wallet_beef, return_txid_only, known_txids)?;

    beef.to_binary_atomic(&atomic_txid)
        .map_err(|e| WalletError::Internal(format!("failed to serialize AtomicBEEF: {}", e)))
}

/// Verify and resolve txid-only entries in a BEEF (binary format).
///
/// Parses the BEEF bytes, runs `verify_returned_txid_only`, and
/// re-serializes as BEEF.
///
/// # Arguments
///
/// * `beef_bytes` - Raw BEEF binary data.
/// * `wallet_beef` - The wallet's BeefParty.
/// * `return_txid_only` - If true, return input unchanged.
pub fn verify_returned_txid_only_beef(
    beef_bytes: &[u8],
    wallet_beef: &BeefParty,
    return_txid_only: bool,
) -> Result<Vec<u8>, WalletError> {
    if return_txid_only {
        return Ok(beef_bytes.to_vec());
    }

    let mut cursor = Cursor::new(beef_bytes);
    let mut beef = Beef::from_binary(&mut cursor)
        .map_err(|e| WalletError::Internal(format!("failed to parse BEEF: {}", e)))?;

    verify_returned_txid_only(&mut beef, wallet_beef, return_txid_only, None)?;

    let mut output = Vec::new();
    beef.to_binary(&mut output)
        .map_err(|e| WalletError::Internal(format!("failed to serialize BEEF: {}", e)))?;
    Ok(output)
}

/// Get known txids from a BeefParty, optionally merging in additional txids.
///
/// Port of the TS `Wallet.getKnownTxids()`. Merges any `new_known_txids`
/// into the beef as txid-only entries, then returns all valid (non-txid-only)
/// txids from the beef.
///
/// # Arguments
///
/// * `beef` - The wallet's BeefParty to query.
/// * `new_known_txids` - Optional additional txids to add as txid-only.
pub fn get_known_txids(beef: &mut BeefParty, new_known_txids: Option<&[String]>) -> Vec<String> {
    // Merge new txids as txid-only entries
    if let Some(txids) = new_known_txids {
        for txid in txids {
            // Add as txid-only if not already present
            if beef.beef.find_txid(txid).is_none() {
                beef.beef
                    .txs
                    .push(bsv::transaction::beef_tx::BeefTx::from_txid(txid.clone()));
            }
        }
    }

    // Sort transactions in dependency order using Kahn's algorithm (SDK 0.1.6)
    // matching TS behavior which calls sortTxs() before collecting txids.
    beef.beef.sort_txs();

    // Collect all txids in dependency order.
    beef.beef.txs.iter().map(|btx| btx.txid.clone()).collect()
}
