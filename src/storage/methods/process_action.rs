//! Storage processAction implementation.
//!
//! Commits a signed transaction to storage: creates ProvenTxReq record,
//! updates Transaction and Output records, handles status state machine.
//! Ported from wallet-toolbox/src/storage/methods/processAction.ts.

use base64::Engine as _;
use bsv::wallet::interfaces::{ActionResultStatus, SendWithResult};
use chrono::Utc;
use rand::RngCore;

use crate::error::{WalletError, WalletResult};
use crate::status::{ProvenTxReqStatus, TransactionStatus};
use crate::storage::action_types::{StorageProcessActionArgs, StorageProcessActionResult};
use crate::storage::find_args::{
    FindOutputsArgs, FindProvenTxReqsArgs, FindTransactionsArgs, OutputPartial, ProvenTxReqPartial,
    TransactionPartial,
};
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::storage::{verify_one, TrxToken};
use crate::tables::ProvenTxReq;

/// Status pair for ProvenTxReq and Transaction during processAction.
struct ReqTxStatus {
    req: ProvenTxReqStatus,
    tx: TransactionStatus,
}

/// Commit a signed transaction to storage.
///
/// Validates the transaction exists, determines the correct status based on
/// nosend/delayed/sendWith flags, creates a ProvenTxReq record, and updates
/// Transaction and Output records.
pub async fn storage_process_action<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    user_id: i64,
    args: &StorageProcessActionArgs,
    _trx: Option<&TrxToken>,
) -> WalletResult<StorageProcessActionResult> {
    let mut r = StorageProcessActionResult {
        send_with_results: None,
        not_delayed_results: None,
        log: None,
    };

    if !args.is_new_tx {
        // A sendWith-only call releases durable noSend requests without
        // creating a new transaction.
        r.send_with_results = Some(schedule_send_with(storage, user_id, &args.send_with).await?);
        return Ok(r);
    }

    let reference = args
        .reference
        .as_ref()
        .ok_or_else(|| WalletError::InvalidOperation("reference is required".to_string()))?;
    let txid = args
        .txid
        .as_ref()
        .ok_or_else(|| WalletError::InvalidOperation("txid is required".to_string()))?;
    let raw_tx = args
        .raw_tx
        .as_ref()
        .ok_or_else(|| WalletError::InvalidOperation("rawTx is required".to_string()))?;

    // Find the existing transaction by user and reference
    let find_tx_args = FindTransactionsArgs {
        partial: TransactionPartial {
            user_id: Some(user_id),
            reference: Some(reference.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let transaction = verify_one(storage.find_transactions(&find_tx_args, _trx).await?)?;

    // Transaction must be outgoing
    if !transaction.is_outgoing {
        return Err(WalletError::InvalidOperation(
            "isOutgoing is not true".to_string(),
        ));
    }

    // Transaction must have unsigned or unprocessed status
    if transaction.status != TransactionStatus::Unsigned
        && transaction.status != TransactionStatus::Unprocessed
    {
        return Err(WalletError::InvalidOperation(format!(
            "invalid transaction status {}",
            transaction.status
        )));
    }

    let transaction_id = transaction.transaction_id;

    // Determine status based on nosend, isDelayed, isSendWith flags
    // Following TS exactly from processAction.ts:
    //   isNoSend && !isSendWith -> req: nosend, tx: nosend
    //   !isNoSend && isDelayed  -> req: unsent, tx: unprocessed
    //   !isNoSend && !isDelayed -> req: unprocessed, tx: unprocessed
    let status = if args.is_no_send && !args.is_send_with {
        ReqTxStatus {
            req: ProvenTxReqStatus::Nosend,
            tx: TransactionStatus::Nosend,
        }
    } else if !args.is_no_send && args.is_delayed {
        ReqTxStatus {
            req: ProvenTxReqStatus::Unsent,
            tx: TransactionStatus::Unprocessed,
        }
    } else if !args.is_no_send && !args.is_delayed {
        ReqTxStatus {
            req: ProvenTxReqStatus::Unprocessed,
            tx: TransactionStatus::Unprocessed,
        }
    } else {
        return Err(WalletError::Internal(
            "logic error in status determination".to_string(),
        ));
    };

    // Begin a database transaction for atomicity
    let db_trx = storage.begin_transaction().await?;

    // Create the ProvenTxReq record
    let now = Utc::now().naive_utc();
    let notify = serde_json::json!({
        "transactionIds": [transaction_id]
    });
    let new_req = ProvenTxReq {
        created_at: now,
        updated_at: now,
        proven_tx_req_id: 0,
        proven_tx_id: None,
        status: status.req,
        attempts: 0,
        notified: false,
        txid: txid.clone(),
        batch: None,
        history: "[]".to_string(),
        notify: serde_json::to_string(&notify).unwrap_or_default(),
        raw_tx: raw_tx.clone(),
        input_beef: transaction.input_beef.clone(),
    };
    let _req_id = storage
        .insert_proven_tx_req(&new_req, Some(&db_trx))
        .await?;

    // Update Transaction record: set txid, status, and store rawTx
    // (rawTx is also stored in ProvenTxReq; keeping it on the Transaction
    // record enables get_valid_beef_for_txid to build BEEF for later
    // createAction calls that spend this transaction's outputs)
    let tx_update = crate::storage::find_args::TransactionPartial {
        txid: Some(txid.clone()),
        status: Some(status.tx),
        raw_tx: Some(raw_tx.clone()),
        ..Default::default()
    };
    storage
        .update_transaction(transaction_id, &tx_update, Some(&db_trx))
        .await?;

    // Update Output records: set txid on all outputs belonging to this transaction
    let find_outputs_args = FindOutputsArgs {
        partial: OutputPartial {
            user_id: Some(user_id),
            transaction_id: Some(transaction_id),
            ..Default::default()
        },
        ..Default::default()
    };
    let outputs = storage
        .find_outputs(&find_outputs_args, Some(&db_trx))
        .await?;
    for output in &outputs {
        let output_update = crate::storage::find_args::OutputPartial {
            txid: Some(txid.clone()),
            spendable: Some(true),
            ..Default::default()
        };
        storage
            .update_output(output.output_id, &output_update, Some(&db_trx))
            .await?;
    }

    // Commit the database transaction
    storage.commit_transaction(db_trx).await?;

    // TS processAction always shares the newly committed request unless it is
    // a standalone noSend action. The delayed path is deliberately durable:
    // queued requests become `unsent` and TaskSendWaiting owns retries after
    // this function returns or the process restarts.
    let mut txids_to_schedule = args.send_with.clone();
    if !(args.is_no_send && !args.is_send_with) {
        txids_to_schedule.push(txid.clone());
    }
    // A standalone delayed transaction is already persisted as `unsent` by
    // the initial status selection above. Run the batch classifier only when
    // sendWith was explicitly requested, so ordinary delayed actions retain
    // their established result contract.
    if !args.send_with.is_empty() {
        r.send_with_results = Some(schedule_send_with(storage, user_id, &txids_to_schedule).await?);
    }

    Ok(r)
}

/// Classify and durably schedule a `sendWith` batch.
///
/// This is the delayed half of TS `shareReqsWithWorld`: ready requests are
/// made visible to the monitor as `unsent`, their owning transactions move to
/// `sending`, and a multi-request call receives one shared batch identifier.
/// Direct network posting remains in the signer/monitor layer, where Rust owns
/// services and can rebuild BEEF immediately before broadcast.
async fn schedule_send_with<S: StorageReaderWriter + ?Sized>(
    storage: &S,
    user_id: i64,
    txids: &[String],
) -> WalletResult<Vec<SendWithResult>> {
    if txids.is_empty() {
        return Ok(vec![]);
    }

    let mut random = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut random);
    let batch = (txids.len() > 1).then(|| base64::engine::general_purpose::STANDARD.encode(random));
    let trx = storage.begin_transaction().await?;
    let mut results = Vec::with_capacity(txids.len());

    for txid in txids {
        let transactions = storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        user_id: Some(user_id),
                        txid: Some(txid.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Some(&trx),
            )
            .await?;
        let Some(owned_txid) = transactions
            .first()
            .and_then(|transaction| transaction.txid.clone())
        else {
            results.push(SendWithResult {
                txid: txid.clone(),
                status: ActionResultStatus::Failed,
            });
            continue;
        };

        let reqs = storage
            .find_proven_tx_reqs(
                &FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial {
                        txid: Some(owned_txid),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Some(&trx),
            )
            .await?;
        let Some(req) = reqs.into_iter().next() else {
            results.push(SendWithResult {
                txid: txid.clone(),
                status: ActionResultStatus::Failed,
            });
            continue;
        };

        let ready = matches!(
            req.status,
            ProvenTxReqStatus::Sending
                | ProvenTxReqStatus::Unsent
                | ProvenTxReqStatus::Nosend
                | ProvenTxReqStatus::Unprocessed
        ) && !req.raw_tx.is_empty();
        let already_sent = matches!(
            req.status,
            ProvenTxReqStatus::Unmined
                | ProvenTxReqStatus::Callback
                | ProvenTxReqStatus::Unconfirmed
                | ProvenTxReqStatus::Completed
        );

        let result_status = if ready {
            storage
                .update_proven_tx_req(
                    req.proven_tx_req_id,
                    &ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Unsent),
                        batch: batch.clone(),
                        ..Default::default()
                    },
                    Some(&trx),
                )
                .await?;
            for transaction in transactions {
                storage
                    .update_transaction(
                        transaction.transaction_id,
                        &TransactionPartial {
                            status: Some(TransactionStatus::Sending),
                            ..Default::default()
                        },
                        Some(&trx),
                    )
                    .await?;
            }
            ActionResultStatus::Sending
        } else if already_sent {
            ActionResultStatus::Unproven
        } else {
            ActionResultStatus::Failed
        };
        results.push(SendWithResult {
            txid: txid.clone(),
            status: result_status,
        });
    }
    storage.commit_transaction(trx).await?;
    Ok(results)
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests {
    use super::*;
    use crate::status::{ProvenTxReqStatus, TransactionStatus};
    use crate::storage::action_types::StorageProcessActionArgs;
    use crate::storage::find_args::{
        FindOutputsArgs, FindProvenTxReqsArgs, FindTransactionsArgs, OutputPartial,
        ProvenTxReqPartial, TransactionPartial,
    };
    use crate::storage::sqlx_impl::SqliteStorage;
    use crate::storage::traits::provider::StorageProvider;
    use crate::storage::traits::reader::StorageReader;
    use crate::storage::traits::reader_writer::StorageReaderWriter;
    use crate::storage::StorageConfig;
    use crate::tables::{Output, Transaction};
    use crate::types::Chain;
    use chrono::Utc;

    /// Helper to set up a test storage with a user, transaction, and outputs.
    async fn setup_test_storage() -> (SqliteStorage, i64, i64, String) {
        let config = StorageConfig {
            url: "sqlite::memory:".to_string(),
            ..Default::default()
        };
        let storage = SqliteStorage::new_sqlite(config, Chain::Test)
            .await
            .expect("create storage");
        storage.migrate_database().await.expect("migrate");
        storage.make_available().await.expect("make available");

        // Create a user
        let (user, _) = storage
            .find_or_insert_user("test_identity_key", None)
            .await
            .expect("create user");
        let user_id = user.user_id;

        // Create a default basket
        let _basket = storage
            .find_or_insert_output_basket(user_id, "default", None)
            .await
            .expect("create basket");

        // Create a transaction in unsigned state
        let now = Utc::now().naive_utc();
        let reference = "test_ref_001".to_string();
        let tx = Transaction {
            created_at: now,
            updated_at: now,
            transaction_id: 0,
            user_id,
            proven_tx_id: None,
            status: TransactionStatus::Unsigned,
            reference: reference.clone(),
            is_outgoing: true,
            satoshis: -1000,
            description: "test transaction".to_string(),
            version: Some(1),
            lock_time: Some(0),
            txid: None,
            input_beef: Some(vec![0xDE, 0xAD]),
            raw_tx: None,
        };
        let tx_id = storage
            .insert_transaction(&tx, None)
            .await
            .expect("insert transaction");

        // Create some outputs for this transaction
        let output = Output {
            created_at: now,
            updated_at: now,
            output_id: 0,
            user_id,
            transaction_id: tx_id,
            basket_id: Some(_basket.basket_id),
            spendable: false,
            change: true,
            output_description: Some("test output".to_string()),
            vout: 0,
            satoshis: 500,
            provided_by: crate::types::StorageProvidedBy::Storage,
            purpose: "change".to_string(),
            output_type: "P2PKH".to_string(),
            txid: None,
            sender_identity_key: None,
            derivation_prefix: None,
            derivation_suffix: None,
            custom_instructions: None,
            spent_by: None,
            sequence_number: None,
            spending_description: None,
            script_length: None,
            script_offset: None,
            locking_script: Some(vec![0x76, 0xa9]),
        };
        storage
            .insert_output(&output, None)
            .await
            .expect("insert output");

        (storage, user_id, tx_id, reference)
    }

    async fn insert_test_transaction(
        storage: &SqliteStorage,
        user_id: i64,
        txid: &str,
        reference: &str,
        status: TransactionStatus,
    ) -> i64 {
        let now = Utc::now().naive_utc();
        storage
            .insert_transaction(
                &Transaction {
                    created_at: now,
                    updated_at: now,
                    transaction_id: 0,
                    user_id,
                    proven_tx_id: None,
                    status,
                    reference: reference.to_string(),
                    is_outgoing: true,
                    satoshis: -1000,
                    description: "sendWith transaction".to_string(),
                    version: Some(1),
                    lock_time: Some(0),
                    txid: Some(txid.to_string()),
                    input_beef: Some(vec![0xDE, 0xAD]),
                    raw_tx: Some(vec![1, 2, 3]),
                },
                None,
            )
            .await
            .expect("insert sendWith transaction")
    }

    #[tokio::test]
    async fn test_process_action_creates_proven_tx_req() {
        let (storage, user_id, _tx_id, reference) = setup_test_storage().await;

        let fake_txid =
            "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string();
        let fake_raw_tx = vec![0x01, 0x00, 0x00, 0x00, 0x00];

        let args = StorageProcessActionArgs {
            is_new_tx: true,
            is_send_with: false,
            is_no_send: false,
            is_delayed: false,
            reference: Some(reference),
            txid: Some(fake_txid.clone()),
            raw_tx: Some(fake_raw_tx),
            send_with: vec![],
        };

        let result = storage_process_action(&storage, user_id, &args, None)
            .await
            .expect("process_action should succeed");
        assert!(result.send_with_results.is_none());

        // Verify ProvenTxReq was created
        let req_args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                txid: Some(fake_txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let reqs = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find reqs");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].status, ProvenTxReqStatus::Unprocessed);
        assert_eq!(reqs[0].txid, fake_txid);

        // Verify Transaction was updated with txid and unprocessed status
        let tx_args = FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(_tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find txs");
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].txid, Some(fake_txid));
        assert_eq!(txs[0].status, TransactionStatus::Unprocessed);
    }

    #[tokio::test]
    async fn test_process_action_nosend_status() {
        let (storage, user_id, _tx_id, reference) = setup_test_storage().await;

        let fake_txid =
            "1111222233334444111122223333444411112222333344441111222233334444".to_string();
        let fake_raw_tx = vec![0x01, 0x00, 0x00, 0x00, 0x00];

        let args = StorageProcessActionArgs {
            is_new_tx: true,
            is_send_with: false,
            is_no_send: true,
            is_delayed: false,
            reference: Some(reference),
            txid: Some(fake_txid.clone()),
            raw_tx: Some(fake_raw_tx),
            send_with: vec![],
        };

        storage_process_action(&storage, user_id, &args, None)
            .await
            .expect("process_action nosend should succeed");

        // Verify ProvenTxReq has nosend status
        let req_args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                txid: Some(fake_txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let reqs = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find reqs");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].status, ProvenTxReqStatus::Nosend);

        // Verify Transaction has nosend status
        let tx_args = FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(_tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find txs");
        assert_eq!(txs[0].status, TransactionStatus::Nosend);
    }

    #[tokio::test]
    async fn test_process_action_delayed_status() {
        let (storage, user_id, _tx_id, reference) = setup_test_storage().await;

        let fake_txid =
            "5555666677778888555566667777888855556666777788885555666677778888".to_string();
        let fake_raw_tx = vec![0x01, 0x00, 0x00, 0x00, 0x00];

        let args = StorageProcessActionArgs {
            is_new_tx: true,
            is_send_with: false,
            is_no_send: false,
            is_delayed: true,
            reference: Some(reference),
            txid: Some(fake_txid.clone()),
            raw_tx: Some(fake_raw_tx),
            send_with: vec![],
        };

        storage_process_action(&storage, user_id, &args, None)
            .await
            .expect("process_action delayed should succeed");

        // Verify ProvenTxReq has unsent status
        let req_args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                txid: Some(fake_txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let reqs = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find reqs");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].status, ProvenTxReqStatus::Unsent);

        // A standalone delayed action keeps its initial unprocessed status.
        // sendWith batches move their owning transactions to sending below.
        let tx_args = FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(_tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let txs = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find txs");
        assert_eq!(txs[0].status, TransactionStatus::Unprocessed);
    }

    #[tokio::test]
    async fn test_process_action_updates_output_txid() {
        let (storage, user_id, _tx_id, reference) = setup_test_storage().await;

        let fake_txid =
            "aaaa1111bbbb2222aaaa1111bbbb2222aaaa1111bbbb2222aaaa1111bbbb2222".to_string();
        let fake_raw_tx = vec![0x01, 0x00, 0x00, 0x00, 0x00];

        let args = StorageProcessActionArgs {
            is_new_tx: true,
            is_send_with: false,
            is_no_send: false,
            is_delayed: false,
            reference: Some(reference),
            txid: Some(fake_txid.clone()),
            raw_tx: Some(fake_raw_tx),
            send_with: vec![],
        };

        storage_process_action(&storage, user_id, &args, None)
            .await
            .expect("process_action should succeed");

        // Verify outputs were updated with txid
        let output_args = FindOutputsArgs {
            partial: OutputPartial {
                user_id: Some(user_id),
                transaction_id: Some(_tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let outputs = storage
            .find_outputs(&output_args, None)
            .await
            .expect("find outputs");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].txid, Some(fake_txid));
        assert!(outputs[0].spendable);
    }

    #[tokio::test]
    async fn test_send_with_does_not_mutate_request_for_another_user() {
        let (storage, user_a_id, _tx_id, _reference) = setup_test_storage().await;
        let user_a = storage
            .find_user_by_identity_key("test_identity_key", None)
            .await
            .expect("find user A")
            .expect("user A exists");
        let (user_b, _) = storage
            .find_or_insert_user("victim_identity_key", None)
            .await
            .expect("create user B");
        assert_eq!(user_a.user_id, user_a_id);
        assert_ne!(user_a.user_id, user_b.user_id);
        assert_ne!(user_a.identity_key, user_b.identity_key);

        let txid = "3333333333333333333333333333333333333333333333333333333333333333".to_string();
        let user_b_tx_id = insert_test_transaction(
            &storage,
            user_b.user_id,
            &txid,
            "victim_shared_tx",
            TransactionStatus::Nosend,
        )
        .await;
        let now = Utc::now().naive_utc();
        storage
            .insert_proven_tx_req(
                &ProvenTxReq {
                    created_at: now,
                    updated_at: now,
                    proven_tx_req_id: 0,
                    proven_tx_id: None,
                    status: ProvenTxReqStatus::Nosend,
                    attempts: 0,
                    notified: false,
                    txid: txid.clone(),
                    batch: Some("victim-batch".to_string()),
                    history: "[]".to_string(),
                    notify: "{\"transactionIds\":[]}".to_string(),
                    raw_tx: vec![1, 2, 3],
                    input_beef: Some(vec![4, 5, 6]),
                },
                None,
            )
            .await
            .expect("insert shared request");

        let req_args = FindProvenTxReqsArgs {
            partial: ProvenTxReqPartial {
                txid: Some(txid.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let req_before = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find request before")
            .into_iter()
            .next()
            .expect("request exists before");
        let tx_args = FindTransactionsArgs {
            partial: TransactionPartial {
                transaction_id: Some(user_b_tx_id),
                ..Default::default()
            },
            ..Default::default()
        };
        let user_b_tx_before = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find user B transaction before")
            .into_iter()
            .next()
            .expect("user B transaction exists before");

        let results = schedule_send_with(&storage, user_a_id, std::slice::from_ref(&txid))
            .await
            .expect("classify unauthorized txid");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ActionResultStatus::Failed);

        let req_after = storage
            .find_proven_tx_reqs(&req_args, None)
            .await
            .expect("find request after")
            .into_iter()
            .next()
            .expect("request exists after");
        assert_eq!(req_after.proven_tx_req_id, req_before.proven_tx_req_id);
        assert_eq!(req_after.status, req_before.status);
        assert_eq!(req_after.batch, req_before.batch);

        let user_b_tx_after = storage
            .find_transactions(&tx_args, None)
            .await
            .expect("find user B transaction after")
            .into_iter()
            .next()
            .expect("user B transaction exists after");
        assert_eq!(user_b_tx_after, user_b_tx_before);
    }

    #[tokio::test]
    async fn test_send_with_allows_user_who_owns_shared_txid() {
        let (storage, user_a_id, _tx_id, _reference) = setup_test_storage().await;
        let (user_b, _) = storage
            .find_or_insert_user("victim_identity_key", None)
            .await
            .expect("create user B");
        assert_ne!(user_a_id, user_b.user_id);

        let txid = "3333333333333333333333333333333333333333333333333333333333333333".to_string();
        let user_b_tx_id = insert_test_transaction(
            &storage,
            user_b.user_id,
            &txid,
            "victim_shared_tx",
            TransactionStatus::Nosend,
        )
        .await;
        let now = Utc::now().naive_utc();
        let req_id = storage
            .insert_proven_tx_req(
                &ProvenTxReq {
                    created_at: now,
                    updated_at: now,
                    proven_tx_req_id: 0,
                    proven_tx_id: None,
                    status: ProvenTxReqStatus::Nosend,
                    attempts: 0,
                    notified: false,
                    txid: txid.clone(),
                    batch: Some("previous-batch".to_string()),
                    history: "[]".to_string(),
                    notify: "{\"transactionIds\":[]}".to_string(),
                    raw_tx: vec![1, 2, 3],
                    input_beef: Some(vec![4, 5, 6]),
                },
                None,
            )
            .await
            .expect("insert shared request");

        let results = schedule_send_with(&storage, user_b.user_id, std::slice::from_ref(&txid))
            .await
            .expect("schedule owned txid");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ActionResultStatus::Sending);

        let req = storage
            .find_proven_tx_reqs(
                &FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial {
                        txid: Some(txid),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("find scheduled request")
            .into_iter()
            .next()
            .expect("scheduled request exists");
        assert_eq!(req.proven_tx_req_id, req_id);
        assert_eq!(req.status, ProvenTxReqStatus::Unsent);
        assert_eq!(req.batch, Some("previous-batch".to_string()));

        let transaction = storage
            .find_transactions(
                &FindTransactionsArgs {
                    partial: TransactionPartial {
                        transaction_id: Some(user_b_tx_id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("find user B transaction")
            .into_iter()
            .next()
            .expect("user B transaction exists");
        assert_eq!(transaction.status, TransactionStatus::Sending);
    }

    #[tokio::test]
    async fn test_send_with_releases_nosend_requests_as_a_durable_batch() {
        let (storage, user_id, _tx_id, _reference) = setup_test_storage().await;
        let now = Utc::now().naive_utc();
        let txids = vec![
            "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            "2222222222222222222222222222222222222222222222222222222222222222".to_string(),
        ];
        for (index, txid) in txids.iter().enumerate() {
            insert_test_transaction(
                &storage,
                user_id,
                txid,
                &format!("batch_transaction_{index}"),
                TransactionStatus::Nosend,
            )
            .await;
            storage
                .insert_proven_tx_req(
                    &ProvenTxReq {
                        created_at: now,
                        updated_at: now,
                        proven_tx_req_id: 0,
                        proven_tx_id: None,
                        status: ProvenTxReqStatus::Nosend,
                        attempts: 0,
                        notified: false,
                        txid: txid.clone(),
                        batch: None,
                        history: "[]".to_string(),
                        notify: "{\"transactionIds\":[]}".to_string(),
                        raw_tx: vec![1, 2, 3],
                        input_beef: Some(vec![4, 5, 6]),
                    },
                    None,
                )
                .await
                .expect("insert noSend request");
        }

        let results = schedule_send_with(&storage, user_id, &txids)
            .await
            .expect("schedule batch");
        assert!(results
            .iter()
            .all(|result| result.status == ActionResultStatus::Sending));

        let reqs = storage
            .find_proven_tx_reqs(
                &FindProvenTxReqsArgs {
                    partial: ProvenTxReqPartial::default(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("find batch requests");
        let batch = reqs[0].batch.clone().expect("batch identifier");
        assert!(reqs.iter().all(|req| {
            req.status == ProvenTxReqStatus::Unsent && req.batch.as_deref() == Some(&batch)
        }));
    }
}
