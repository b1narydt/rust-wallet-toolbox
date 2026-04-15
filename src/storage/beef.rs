//! BEEF construction from storage via recursive input chain walking.
//!
//! Builds a valid BEEF (BRC-62) for a given txid by recursively collecting
//! proven transactions and their merkle proofs from storage. This is needed
//! by the signer pipeline to build valid BEEF for transactions.

use std::collections::HashSet;
use std::io::Cursor;

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::*;
use crate::storage::traits::reader::StorageReader;
use crate::storage::traits::wallet_provider::WalletStorageProvider;

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
    /// Stored inputBEEF from the transaction record (ancestor proof chain).
    input_beef: Option<Vec<u8>>,
}

/// Build a valid BEEF for the given txid by recursively walking input chains.
///
/// Looks up the txid in ProvenTx table first. If found with merkle_path, it's
/// a proven leaf. Otherwise looks up in Transaction table for raw_tx and
/// recurses into input txids.
///
/// Returns serialized BEEF bytes, or None if the transaction cannot be found.
pub async fn get_valid_beef_for_txid(
    storage: &(dyn WalletStorageProvider + Send + Sync),
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
) -> WalletResult<Option<Vec<u8>>> {
    get_valid_beef_for_txid_inner(storage, txid, trust_self, known_txids).await
}

/// Variant accepting a `StorageReader` for use from within storage methods.
///
/// Uses the low-level `StorageReader` trait (with explicit `None` trx args)
/// instead of `WalletStorageProvider`. This allows it to be called from within
/// `storage_create_action` where the storage is `&S: StorageReaderWriter`.
pub async fn get_valid_beef_for_storage_reader<S: StorageReader + ?Sized>(
    storage: &S,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
) -> WalletResult<Option<Vec<u8>>> {
    let mut collected: Vec<CollectedTx> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    collect_tx_recursive_reader(
        storage,
        txid,
        trust_self,
        known_txids,
        &mut collected,
        &mut visited,
    )
    .await?;
    build_beef_from_collected(collected)
}

async fn get_valid_beef_for_txid_inner(
    storage: &dyn WalletStorageProvider,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
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
    )
    .await?;

    build_beef_from_collected(collected)
}

/// Recursively collect transaction data for BEEF construction.
///
/// Walks the input chain: for each unvisited input txid, checks if it's proven
/// (has merkle_path), otherwise recurses into its inputs.
async fn collect_tx_recursive(
    storage: &dyn WalletStorageProvider,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
    collected: &mut Vec<CollectedTx>,
    visited: &mut HashSet<String>,
) -> WalletResult<()> {
    if visited.contains(txid) {
        return Ok(());
    }
    visited.insert(txid.to_string());

    // 1. Check if txid is in ProvenTx table (has merkle proof)
    let proven = storage
        .find_proven_txs(&FindProvenTxsArgs {
            partial: ProvenTxPartial {
                txid: Some(txid.to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
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
                input_beef: None,
            });
        } else {
            collected.push(CollectedTx {
                raw_tx: ptx.raw_tx,
                txid: txid.to_string(),
                merkle_path: Some(ptx.merkle_path),
                txid_only: false,
                input_beef: None,
            });
        }
        return Ok(());
    }

    // 2. Not proven -- check Transaction table
    let txs = storage
        .find_transactions(&FindTransactionsArgs {
            partial: TransactionPartial {
                txid: Some(txid.to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
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
                    input_beef: None,
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
            input_beef: None,
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
                    input_beef: None,
                });
            }
            return Ok(());
        }
    };

    // Extract stored inputBEEF before consuming tx_record
    let stored_input_beef = tx_record.input_beef.filter(|ib| !ib.is_empty());

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
                        input_beef: None,
                    });
                } else {
                    Box::pin(collect_tx_recursive(
                        storage,
                        source_txid,
                        trust_self,
                        known_txids,
                        collected,
                        visited,
                    ))
                    .await?;
                }
            }
        }
    }

    // Add this transaction (not proven, with raw_tx, no merkle proof)
    // Include stored inputBEEF so ancestor proof chains are preserved
    collected.push(CollectedTx {
        raw_tx,
        txid: txid.to_string(),
        merkle_path: None,
        txid_only: false,
        input_beef: stored_input_beef,
    });

    Ok(())
}

/// Build a BEEF from collected transactions and return serialized bytes.
fn build_beef_from_collected(collected: Vec<CollectedTx>) -> WalletResult<Option<Vec<u8>>> {
    if collected.is_empty() {
        return Ok(None);
    }

    let mut beef = Beef::new(BEEF_V2);

    for ctx in &collected {
        if ctx.txid_only {
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

        // Merge stored inputBEEF (ancestor proof chains from received payments)
        if let Some(ref ib) = ctx.input_beef {
            beef.merge_beef_from_binary(ib).map_err(|e| {
                WalletError::Internal(format!(
                    "Failed to merge stored inputBEEF for {}: {}",
                    ctx.txid, e
                ))
            })?;
        }
    }

    let mut buf = Vec::new();
    beef.to_binary(&mut buf)
        .map_err(|e| WalletError::Internal(format!("Failed to serialize BEEF: {}", e)))?;

    Ok(Some(buf))
}

/// Recursively collect transaction data using the low-level `StorageReader` trait.
///
/// Used by `get_valid_beef_for_storage_reader` which is called from within
/// storage method implementations where `WalletStorageProvider` isn't available.
async fn collect_tx_recursive_reader<S: StorageReader + ?Sized>(
    storage: &S,
    txid: &str,
    trust_self: TrustSelf,
    known_txids: &HashSet<String>,
    collected: &mut Vec<CollectedTx>,
    visited: &mut HashSet<String>,
) -> WalletResult<()> {
    if visited.contains(txid) {
        return Ok(());
    }
    visited.insert(txid.to_string());

    // 1. Check ProvenTx table
    let proven = storage
        .find_proven_txs(
            &FindProvenTxsArgs {
                partial: ProvenTxPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;

    if let Some(ptx) = proven.into_iter().next() {
        if trust_self == TrustSelf::Known {
            collected.push(CollectedTx {
                raw_tx: Vec::new(),
                txid: txid.to_string(),
                merkle_path: None,
                txid_only: true,
                input_beef: None,
            });
        } else {
            collected.push(CollectedTx {
                raw_tx: ptx.raw_tx,
                txid: txid.to_string(),
                merkle_path: Some(ptx.merkle_path),
                txid_only: false,
                input_beef: None,
            });
        }
        return Ok(());
    }

    // 2. Check Transaction table
    let txs = storage
        .find_transactions(
            &FindTransactionsArgs {
                partial: TransactionPartial {
                    txid: Some(txid.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .await?;

    let tx_record = match txs.into_iter().next() {
        Some(t) => t,
        None => {
            if known_txids.contains(txid) {
                collected.push(CollectedTx {
                    raw_tx: Vec::new(),
                    txid: txid.to_string(),
                    merkle_path: None,
                    txid_only: true,
                    input_beef: None,
                });
            }
            return Ok(());
        }
    };

    if trust_self == TrustSelf::Known {
        collected.push(CollectedTx {
            raw_tx: Vec::new(),
            txid: txid.to_string(),
            merkle_path: None,
            txid_only: true,
            input_beef: None,
        });
        return Ok(());
    }

    let raw_tx = match tx_record.raw_tx {
        Some(ref raw) => raw.clone(),
        None => {
            if known_txids.contains(txid) {
                collected.push(CollectedTx {
                    raw_tx: Vec::new(),
                    txid: txid.to_string(),
                    merkle_path: None,
                    txid_only: true,
                    input_beef: None,
                });
            }
            return Ok(());
        }
    };

    // Extract stored inputBEEF before consuming tx_record
    let stored_input_beef = tx_record.input_beef.filter(|ib| !ib.is_empty());

    let bsv_tx = {
        let mut cursor = Cursor::new(&raw_tx);
        BsvTransaction::from_binary(&mut cursor).map_err(|e| {
            WalletError::Internal(format!("Failed to parse raw_tx for {}: {}", txid, e))
        })?
    };

    for input in &bsv_tx.inputs {
        if let Some(ref source_txid) = input.source_txid {
            if !visited.contains(source_txid.as_str()) {
                if known_txids.contains(source_txid.as_str()) {
                    visited.insert(source_txid.clone());
                    collected.push(CollectedTx {
                        raw_tx: Vec::new(),
                        txid: source_txid.clone(),
                        merkle_path: None,
                        txid_only: true,
                        input_beef: None,
                    });
                } else {
                    Box::pin(collect_tx_recursive_reader(
                        storage,
                        source_txid,
                        trust_self,
                        known_txids,
                        collected,
                        visited,
                    ))
                    .await?;
                }
            }
        }
    }

    // Include stored inputBEEF so ancestor proof chains are preserved
    collected.push(CollectedTx {
        raw_tx,
        txid: txid.to_string(),
        merkle_path: None,
        txid_only: false,
        input_beef: stored_input_beef,
    });

    Ok(())
}
