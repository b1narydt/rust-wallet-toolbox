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

        // Try to find the full transaction in the wallet's beef and merge
        // ONLY that transaction's entry (TS `beef.mergeBeefTx(btx)`), with
        // its bump when it has one. Merging the whole party beef here — the
        // previous behavior — leaked every transaction the wallet's
        // BeefParty had ever accumulated into the returned BEEF.
        if let Some(full_btx) = wallet_beef.beef.find_txid(txid) {
            if let Some(ref full_tx) = full_btx.tx {
                let merged_bump_index = match full_btx.bump_index {
                    Some(bi) => {
                        let bump = wallet_beef.beef.bumps.get(bi).ok_or_else(|| {
                            WalletError::Internal(format!(
                                "wallet beef entry {txid} names bump {bi} which does not exist"
                            ))
                        })?;
                        Some(beef.merge_bump(bump).map_err(|e| {
                            WalletError::Internal(format!(
                                "unable to merge bump for txid {txid} into beef: {e}"
                            ))
                        })?)
                    }
                    None => None,
                };
                let mut raw = Vec::new();
                full_tx.to_binary(&mut raw).map_err(|e| {
                    WalletError::Internal(format!("unable to serialize txid {txid}: {e}"))
                })?;
                beef.merge_raw_tx(&raw, merged_bump_index).map_err(|e| {
                    WalletError::Internal(format!("unable to merge txid {txid} into beef: {e}"))
                })?;
                continue;
            }
        }

        return Err(WalletError::Internal(format!(
            "unable to merge txid {txid} into beef"
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
        .map_err(|e| WalletError::Internal(format!("failed to parse AtomicBEEF: {e}")))?;

    let atomic_txid = beef
        .atomic_txid
        .clone()
        .ok_or_else(|| WalletError::Internal("AtomicBEEF missing atomic txid".to_string()))?;

    verify_returned_txid_only(&mut beef, wallet_beef, return_txid_only, known_txids)?;

    beef.to_binary_atomic(&atomic_txid)
        .map_err(|e| WalletError::Internal(format!("failed to serialize AtomicBEEF: {e}")))
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
        .map_err(|e| WalletError::Internal(format!("failed to parse BEEF: {e}")))?;

    verify_returned_txid_only(&mut beef, wallet_beef, return_txid_only, None)?;

    let mut output = Vec::new();
    beef.to_binary(&mut output)
        .map_err(|e| WalletError::Internal(format!("failed to serialize BEEF: {e}")))?;
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

    // Collect txids in dependency order, EXCLUDING txid-only entries: a
    // txid-only entry is an unproven claim, and advertising it as "known"
    // lets the wallet elide proof data the recipient cannot reconstruct.
    //
    // TODO(bsv-sdk): full TS parity is `sortTxs().valid` — only txids that
    // are bump-proven or chain to proven ancestors. The Rust SDK's
    // `sort_txs()` returns no SortResult, so the proven-or-chained subset
    // is not observable here; excluding txid-only entries is the sound
    // narrowing available today. Upstream a SortResult return, then switch
    // this to its `valid` set.
    beef.beef
        .txs
        .iter()
        .filter(|btx| !btx.is_txid_only())
        .map(|btx| btx.txid.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bsv::script::locking_script::LockingScript;
    use bsv::transaction::beef::BEEF_V2;
    use bsv::transaction::transaction::Transaction;
    use bsv::transaction::transaction_output::TransactionOutput;

    fn simple_tx(sats: u64) -> Transaction {
        let mut tx = Transaction::new();
        tx.add_output(TransactionOutput {
            satoshis: Some(sats),
            locking_script: LockingScript::from_binary(&[0x51]),
            change: false,
        });
        tx
    }

    fn party_with(txs: &[&Transaction]) -> BeefParty {
        let mut party = BeefParty::new(["test"]);
        for tx in txs {
            let mut raw = Vec::new();
            tx.to_binary(&mut raw).unwrap();
            party.beef.merge_raw_tx(&raw, None).unwrap();
        }
        party
    }

    /// Resolving a txid-only entry merges ONLY that transaction from the
    /// wallet's BeefParty — never the party's whole accumulated beef.
    #[test]
    fn resolves_txid_only_without_leaking_the_party_beef() {
        let wanted = simple_tx(100);
        let unrelated = simple_tx(200);
        let wanted_txid = wanted.id().unwrap();
        let unrelated_txid = unrelated.id().unwrap();
        let party = party_with(&[&wanted, &unrelated]);

        let mut beef = Beef::new(BEEF_V2);
        beef.txs.push(bsv::transaction::beef_tx::BeefTx::from_txid(
            wanted_txid.clone(),
        ));

        verify_returned_txid_only(&mut beef, &party, false, None).expect("resolves");

        let resolved = beef.find_txid(&wanted_txid).expect("wanted present");
        assert!(!resolved.is_txid_only(), "wanted resolved to a full tx");
        assert!(
            beef.find_txid(&unrelated_txid).is_none(),
            "unrelated party transaction must NOT leak into the returned BEEF"
        );
    }

    /// A txid-only entry the party cannot resolve still errors, naming it.
    #[test]
    fn unresolvable_txid_only_errors() {
        let party = party_with(&[]);
        let missing = "ab".repeat(32);
        let mut beef = Beef::new(BEEF_V2);
        beef.txs.push(bsv::transaction::beef_tx::BeefTx::from_txid(
            missing.clone(),
        ));

        let err = verify_returned_txid_only(&mut beef, &party, false, None)
            .expect_err("must not accept an unresolvable txid-only entry");
        assert!(err.to_string().contains(&missing));
    }

    /// get_known_txids excludes txid-only entries: an unproven claim must
    /// not be advertised as a txid the wallet can vouch for.
    #[test]
    fn get_known_txids_excludes_txid_only_entries() {
        let full = simple_tx(300);
        let full_txid = full.id().unwrap();
        let mut party = party_with(&[&full]);
        let claimed = "cd".repeat(32);

        let known = get_known_txids(&mut party, Some(std::slice::from_ref(&claimed)));

        assert!(known.contains(&full_txid), "full tx txid is known");
        assert!(
            !known.contains(&claimed),
            "txid-only entry must not be advertised as known"
        );
    }
}
