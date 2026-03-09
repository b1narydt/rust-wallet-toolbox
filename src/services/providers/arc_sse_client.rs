//! ARC SSE streaming client for real-time transaction status updates.
//!
//! Ported from wallet-toolbox/src/services/providers/ArcSSEClient.ts.
//! Uses reqwest-eventsource for SSE connectivity with exponential backoff reconnection.

use std::time::Duration;

use reqwest_eventsource::{Event, EventSource};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::services::types::ArcSseEvent;

/// Options for creating an ARC SSE client.
pub struct ArcSseClientOptions {
    /// Base URL of the ARC instance.
    pub base_url: String,
    /// Stable per-wallet token matching the X-CallbackToken sent on broadcast.
    pub callback_token: String,
    /// Server-level API key for Authorization header.
    pub arc_api_key: Option<String>,
    /// Initial lastEventId for catchup.
    pub last_event_id: Option<String>,
}

/// SSE streaming client for ARC transaction status updates.
///
/// Connects to ARC's SSE endpoint for real-time status updates.
/// Supports reconnection with exponential backoff and event ID tracking.
pub struct ArcSseClient {
    base_url: String,
    callback_token: String,
    arc_api_key: Option<String>,
    last_event_id: Option<String>,
    on_last_event_id_changed: Option<Box<dyn Fn(&str) + Send + Sync>>,
    event_tx: mpsc::Sender<ArcSseEvent>,
    cancel_token: CancellationToken,
}

impl ArcSseClient {
    /// Create a new ARC SSE client and its event receiver channel.
    ///
    /// Returns the client and a receiver that will receive ArcSseEvent messages.
    pub fn new(options: ArcSseClientOptions) -> (Self, mpsc::Receiver<ArcSseEvent>) {
        let (tx, rx) = mpsc::channel(100);
        let client = Self {
            base_url: options.base_url.trim_end_matches('/').to_string(),
            callback_token: options.callback_token,
            arc_api_key: options.arc_api_key,
            last_event_id: options.last_event_id,
            on_last_event_id_changed: None,
            event_tx: tx,
            cancel_token: CancellationToken::new(),
        };
        (client, rx)
    }

    /// Set the callback that is invoked whenever lastEventId changes.
    pub fn set_on_last_event_id_changed(&mut self, cb: Box<dyn Fn(&str) + Send + Sync>) {
        self.on_last_event_id_changed = Some(cb);
    }

    /// Return the current last_event_id.
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// Cancel the SSE connection loop.
    pub fn close(&self) {
        self.cancel_token.cancel();
    }

    /// Connect to the ARC SSE endpoint and begin receiving events.
    ///
    /// Events are sent through the mpsc channel returned by `new()`.
    /// Reconnects with exponential backoff (1s, 2s, 4s, ... up to 30s).
    /// Respects the cancel_token to stop reconnection.
    pub async fn connect(&mut self) {
        let url = format!("{}/v1/tx/status/stream", self.base_url);
        let mut backoff_secs: u64 = 1;
        #[allow(unused_assignments)]
        let max_backoff_secs: u64 = 30;

        loop {
            if self.cancel_token.is_cancelled() {
                tracing::info!("[ArcSSE] Connection cancelled");
                break;
            }

            let mut request = reqwest::Client::new()
                .get(&url)
                .header("x-callback-token", &self.callback_token);

            if let Some(ref api_key) = self.arc_api_key {
                request = request.header("Authorization", format!("Bearer {}", api_key));
            }

            if let Some(ref last_id) = self.last_event_id {
                request = request.header("Last-Event-ID", last_id.as_str());
            }

            tracing::info!(
                "[ArcSSE] Connecting to {} (Last-Event-ID: {:?})",
                url,
                self.last_event_id
            );

            let mut es = EventSource::new(request).expect("Failed to create EventSource");

            // Reset backoff on successful connection
            backoff_secs = 1;

            loop {
                use futures::StreamExt;

                tokio::select! {
                    _ = self.cancel_token.cancelled() => {
                        tracing::info!("[ArcSSE] Connection cancelled during event loop");
                        es.close();
                        return;
                    }
                    event = es.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                tracing::info!("[ArcSSE] Connected");
                                backoff_secs = 1;
                            }
                            Some(Ok(Event::Message(msg))) => {
                                if msg.event == "status" {
                                    match serde_json::from_str::<ArcSseEvent>(&msg.data) {
                                        Ok(sse_event) => {
                                            tracing::debug!(
                                                "[ArcSSE] Event: txid={} status={}",
                                                sse_event.txid,
                                                sse_event.tx_status
                                            );

                                            // Update last_event_id
                                            if !msg.id.is_empty() {
                                                self.last_event_id = Some(msg.id.clone());
                                                if let Some(ref cb) = self.on_last_event_id_changed {
                                                    cb(&msg.id);
                                                }
                                            }

                                            // Send event through channel (non-blocking)
                                            if self.event_tx.try_send(sse_event).is_err() {
                                                tracing::warn!(
                                                    "[ArcSSE] Event channel full, dropping event"
                                                );
                                            }
                                        }
                                        Err(_e) => {
                                            tracing::warn!(
                                                "[ArcSSE] Malformed event data: {}",
                                                &msg.data[..msg.data.len().min(200)]
                                            );
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                tracing::warn!("[ArcSSE] Error: {}", e);
                                es.close();
                                break;
                            }
                            None => {
                                tracing::info!("[ArcSSE] Stream ended");
                                break;
                            }
                        }
                    }
                }
            }

            // Reconnection with backoff
            if self.cancel_token.is_cancelled() {
                break;
            }

            tracing::info!(
                "[ArcSSE] Reconnecting in {}s (backoff)",
                backoff_secs
            );

            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
            }

            backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
        }
    }
}
