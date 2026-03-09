//! WalletSettingsManager -- cached wallet settings with TTL.
//!
//! Manages wallet settings (trust settings, theme, currency, permissions)
//! with a 2-minute cache to avoid repeated storage reads. Port of the
//! TypeScript `WalletSettingsManager` class.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::WalletResult;
use crate::storage::traits::provider::StorageProvider;

/// Cache time-to-live: 2 minutes.
const CACHE_TTL: Duration = Duration::from_secs(120);

/// Settings basket key used for storage operations.
const _SETTINGS_KEY: &str = "settings";

/// Settings basket name.
const _SETTINGS_BASKET: &str = "wallet settings";

// ---------------------------------------------------------------------------
// Settings types
// ---------------------------------------------------------------------------

/// A trusted certifier entry in the trust settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedCertifier {
    /// Human-readable name of the certifier.
    pub name: String,
    /// Description of the certifier's purpose.
    pub description: String,
    /// Compressed public key hex of the certifier.
    pub identity_key: String,
    /// Trust level (higher = more trusted).
    pub trust: u32,
    /// Optional icon URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Optional base URL for the certifier service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Trust settings controlling which certifiers the wallet trusts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustSettings {
    /// Overall trust level for the wallet.
    pub trust_level: u32,
    /// List of trusted certifiers.
    pub trusted_certifiers: Vec<TrustedCertifier>,
}

/// Wallet theme settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletTheme {
    /// Theme mode (e.g. "dark", "light").
    pub mode: String,
}

/// Complete wallet settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletSettings {
    /// Trust settings for certifier management.
    pub trust_settings: TrustSettings,
    /// Optional UI theme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<WalletTheme>,
    /// Optional preferred currency code (e.g. "USD").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Optional vendor-specific permission UX mode identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

// ---------------------------------------------------------------------------
// Default settings (matching TS DEFAULT_SETTINGS)
// ---------------------------------------------------------------------------

/// Returns the default wallet settings matching the TS `DEFAULT_SETTINGS`.
pub fn default_settings() -> WalletSettings {
    WalletSettings {
        trust_settings: TrustSettings {
            trust_level: 2,
            trusted_certifiers: vec![
                TrustedCertifier {
                    name: "Metanet Trust Services".to_string(),
                    description: "Registry for protocols, baskets, and certificates types"
                        .to_string(),
                    icon_url: Some("https://bsvblockchain.org/favicon.ico".to_string()),
                    identity_key:
                        "03daf815fe38f83da0ad83b5bedc520aa488aef5cbc93a93c67a7fe60406cbffe8"
                            .to_string(),
                    trust: 4,
                    base_url: None,
                },
                TrustedCertifier {
                    name: "SocialCert".to_string(),
                    description: "Certifies social media handles, phone numbers and emails"
                        .to_string(),
                    icon_url: Some("https://socialcert.net/favicon.ico".to_string()),
                    trust: 3,
                    identity_key:
                        "02cf6cdf466951d8dfc9e7c9367511d0007ed6fba35ed42d425cc412fd6cfd4a17"
                            .to_string(),
                    base_url: None,
                },
            ],
        },
        theme: Some(WalletTheme {
            mode: "dark".to_string(),
        }),
        currency: None,
        permission_mode: Some("simple".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Cache internals
// ---------------------------------------------------------------------------

/// A cached value with an expiration time.
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> CacheEntry<T> {
    fn new(value: T, ttl: Duration) -> Self {
        CacheEntry {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

// ---------------------------------------------------------------------------
// WalletSettingsManager
// ---------------------------------------------------------------------------

/// Manages wallet settings with a 2-minute TTL cache.
///
/// Reads settings from storage via a key-value pattern. If no settings
/// exist in storage, returns the default settings. Settings are cached
/// in memory for 2 minutes to avoid repeated storage reads.
pub struct WalletSettingsManager {
    /// Storage provider for reading/writing settings.
    _storage: Arc<dyn StorageProvider>,
    /// Cached settings with TTL.
    cache: Mutex<Option<CacheEntry<WalletSettings>>>,
    /// Default settings to use when none are stored.
    default_settings: WalletSettings,
}

impl WalletSettingsManager {
    /// Create a new WalletSettingsManager with the given storage provider.
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self::with_defaults(storage, default_settings())
    }

    /// Create a new WalletSettingsManager with custom default settings.
    pub fn with_defaults(
        storage: Arc<dyn StorageProvider>,
        default_settings: WalletSettings,
    ) -> Self {
        WalletSettingsManager {
            _storage: storage,
            cache: Mutex::new(None),
            default_settings,
        }
    }

    /// Get the current wallet settings.
    ///
    /// Returns cached settings if the cache is still fresh (within 2-minute TTL).
    /// Otherwise reads from storage, updates the cache, and returns the settings.
    /// If no settings exist in storage, returns the default settings.
    pub async fn get(&self) -> WalletResult<WalletSettings> {
        let mut cache = self.cache.lock().await;

        // Check if cache is still fresh
        if let Some(ref entry) = *cache {
            if entry.is_fresh() {
                return Ok(entry.value.clone());
            }
        }

        // TODO: Read from storage via LocalKVStore pattern when available.
        // For now, return default settings and cache them.
        let settings = self.default_settings.clone();

        *cache = Some(CacheEntry::new(settings.clone(), CACHE_TTL));
        Ok(settings)
    }

    /// Set new wallet settings (also updates the cache).
    pub async fn set(&self, settings: WalletSettings) -> WalletResult<()> {
        // TODO: Write to storage via LocalKVStore pattern when available.

        // Update cache immediately
        let mut cache = self.cache.lock().await;
        *cache = Some(CacheEntry::new(settings, CACHE_TTL));
        Ok(())
    }

    /// Clear the cached settings, forcing a fresh read on next `get()`.
    pub async fn invalidate_cache(&self) {
        let mut cache = self.cache.lock().await;
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings_has_two_certifiers() {
        let settings = default_settings();
        assert_eq!(settings.trust_settings.trusted_certifiers.len(), 2);
        assert_eq!(settings.trust_settings.trust_level, 2);
    }

    #[test]
    fn test_settings_serialization_roundtrip() {
        let settings = default_settings();
        let json = serde_json::to_string(&settings).unwrap();
        let deserialized: WalletSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.trust_settings.trusted_certifiers.len(),
            settings.trust_settings.trusted_certifiers.len()
        );
    }
}
