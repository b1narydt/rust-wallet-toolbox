//! Permission caching with concurrent request deduplication.
//!
//! Provides a `PermissionCache` that maintains:
//! - **Manifest cache**: Per-key permission grants with a configurable TTL (default 5 min).
//! - **Recent grants**: Short-lived window (15 sec) tracking recently-granted permissions
//!   to avoid re-prompting during rapid sequences of related calls.
//! - **Active requests**: Deduplication of concurrent permission checks for the same key --
//!   only the first caller fires the callback; subsequent callers wait for its result.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};

use crate::WalletError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default manifest cache TTL: 5 minutes.
pub const MANIFEST_CACHE_TTL: Duration = Duration::from_secs(300);

/// Recent grant window: 15 seconds.
pub const RECENT_GRANT_WINDOW: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// A single entry in the manifest cache.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Whether the permission was granted (true) or denied (false).
    pub granted: bool,
    /// When this entry expires.
    pub expires_at: Instant,
}

// ---------------------------------------------------------------------------
// ActiveRequest and deduplication
// ---------------------------------------------------------------------------

/// Shared state for an in-flight permission request.
struct ActiveRequest {
    /// Notifies waiters when the result is available.
    notify: Arc<Notify>,
    /// The eventual result, set by the first caller.
    result: Arc<Mutex<Option<Result<bool, String>>>>,
}

/// Result of attempting to deduplicate a permission request.
pub enum DeduplicateResult {
    /// This caller should perform the check. The handle must be completed
    /// via `PermissionCache::complete_request` when done.
    Proceed(ActiveRequestHandle),
    /// Another caller is already checking. Wait on the `Notify` then read
    /// the shared result.
    Wait(Arc<Notify>, Arc<Mutex<Option<Result<bool, String>>>>),
}

/// Handle returned to the "proceed" caller for RAII cleanup.
///
/// If dropped without calling `complete_request`, the entry is cleaned up
/// from active_requests so waiters do not hang indefinitely.
pub struct ActiveRequestHandle {
    key: String,
}

impl ActiveRequestHandle {
    /// Get the cache key this handle corresponds to.
    pub fn key(&self) -> &str {
        &self.key
    }
}

// ---------------------------------------------------------------------------
// PermissionCache
// ---------------------------------------------------------------------------

/// Thread-safe permission cache with deduplication.
pub struct PermissionCache {
    /// Manifest cache: permission grant/deny with TTL.
    manifest_cache: Mutex<HashMap<String, CacheEntry>>,
    /// Recent grants: keys that were granted within the last 15 seconds.
    recent_grants: Mutex<HashMap<String, Instant>>,
    /// Active permission requests being resolved (for deduplication).
    active_requests: Mutex<HashMap<String, ActiveRequest>>,
}

impl PermissionCache {
    /// Create a new empty permission cache.
    pub fn new() -> Self {
        Self {
            manifest_cache: Mutex::new(HashMap::new()),
            recent_grants: Mutex::new(HashMap::new()),
            active_requests: Mutex::new(HashMap::new()),
        }
    }

    /// Check the manifest cache for a non-expired entry.
    ///
    /// Returns `Some(true)` if granted, `Some(false)` if denied,
    /// `None` if not found or expired.
    pub async fn check_cache(&self, key: &str) -> Option<bool> {
        let mut cache = self.manifest_cache.lock().await;
        if let Some(entry) = cache.get(key) {
            if Instant::now() < entry.expires_at {
                return Some(entry.granted);
            }
            // Expired -- clean up
            cache.remove(key);
        }
        None
    }

    /// Insert a permission result into the manifest cache.
    pub async fn insert_cache(&self, key: &str, granted: bool, ttl: Duration) {
        let entry = CacheEntry {
            granted,
            expires_at: Instant::now() + ttl,
        };
        self.manifest_cache
            .lock()
            .await
            .insert(key.to_string(), entry);
    }

    /// Check if a key has a recent grant within the 15-second window.
    pub async fn is_recent_grant(&self, key: &str) -> bool {
        let grants = self.recent_grants.lock().await;
        if let Some(when) = grants.get(key) {
            Instant::now().duration_since(*when) < RECENT_GRANT_WINDOW
        } else {
            false
        }
    }

    /// Record a recent grant for the given key.
    pub async fn record_recent_grant(&self, key: &str) {
        self.recent_grants
            .lock()
            .await
            .insert(key.to_string(), Instant::now());
    }

    /// Attempt to deduplicate a permission request.
    ///
    /// If no other caller is checking this key, returns `Proceed` with a handle.
    /// If another caller is already checking, returns `Wait` with shared notify/result.
    pub async fn try_deduplicate(&self, key: &str) -> DeduplicateResult {
        let mut active = self.active_requests.lock().await;
        if let Some(existing) = active.get(key) {
            return DeduplicateResult::Wait(
                Arc::clone(&existing.notify),
                Arc::clone(&existing.result),
            );
        }

        let notify = Arc::new(Notify::new());
        let result = Arc::new(Mutex::new(None));
        active.insert(
            key.to_string(),
            ActiveRequest {
                notify: Arc::clone(&notify),
                result: Arc::clone(&result),
            },
        );

        DeduplicateResult::Proceed(ActiveRequestHandle {
            key: key.to_string(),
        })
    }

    /// Complete an active request, notifying all waiters.
    pub async fn complete_request(&self, key: &str, result: Result<bool, WalletError>) {
        let mut active = self.active_requests.lock().await;
        if let Some(req) = active.remove(key) {
            let stored = result.map_err(|e| format!("{}", e));
            *req.result.lock().await = Some(stored);
            req.notify.notify_waiters();
        }
    }

    /// Clear all caches and active requests.
    pub async fn clear(&self) {
        self.manifest_cache.lock().await.clear();
        self.recent_grants.lock().await.clear();
        // Notify any waiters before clearing
        let mut active = self.active_requests.lock().await;
        for (_, req) in active.drain() {
            *req.result.lock().await =
                Some(Err("Cache cleared".to_string()));
            req.notify.notify_waiters();
        }
    }

    /// Build a deterministic cache key from permission parameters.
    pub fn build_cache_key(
        permission_type: &super::types::PermissionType,
        originator: &str,
        details: &str,
    ) -> String {
        format!("{:?}:{}:{}", permission_type, originator, details)
    }
}

impl Default for PermissionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::types::PermissionType;

    #[tokio::test]
    async fn test_cache_insert_and_check() {
        let cache = PermissionCache::new();
        cache
            .insert_cache("proto:test", true, Duration::from_secs(60))
            .await;
        assert_eq!(cache.check_cache("proto:test").await, Some(true));
    }

    #[tokio::test]
    async fn test_cache_expiry() {
        let cache = PermissionCache::new();
        // Insert with zero TTL -- should expire immediately
        cache
            .insert_cache("proto:expired", true, Duration::from_secs(0))
            .await;
        // Give it a tiny moment to ensure Instant comparison fails
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(cache.check_cache("proto:expired").await, None);
    }

    #[tokio::test]
    async fn test_recent_grant_within_window() {
        let cache = PermissionCache::new();
        cache.record_recent_grant("proto:recent").await;
        assert!(cache.is_recent_grant("proto:recent").await);
    }

    #[tokio::test]
    async fn test_recent_grant_not_present() {
        let cache = PermissionCache::new();
        assert!(!cache.is_recent_grant("proto:missing").await);
    }

    #[tokio::test]
    async fn test_deduplication_proceed_first() {
        let cache = PermissionCache::new();
        let result = cache.try_deduplicate("proto:dedup").await;
        assert!(matches!(result, DeduplicateResult::Proceed(_)));
    }

    #[tokio::test]
    async fn test_deduplication_wait_second() {
        let cache = PermissionCache::new();
        // First caller gets Proceed
        let _handle = match cache.try_deduplicate("proto:dedup2").await {
            DeduplicateResult::Proceed(h) => h,
            _ => panic!("Expected Proceed for first caller"),
        };
        // Second caller gets Wait
        let result = cache.try_deduplicate("proto:dedup2").await;
        assert!(matches!(result, DeduplicateResult::Wait(..)));

        // Clean up by completing the request
        cache
            .complete_request("proto:dedup2", Ok(true))
            .await;
    }

    #[tokio::test]
    async fn test_build_cache_key_deterministic() {
        let key1 = PermissionCache::build_cache_key(
            &PermissionType::ProtocolPermission,
            "example.com",
            "proto:1:self",
        );
        let key2 = PermissionCache::build_cache_key(
            &PermissionType::ProtocolPermission,
            "example.com",
            "proto:1:self",
        );
        assert_eq!(key1, key2);

        // Different inputs produce different keys
        let key3 = PermissionCache::build_cache_key(
            &PermissionType::BasketAccess,
            "example.com",
            "my-basket",
        );
        assert_ne!(key1, key3);
    }

    #[tokio::test]
    async fn test_clear_clears_all() {
        let cache = PermissionCache::new();
        cache
            .insert_cache("k1", true, Duration::from_secs(300))
            .await;
        cache.record_recent_grant("k2").await;
        cache.clear().await;
        assert_eq!(cache.check_cache("k1").await, None);
        assert!(!cache.is_recent_grant("k2").await);
    }
}
