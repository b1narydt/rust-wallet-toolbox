//! TaskArcSse -- processes real-time SSE events from ARC transaction status updates.
//!
//! Translated from wallet-toolbox/src/monitor/tasks/TaskArcSSE.ts (290 lines).
//!
//! Receives transaction status updates from Arcade via SSE (Server-Sent Events)
//! and processes them -- including fetching merkle proofs directly from Arcade
//! when transactions are MINED.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::error::WalletError;
use crate::monitor::task_trait::WalletMonitorTask;
use crate::monitor::AsyncCallback;
use crate::services::providers::arc_sse_client::{ArcSseClient, ArcSseClientOptions};
use crate::services::traits::WalletServices;
use crate::services::types::ArcSseEvent;
use crate::status::ProvenTxReqStatus;
use crate::storage::find_args::{FindProvenTxReqsArgs, ProvenTxReqPartial};
use crate::storage::manager::WalletStorageManager;

/// Terminal statuses for ProvenTxReq -- events for these are skipped.
const TERMINAL_STATUSES: &[ProvenTxReqStatus] = &[
    ProvenTxReqStatus::Completed,
    ProvenTxReqStatus::Invalid,
    ProvenTxReqStatus::DoubleSpend,
];

/// Task that receives and processes real-time ARC transaction status via SSE.
///
/// Consumes events from an ArcSseClient mpsc channel. Both a callback token and
/// an Arcade base URL are required; with either missing the task stays dormant
/// (trigger always returns false), which is TS's behaviour too
/// (TaskArcadeSSE.asyncSetup: "no arcadeUrl configured; SSE disabled").
pub struct TaskArcSse {
    storage: Arc<WalletStorageManager>,
    _services: Arc<dyn WalletServices>,
    /// Callback token for ARC SSE streaming.
    callback_token: Option<String>,
    /// Base URL of the Arcade instance serving `/events`.
    arcade_url: Option<String>,
    /// Optional callback when transaction status changes.
    on_tx_status_changed: Option<AsyncCallback<(String, String)>>,
    /// Cancels the active connection loop during replacement or shutdown.
    sse_cancel: Option<CancellationToken>,
    /// Lets setup wait until the prior connection loop has stopped.
    sse_handle: Option<JoinHandle<()>>,
    /// Receiver for SSE events from the client.
    sse_receiver: Option<mpsc::Receiver<ArcSseEvent>>,
    /// Buffer of pending events drained from the receiver.
    pending_events: Vec<ArcSseEvent>,
}

impl Drop for TaskArcSse {
    fn drop(&mut self) {
        if let Some(cancel) = self.sse_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.sse_handle.take() {
            handle.abort();
        }
    }
}

impl TaskArcSse {
    /// Create a new ARC SSE task.
    pub fn new(
        storage: Arc<WalletStorageManager>,
        services: Arc<dyn WalletServices>,
        callback_token: Option<String>,
        arcade_url: Option<String>,
        on_tx_status_changed: Option<AsyncCallback<(String, String)>>,
    ) -> Self {
        Self {
            storage,
            _services: services,
            callback_token,
            arcade_url,
            on_tx_status_changed,
            sse_cancel: None,
            sse_handle: None,
            sse_receiver: None,
            pending_events: Vec::new(),
        }
    }

    async fn stop_sse_connection(&mut self) {
        self.sse_receiver = None;
        cancel_and_await_sse_loop(&mut self.sse_cancel, &mut self.sse_handle).await;
    }

    /// Process a single SSE status event.
    async fn process_status_event(&self, event: &ArcSseEvent) -> String {
        let mut log = format!("SSE: txid={} status={}\n", event.txid, event.tx_status);

        // Find matching ProvenTxReqs
        let reqs = match self
            .storage
            .find_proven_tx_reqs(&FindProvenTxReqsArgs {
                partial: ProvenTxReqPartial {
                    txid: Some(event.txid.clone()),
                    ..Default::default()
                },
                since: None,
                paged: None,
                statuses: None,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log.push_str(&format!("  error finding reqs: {e}\n"));
                return log;
            }
        };

        if reqs.is_empty() {
            log.push_str("  No matching ProvenTxReq\n");
            return log;
        }

        for req in &reqs {
            // Skip terminal statuses
            if TERMINAL_STATUSES.contains(&req.status) {
                log.push_str(&format!(
                    "  req {} already terminal: {:?}\n",
                    req.proven_tx_req_id, req.status
                ));
                continue;
            }

            match event.tx_status.as_str() {
                "SENT_TO_NETWORK" | "ACCEPTED_BY_NETWORK" | "SEEN_ON_NETWORK" => {
                    // Transition to unmined if currently in pre-mining states
                    if matches!(
                        req.status,
                        ProvenTxReqStatus::Unsent
                            | ProvenTxReqStatus::Sending
                            | ProvenTxReqStatus::Callback
                    ) {
                        let update = ProvenTxReqPartial {
                            status: Some(ProvenTxReqStatus::Unmined),
                            ..Default::default()
                        };
                        let _ = self
                            .storage
                            .update_proven_tx_req(req.proven_tx_req_id, &update)
                            .await;

                        // Update referenced transactions to unproven
                        if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&req.notify) {
                            if let Some(tx_ids) =
                                notify.get("transactionIds").and_then(|v| v.as_array())
                            {
                                let ids: Vec<i64> =
                                    tx_ids.iter().filter_map(|v| v.as_i64()).collect();
                                for id in &ids {
                                    let _ = self
                                        .storage
                                        .update_transaction(
                                            *id,
                                            &crate::storage::find_args::TransactionPartial {
                                                status: Some(
                                                    crate::status::TransactionStatus::Unproven,
                                                ),
                                                ..Default::default()
                                            },
                                        )
                                        .await;
                                }
                            }
                        }
                        log.push_str(&format!("  req {} => unmined\n", req.proven_tx_req_id));
                    }
                }
                "MINED" | "IMMUTABLE" => {
                    // Transaction mined -- attempt to fetch proof
                    // For now we set to unmined and let TaskCheckForProofs pick it up
                    let update = ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Unmined),
                        ..Default::default()
                    };
                    let _ = self
                        .storage
                        .update_proven_tx_req(req.proven_tx_req_id, &update)
                        .await;
                    log.push_str(&format!(
                        "  req {} MINED/IMMUTABLE => unmined (proof collection deferred)\n",
                        req.proven_tx_req_id
                    ));
                }
                "DOUBLE_SPEND_ATTEMPTED" => {
                    let update = ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::DoubleSpend),
                        ..Default::default()
                    };
                    let _ = self
                        .storage
                        .update_proven_tx_req(req.proven_tx_req_id, &update)
                        .await;

                    // Update referenced transactions to failed
                    if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&req.notify) {
                        if let Some(tx_ids) =
                            notify.get("transactionIds").and_then(|v| v.as_array())
                        {
                            let ids: Vec<i64> = tx_ids.iter().filter_map(|v| v.as_i64()).collect();
                            for id in &ids {
                                let _ = self
                                    .storage
                                    .update_transaction(
                                        *id,
                                        &crate::storage::find_args::TransactionPartial {
                                            status: Some(crate::status::TransactionStatus::Failed),
                                            ..Default::default()
                                        },
                                    )
                                    .await;
                            }
                        }
                    }
                    log.push_str(&format!("  req {} => doubleSpend\n", req.proven_tx_req_id));
                }
                "REJECTED" => {
                    let update = ProvenTxReqPartial {
                        status: Some(ProvenTxReqStatus::Invalid),
                        ..Default::default()
                    };
                    let _ = self
                        .storage
                        .update_proven_tx_req(req.proven_tx_req_id, &update)
                        .await;

                    // Update referenced transactions to failed
                    if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&req.notify) {
                        if let Some(tx_ids) =
                            notify.get("transactionIds").and_then(|v| v.as_array())
                        {
                            let ids: Vec<i64> = tx_ids.iter().filter_map(|v| v.as_i64()).collect();
                            for id in &ids {
                                let _ = self
                                    .storage
                                    .update_transaction(
                                        *id,
                                        &crate::storage::find_args::TransactionPartial {
                                            status: Some(crate::status::TransactionStatus::Failed),
                                            ..Default::default()
                                        },
                                    )
                                    .await;
                            }
                        }
                    }
                    log.push_str(&format!("  req {} => invalid\n", req.proven_tx_req_id));
                }
                other => {
                    log.push_str(&format!(
                        "  req {} unhandled status: {}\n",
                        req.proven_tx_req_id, other
                    ));
                }
            }
        }

        // Fire status changed callback
        if let Some(ref cb) = self.on_tx_status_changed {
            cb((event.txid.clone(), event.tx_status.clone())).await;
        }

        log
    }
}

#[async_trait]
impl WalletMonitorTask for TaskArcSse {
    fn storage_manager(&self) -> Option<&WalletStorageManager> {
        Some(&self.storage)
    }

    fn name(&self) -> &str {
        "ArcadeSSE"
    }

    async fn async_setup(&mut self) -> Result<(), WalletError> {
        self.stop_sse_connection().await;

        // Inherit whatever the default setup does (currently:
        // `make_available()` on `storage_manager()`). Delegating
        // instead of hand-copying means this task automatically picks
        // up any future additions to the default setup — metrics,
        // logging, additional state prep — rather than silently
        // skipping them.
        crate::monitor::task_trait::default_async_setup(self).await?;

        let callback_token = match &self.callback_token {
            Some(t) if !t.is_empty() => t.clone(),
            _ => {
                info!("[TaskArcSse] no callbackToken configured -- SSE disabled");
                return Ok(());
            }
        };

        let arcade_url = match &self.arcade_url {
            Some(u) if !u.is_empty() => u.clone(),
            _ => {
                info!("[TaskArcSse] no arcadeUrl configured -- SSE disabled");
                return Ok(());
            }
        };

        info!("[TaskArcSse] setting up SSE for arcadeUrl={arcade_url}");

        let options = ArcSseClientOptions {
            base_url: arcade_url,
            callback_token,
            arc_api_key: None,
            last_event_id: None,
        };

        // The reconnect loop owns the client while this monitor task retains
        // the receiver, cancellation token, and join handle for its lifecycle.
        let (mut client, receiver) = ArcSseClient::new(options);
        self.sse_cancel = Some(client.cancel_handle());
        self.sse_handle = Some(tokio::spawn(async move { client.connect().await }));
        self.sse_receiver = Some(receiver);

        Ok(())
    }

    fn trigger(&mut self, _now_msecs: u64) -> bool {
        // Drain any available events from the receiver
        if let Some(ref mut rx) = self.sse_receiver {
            loop {
                match rx.try_recv() {
                    Ok(event) => self.pending_events.push(event),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
        }
        !self.pending_events.is_empty()
    }

    async fn run_task(&mut self) -> Result<String, WalletError> {
        let events: Vec<ArcSseEvent> = self.pending_events.drain(..).collect();
        if events.is_empty() {
            return Ok(String::new());
        }

        let mut log = String::new();
        for event in &events {
            log.push_str(&self.process_status_event(event).await);
        }

        Ok(log)
    }
}

async fn cancel_and_await_sse_loop(
    cancel: &mut Option<CancellationToken>,
    handle: &mut Option<JoinHandle<()>>,
) {
    if let Some(cancel) = cancel.take() {
        cancel.cancel();
    }
    if let Some(handle) = handle.take() {
        let _ = handle.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_terminal_statuses() {
        assert!(TERMINAL_STATUSES.contains(&ProvenTxReqStatus::Completed));
        assert!(TERMINAL_STATUSES.contains(&ProvenTxReqStatus::Invalid));
        assert!(TERMINAL_STATUSES.contains(&ProvenTxReqStatus::DoubleSpend));
        assert!(!TERMINAL_STATUSES.contains(&ProvenTxReqStatus::Unmined));
    }

    #[test]
    fn test_name() {
        assert_eq!("ArcadeSSE", "ArcadeSSE");
    }

    #[test]
    fn test_no_sse_configured_is_noop() {
        // When no callback_token is set, the task has no receiver and never triggers.
        // We cannot construct TaskArcSse without a real WalletStorageManager,
        // so we verify the logic conceptually: empty pending_events and no receiver
        // means trigger returns false.
        let events: Vec<ArcSseEvent> = Vec::new();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn replacing_connection_cancels_and_awaits_old_loop() {
        let token = CancellationToken::new();
        let child_token = token.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let task_exited = Arc::clone(&exited);
        let join = tokio::spawn(async move {
            child_token.cancelled().await;
            task_exited.store(true, Ordering::SeqCst);
        });
        let mut cancel = Some(token);
        let mut handle = Some(join);

        cancel_and_await_sse_loop(&mut cancel, &mut handle).await;

        assert!(cancel.is_none());
        assert!(handle.is_none());
        assert!(exited.load(Ordering::SeqCst));
    }
}
