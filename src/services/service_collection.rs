//! Generic ServiceCollection with round-robin failover and call history tracking.
//!
//! Ported from wallet-toolbox/src/services/ServiceCollection.ts.
//! Provides round-robin cycling through providers, call history tracking
//! (per-provider success/failure/error counts), and provider reordering.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::types::{
    ProviderCallHistory, ServiceCall, ServiceCallError, ServiceCallHistory,
    ServiceCallHistoryCounts, MAX_CALL_HISTORY, MAX_RESET_COUNTS,
};
use crate::error::WalletError;

/// A named service provider entry in the collection.
struct NamedService<T: ?Sized> {
    name: String,
    service: Box<T>,
}

/// Generic collection of service providers with round-robin failover
/// and per-provider call history tracking.
///
/// Type parameter `T` is a provider trait (e.g., `dyn GetMerklePathProvider`).
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
    pub fn add(&mut self, name: &str, service: Box<T>) {
        self.services.push(NamedService {
            name: name.to_string(),
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

    /// Returns the current provider and its name, or None if empty.
    pub fn service_to_call(&self) -> Option<(&T, &str)> {
        if self.services.is_empty() {
            return None;
        }
        let entry = &self.services[self.index];
        Some((&*entry.service, &entry.name))
    }

    /// Returns the service name of this collection.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Advance to the next provider (wraps around). Returns the new index.
    pub fn next(&mut self) -> usize {
        if !self.services.is_empty() {
            self.index = (self.index + 1) % self.services.len();
        }
        self.index
    }

    /// Iterate over all providers and their names.
    pub fn all_services(&self) -> impl Iterator<Item = (&T, &str)> {
        self.services
            .iter()
            .map(|entry| (&*entry.service, entry.name.as_str()))
    }

    /// Move a named provider to the end of the list, preserving others' order.
    /// Used to de-prioritize a failing provider.
    pub fn move_service_to_last(&mut self, provider_name: &str) {
        if let Some(pos) = self.services.iter().position(|s| s.name == provider_name) {
            let entry = self.services.remove(pos);
            self.services.push(entry);
            // Reset index to 0 so next call starts from the new front
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
