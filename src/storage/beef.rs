//! BEEF construction from storage via recursive input chain walking.
//!
//! Builds a valid BEEF (BRC-62) for a given txid by recursively collecting
//! proven transactions and their merkle proofs from storage. This is needed
//! by the signer pipeline to build valid BEEF for transactions.

use std::collections::HashSet;
use std::io::Cursor;

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::*;
use crate::storage::traits::provider::StorageProvider;
use crate::storage::TrxToken;

use bsv::transaction::beef::{Beef, BEEF_V2};
use bsv::transaction::beef_tx::BeefTx;
use bsv::transaction::merkle_path::MerklePath;
use bsv::transaction::transaction::Transaction as BsvTransaction;

/// How to handle self-signed (wallet-created) transactions in BEEF construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustSelf {
    /// Trust transactions known to have been created by this wallet.
    /// Include them as txid-only entries without proof.
    Known,
    /// Trust both known and newly-created wallet transactions.
    KnownAndNew,
    /// Do not trust any self-signed transactions -- require full proofs.
    No,
}

/// Collected transaction data during recursive BEEF building.
struct CollectedTx {
    /// The raw transaction bytes.
    raw_tx: Vec<u8>,
    /// The txid of this transaction.
    txid: String,
    /// If proven, the merkle path bytes.
    merkle_path: Option<Vec<u8>>,
    /// Whether this is a txid-only entry (trusted, no raw_tx needed in BEEF).
    txid_only: bool,
}

/// Build a valid BEEF for the given txid by recursively walking input chains.
///
/// Looks up the txid in ProvenTx table first. If found with merkle_path, it's
/// a proven leaf. Otherwise looks up in Transaction table for raw_tx and
/// recurses into input txids.
///
/// Returns serialized BEEF bytes, or None if the transaction cannot be found.
pub async fn get_valid_beef_for_txid(
    storage: &dyn StorageProvider,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
    trx: Option<&TrxToken>,
) -> WalletResult<Option<Vec<u8>>> {
    let mut collected: Vec<CollectedTx> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    collect_tx_recursive(
        storage,
        txid,
        trust_self,
        known_txids,
        &mut collected,
        &mut visited,
        trx,
    )
    .await?;

    if collected.is_empty() {
        return Ok(None);
    }

    // Build Beef from collected transactions
    let mut beef = Beef::new(BEEF_V2);

    for ctx in &collected {
        if ctx.txid_only {
            // Txid-only entry -- no raw tx, no proof
            beef.txs.push(BeefTx {
                tx: None,
                txid: ctx.txid.clone(),
                bump_index: None,
                input_txids: Vec::new(),
            });
            continue;
        }

        let bsv_tx = {
            let mut cursor = Cursor::new(&ctx.raw_tx);
            BsvTransaction::from_binary(&mut cursor).map_err(|e| {
                WalletError::Internal(format!("Failed to parse raw_tx for {}: {}", ctx.txid, e))
            })?
        };

        let bump_index = if let Some(ref mp_bytes) = ctx.merkle_path {
            let mp = {
                let mut cursor = Cursor::new(mp_bytes);
                MerklePath::from_binary(&mut cursor).map_err(|e| {
                    WalletError::Internal(format!(
                        "Failed to parse merkle_path for {}: {}",
                        ctx.txid, e
                    ))
                })?
            };
            let idx = beef.bumps.len();
            beef.bumps.push(mp);
            Some(idx)
        } else {
            None
        };

        let beef_tx = BeefTx::from_tx(bsv_tx, bump_index).map_err(|e| {
            WalletError::Internal(format!("Failed to create BeefTx for {}: {}", ctx.txid, e))
        })?;
        beef.txs.push(beef_tx);
    }

    // Serialize BEEF to bytes
    let mut buf = Vec::new();
    beef.to_binary(&mut buf).map_err(|e| {
        WalletError::Internal(format!("Failed to serialize BEEF: {}", e))
    })?;

    Ok(Some(buf))
}

/// Recursively collect transaction data for BEEF construction.
///
/// Walks the input chain: for each unvisited input txid, checks if it's proven
/// (has merkle_path), otherwise recurses into its inputs.
async fn collect_tx_recursive(
    storage: &dyn StorageProvider,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
    collected: &mut Vec<CollectedTx>,
    visited: &mut HashSet<String>,
    trx: Option<&TrxToken>,
) -> WalletResult<()> {
    if visited.contains(txid) {
        return Ok(());
    }
    visited.insert(txid.to_string());

    // 1. Check if txid is in ProvenTx table (has merkle proof)
    let proven = storage
        .find_proven_txs(
            &FindProvenTxsArgs {
                partial: ProvenTxPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    if let Some(ptx) = proven.into_iter().next() {
        // Proven transaction -- it's a leaf with merkle proof
        if trust_self == TrustSelf::Known {
            // In Known mode, proven txs are trusted as txid-only
            collected.push(CollectedTx {
                raw_tx: Vec::new(),
                txid: txid.to_string(),
                merkle_path: None,
                txid_only: true,
            });
        } else {
            collected.push(CollectedTx {
                raw_tx: ptx.raw_tx,
                txid: txid.to_string(),
                merkle_path: Some(ptx.merkle_path),
                txid_only: false,
            });
        }
        return Ok(());
    }

    // 2. Not proven -- check Transaction table
    let txs = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            trx,
        )
        .await?;

    let tx_record = match txs.into_iter().next() {
        Some(t) => t,
        None => {
            // Transaction not found in storage -- check if it's a known txid
            if known_txids.contains(txid) {
                collected.push(CollectedTx {
                    raw_tx: Vec::new(),
                    txid: txid.to_string(),
                    merkle_path: None,
                    txid_only: true,
                });
            }
            return Ok(());
        }
    };

    // If trust_self is Known, treat wallet-created transactions as txid-only
    if trust_self == TrustSelf::Known {
        collected.push(CollectedTx {
            raw_tx: Vec::new(),
            txid: txid.to_string(),
            merkle_path: None,
            txid_only: true,
        });
        return Ok(());
    }

    let raw_tx = match tx_record.raw_tx {
        Some(ref raw) => raw.clone(),
        None => {
            // No raw_tx available -- can't build BEEF for this path.
            // If it's in known_txids, include as txid-only.
            if known_txids.contains(txid) {
                collected.push(CollectedTx {
                    raw_tx: Vec::new(),
                    txid: txid.to_string(),
                    merkle_path: None,
                    txid_only: true,
                });
            }
            return Ok(());
        }
    };

    // Parse raw_tx to extract input txids for recursion
    let bsv_tx = {
        let mut cursor = Cursor::new(&raw_tx);
        BsvTransaction::from_binary(&mut cursor).map_err(|e| {
            WalletError::Internal(format!("Failed to parse raw_tx for {}: {}", txid, e))
        })?
    };

    // Recurse into each input's source txid
    for input in &bsv_tx.inputs {
        if let Some(ref source_txid) = input.source_txid {
            if !visited.contains(source_txid.as_str()) {
                // If in known_txids, skip recursion and just add txid-only
                if known_txids.contains(source_txid.as_str()) {
                    visited.insert(source_txid.clone());
                    collected.push(CollectedTx {
                        raw_tx: Vec::new(),
                        txid: source_txid.clone(),
                        merkle_path: None,
                        txid_only: true,
                    });
                } else {
                    Box::pin(collect_tx_recursive(
                        storage,
                        source_txid,
                        trust_self,
                        known_txids,
                        collected,
                        visited,
                        trx,
                    ))
                    .await?;
                }
            }
        }
    }

    // Add this transaction (not proven, with raw_tx, no merkle proof)
    // Also merge inputBEEF if available
    collected.push(CollectedTx {
        raw_tx,
        txid: txid.to_string(),
        merkle_path: None,
        txid_only: false,
    });

    Ok(())
}
