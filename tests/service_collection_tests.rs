//! Tests for ServiceCollection round-robin failover and call history tracking.

use bsv_wallet_toolbox::error::WalletError;
use bsv_wallet_toolbox::services::service_collection::ServiceCollection;
use bsv_wallet_toolbox::services::types::{ServiceCall, MAX_CALL_HISTORY, MAX_RESET_COUNTS};
use chrono::Utc;

// ---------------------------------------------------------------------------
// Mock provider trait and implementation for testing
// ---------------------------------------------------------------------------

// `name`/`name_val` are kept for parity with the provider trait shape used
// in production (`WalletServices`-adjacent providers expose a `name`) even
// though the round-robin failover tests below only exercise insertion order.
#[allow(dead_code)]
trait MockProvider: Send + Sync {
    fn name(&self) -> &str;
}

#[allow(dead_code)]
struct TestProvider {
    name_val: String,
}

impl TestProvider {
    fn new(name: &str) -> Self {
        Self {
            name_val: name.to_string(),
        }
    }
}

impl MockProvider for TestProvider {
    fn name(&self) -> &str {
        &self.name_val
    }
}

fn make_call(success: bool) -> ServiceCall {
    ServiceCall {
        when: Utc::now(),
        msecs: 42,
        success,
        result: if success {
            Some("ok".to_string())
        } else {
            None
        },
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn round_robin_cycles_through_three_providers() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProviderA", Box::new(TestProvider::new("ProviderA")));
    sc.add("ProviderB", Box::new(TestProvider::new("ProviderB")));
    sc.add("ProviderC", Box::new(TestProvider::new("ProviderC")));

    // First call should be at index 0
    let (_, name) = sc.service_to_call().unwrap();
    assert_eq!(name, "ProviderA");

    // Advance to next
    sc.next();
    let (_, name) = sc.service_to_call().unwrap();
    assert_eq!(name, "ProviderB");

    sc.next();
    let (_, name) = sc.service_to_call().unwrap();
    assert_eq!(name, "ProviderC");

    // Wraps around
    sc.next();
    let (_, name) = sc.service_to_call().unwrap();
    assert_eq!(name, "ProviderA");
}

#[test]
fn move_service_to_last_reorders_correctly() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("Alpha", Box::new(TestProvider::new("Alpha")));
    sc.add("Beta", Box::new(TestProvider::new("Beta")));
    sc.add("Gamma", Box::new(TestProvider::new("Gamma")));

    // Move Alpha to last
    sc.move_service_to_last("Alpha");

    // Order should now be: Beta, Gamma, Alpha
    let names: Vec<&str> = sc.all_services().map(|(_, n)| n).collect();
    assert_eq!(names, vec!["Beta", "Gamma", "Alpha"]);
}

#[test]
fn add_service_call_success_records_in_history() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProvA", Box::new(TestProvider::new("ProvA")));

    let call = make_call(true);
    sc.add_service_call_success("ProvA", call, Some("result text".to_string()));

    let history = sc.get_service_call_history(false);
    let prov = history.history_by_provider.get("ProvA").unwrap();

    assert_eq!(prov.total_counts.success, 1);
    assert_eq!(prov.total_counts.failure, 0);
    assert_eq!(prov.total_counts.error, 0);
    assert_eq!(prov.calls.len(), 1);
    assert!(prov.calls[0].success);
}

#[test]
fn add_service_call_error_increments_error_count() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProvA", Box::new(TestProvider::new("ProvA")));

    let call = make_call(false);
    let err = WalletError::Internal("something failed".to_string());
    sc.add_service_call_error("ProvA", call, &err);

    let history = sc.get_service_call_history(false);
    let prov = history.history_by_provider.get("ProvA").unwrap();

    assert_eq!(prov.total_counts.success, 0);
    assert_eq!(prov.total_counts.failure, 1);
    assert_eq!(prov.total_counts.error, 1);
    assert_eq!(prov.calls.len(), 1);
    assert!(!prov.calls[0].success);
    let call_err = prov.calls[0].error.as_ref().unwrap();
    assert!(call_err.message.contains("something failed"));
    assert_eq!(call_err.code, "WERR_INTERNAL");
}

#[test]
fn get_service_call_history_with_reset_creates_new_interval() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProvA", Box::new(TestProvider::new("ProvA")));

    // Record two successes
    sc.add_service_call_success("ProvA", make_call(true), None);
    sc.add_service_call_success("ProvA", make_call(true), None);

    // Reset
    let history = sc.get_service_call_history(true);
    let prov = history.history_by_provider.get("ProvA").unwrap();

    // The returned history should show 2 successes from before reset
    assert_eq!(prov.total_counts.success, 2);
    // reset_counts[0] should be the pre-reset interval with 2 successes
    assert_eq!(prov.reset_counts[0].success, 2);

    // After reset, recording another call should go into the new interval
    sc.add_service_call_success("ProvA", make_call(true), None);
    let history2 = sc.get_service_call_history(false);
    let prov2 = history2.history_by_provider.get("ProvA").unwrap();

    // Total counts keeps accumulating
    assert_eq!(prov2.total_counts.success, 3);
    // reset_counts[0] is the new interval (after reset) with 1 success
    assert_eq!(prov2.reset_counts[0].success, 1);
    // reset_counts[1] is the old interval with 2 successes
    assert_eq!(prov2.reset_counts[1].success, 2);
}

#[test]
fn history_respects_max_call_history_limit() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProvA", Box::new(TestProvider::new("ProvA")));

    // Add more than MAX_CALL_HISTORY calls
    for _ in 0..(MAX_CALL_HISTORY + 10) {
        sc.add_service_call_success("ProvA", make_call(true), None);
    }

    let history = sc.get_service_call_history(false);
    let prov = history.history_by_provider.get("ProvA").unwrap();

    // Calls should be capped at MAX_CALL_HISTORY
    assert_eq!(prov.calls.len(), MAX_CALL_HISTORY);
    // But total counts should reflect all calls
    assert_eq!(prov.total_counts.success, (MAX_CALL_HISTORY + 10) as u32);
}

#[test]
fn history_respects_max_reset_counts_limit() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("ProvA", Box::new(TestProvider::new("ProvA")));

    // Record a call so the provider has history
    sc.add_service_call_success("ProvA", make_call(true), None);

    // Reset more than MAX_RESET_COUNTS times
    for _ in 0..(MAX_RESET_COUNTS + 5) {
        sc.get_service_call_history(true);
    }

    // Now get history without reset to check the count
    let history = sc.get_service_call_history(false);
    let prov = history.history_by_provider.get("ProvA").unwrap();

    // reset_counts should be capped at MAX_RESET_COUNTS
    assert!(prov.reset_counts.len() <= MAX_RESET_COUNTS);
}

#[test]
fn empty_collection_returns_none() {
    let sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("empty");
    assert!(sc.service_to_call().is_none());
    assert!(sc.is_empty());
    assert_eq!(sc.len(), 0);
}

#[test]
fn all_services_iterates_all_providers() {
    let mut sc: ServiceCollection<dyn MockProvider> = ServiceCollection::new("test_service");
    sc.add("A", Box::new(TestProvider::new("A")));
    sc.add("B", Box::new(TestProvider::new("B")));

    let names: Vec<&str> = sc.all_services().map(|(_, n)| n).collect();
    assert_eq!(names, vec!["A", "B"]);
    assert_eq!(sc.len(), 2);
    assert!(!sc.is_empty());
}
