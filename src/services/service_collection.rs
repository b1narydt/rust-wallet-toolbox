//! Generic ServiceCollection with round-robin failover and call history tracking.
//!
//! Ported from wallet-toolbox/src/services/ServiceCollection.ts.
//! Provides round-robin cycling through providers, call history tracking
//! (per-provider success/failure/error counts), and provider reordering.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::types::{
    ProviderCallHistory, ServiceCall, ServiceCallError, ServiceCallHistory,
    ServiceCallHistoryCounts, MAX_CALL_HISTORY, MAX_RESET_COUNTS,
};
use crate::error::WalletError;

/// A named service provider entry in the collection.
struct NamedService<T: ?Sized> {
    name: Arc<str>,
    service: Arc<T>,
}

/// Generic collection of service providers with round-robin failover
/// and per-provider call history tracking.
///
/// Type parameter `T` is a provider trait (e.g., `dyn GetMerklePathProvider`).
/// Intentionally not `Clone`: although `Arc` providers make it structurally
/// cloneable, copies would share providers while cursors and call histories diverged.
pub struct ServiceCollection<T: ?Sized> {
    service_name: String,
    services: Vec<NamedService<T>>,
    index: usize,
    since: DateTime<Utc>,
    history_by_provider: HashMap<String, InternalProviderHistory>,
}

/// Internal mutable history for a single provider (not exposed directly).
struct InternalProviderHistory {
    service_name: String,
    provider_name: String,
    calls: Vec<ServiceCall>,
    total_counts: ServiceCallHistoryCounts,
    reset_counts: Vec<ServiceCallHistoryCounts>,
}

impl<T: ?Sized> ServiceCollection<T> {
    /// Create a new empty ServiceCollection for a named service type.
    pub fn new(service_name: &str) -> Self {
        let now = Utc::now();
        Self {
            service_name: service_name.to_string(),
            services: Vec::new(),
            index: 0,
            since: now,
            history_by_provider: HashMap::new(),
        }
    }

    /// Add a provider to the collection.
    pub fn add(&mut self, name: &str, service: Arc<T>) {
        self.services.push(NamedService {
            name: Arc::from(name),
            service,
        });
    }

    /// Number of providers in the collection.
    pub fn len(&self) -> usize {
        self.services.len()
    }

    /// Whether the collection is empty.
    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    /// Returns the service name of this collection.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Advance to the next provider (wraps around). Returns the new index.
    //
    // This is NOT `Iterator::next` — the method advances the internal
    // round-robin cursor and returns the resulting index (a `usize`),
    // not an `Option<Self::Item>`. The public API intentionally mirrors
    // the TypeScript wallet-toolbox `ServiceCollection.next()` shape and
    // renaming would be a breaking change. `ServiceCollection` is not
    // meant to be an `Iterator`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> usize {
        if !self.services.is_empty() {
            self.index = (self.index + 1) % self.services.len();
        }
        self.index
    }

    /// Providers in dispatch order: index 0 is the provider the round-robin cursor
    /// currently points at, followed by the rest in round-robin order.
    ///
    /// This is a SNAPSHOT. Concurrent `next()` or `move_service_to_last()` calls
    /// affect subsequent dispatches, not a walk already in progress over this Vec.
    /// Returning the pre-rotated order in a single call is deliberate: callers must
    /// not be able to pair a cursor index with a provider list read under a
    /// different lock acquisition, which would index an order that had since rotated.
    pub fn call_order(&self) -> Vec<(Arc<T>, Arc<str>)> {
        let n = self.services.len();
        (0..n)
            .map(|i| {
                let entry = &self.services[(self.index + i) % n];
                (Arc::clone(&entry.service), Arc::clone(&entry.name))
            })
            .collect()
    }

    /// Move a named provider to the end of the list, preserving others' order.
    /// Used to de-prioritize a failing provider.
    pub fn move_service_to_last(&mut self, provider_name: &str) {
        if let Some(pos) = self
            .services
            .iter()
            .position(|s| s.name.as_ref() == provider_name)
        {
            let entry = self.services.remove(pos);
            self.services.push(entry);
            // Deliberately leave the cursor alone here. Since remove/push preserves
            // the length, the guard below cannot fire; the cursor can still point at
            // the de-prioritized provider, so the next dispatch may try it first.
            // This known defect is tracked separately, and `call_order()` now makes
            // it fixable inside the type.
            if self.index >= self.services.len() {
                self.index = 0;
            }
        }
    }

    /// Record a successful service call for a provider.
    pub fn add_service_call_success(
        &mut self,
        provider: &str,
        call: ServiceCall,
        _result: Option<String>,
    ) {
        let h = self.ensure_provider_history(provider);
        h.calls.insert(0, call);
        h.calls.truncate(MAX_CALL_HISTORY);
        let now = Utc::now();
        h.total_counts.until = now;
        h.total_counts.success += 1;
        if let Some(rc) = h.reset_counts.first_mut() {
            rc.until = now;
            rc.success += 1;
        }
    }

    /// Record a failed service call (with error) for a provider.
    pub fn add_service_call_error(
        &mut self,
        provider: &str,
        mut call: ServiceCall,
        error: &WalletError,
    ) {
        call.success = false;
        call.error = Some(ServiceCallError::from_wallet_error(error));
        let h = self.ensure_provider_history(provider);
        h.calls.insert(0, call);
        h.calls.truncate(MAX_CALL_HISTORY);
        let now = Utc::now();
        h.total_counts.until = now;
        h.total_counts.failure += 1;
        h.total_counts.error += 1;
        if let Some(rc) = h.reset_counts.first_mut() {
            rc.until = now;
            rc.failure += 1;
            rc.error += 1;
        }
    }

    /// Record a failed service call (without a thrown error) for a provider.
    pub fn add_service_call_failure(&mut self, provider: &str, call: ServiceCall) {
        let h = self.ensure_provider_history(provider);
        h.calls.insert(0, call);
        h.calls.truncate(MAX_CALL_HISTORY);
        let now = Utc::now();
        h.total_counts.until = now;
        h.total_counts.failure += 1;
        if let Some(rc) = h.reset_counts.first_mut() {
            rc.until = now;
            rc.failure += 1;
        }
    }

    /// Returns the per-provider call history. If `reset` is true, starts a new
    /// counting interval (pushes current counts to reset_counts and zeroes).
    pub fn get_service_call_history(&mut self, reset: bool) -> ServiceCallHistory {
        let now = Utc::now();
        let mut history_by_provider = HashMap::new();

        for (name, h) in &mut self.history_by_provider {
            let provider_history = ProviderCallHistory {
                service_name: h.service_name.clone(),
                provider_name: h.provider_name.clone(),
                calls: h.calls.clone(),
                total_counts: h.total_counts.clone(),
                reset_counts: h.reset_counts.clone(),
            };

            if reset {
                // Close the current interval
                if let Some(rc) = h.reset_counts.first_mut() {
                    rc.until = now;
                }
                // Insert a new zero-count interval at the front
                h.reset_counts
                    .insert(0, ServiceCallHistoryCounts::new_at(now));
                h.reset_counts.truncate(MAX_RESET_COUNTS);
            }

            history_by_provider.insert(name.clone(), provider_history);
        }

        ServiceCallHistory {
            service_name: self.service_name.clone(),
            history_by_provider,
        }
    }

    /// Ensure a provider history entry exists, creating one if needed.
    fn ensure_provider_history(&mut self, provider_name: &str) -> &mut InternalProviderHistory {
        self.history_by_provider
            .entry(provider_name.to_string())
            .or_insert_with(|| {
                let now = Utc::now();
                InternalProviderHistory {
                    service_name: self.service_name.clone(),
                    provider_name: provider_name.to_string(),
                    calls: Vec::new(),
                    total_counts: ServiceCallHistoryCounts {
                        success: 0,
                        failure: 0,
                        error: 0,
                        since: self.since,
                        until: now,
                    },
                    reset_counts: vec![ServiceCallHistoryCounts {
                        success: 0,
                        failure: 0,
                        error: 0,
                        since: self.since,
                        until: now,
                    }],
                }
            })
    }
}
