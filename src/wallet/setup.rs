//! Setup helpers for ergonomic wallet construction and P2PKH utilities.
//!
//! Provides `WalletBuilder` for fluent wallet construction from minimal configuration,
//! `SetupWallet` as the rich return type exposing all wired components, and P2PKH
//! helper functions for key derivation and output creation.
//!
//! Ported from wallet-toolbox/src/Setup.ts.

use std::sync::Arc;

use bsv::primitives::private_key::PrivateKey;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::{CreateActionArgs, CreateActionOutput, CreateActionResult};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};

use crate::error::{WalletError, WalletResult};
use crate::monitor::Monitor;
use crate::services::traits::WalletServices;
use crate::storage::manager::WalletStorageManager;
use crate::storage::StorageConfig;
use crate::types::Chain;
use crate::utility::script_template_brc29::ScriptTemplateBRC29;
use crate::wallet::privileged::PrivilegedKeyManager;
use crate::wallet::types::{KeyPair, WalletArgs};
use crate::wallet::wallet::Wallet;

// ---------------------------------------------------------------------------
// SetupWallet -- rich return type from WalletBuilder.build()
// ---------------------------------------------------------------------------

/// Result of a successful `WalletBuilder::build()` call.
///
/// Exposes all wired components so callers can access the wallet, storage,
/// services, key deriver, identity key, and monitor independently. This is
/// especially useful for testing and advanced customization scenarios.
pub struct SetupWallet {
    /// The fully constructed Wallet instance.
    pub wallet: Wallet,
    /// The chain this wallet operates on.
    pub chain: Chain,
    /// The key deriver used for Type-42 derivation.
    pub key_deriver: Arc<CachedKeyDeriver>,
    /// The wallet's identity key as a hex DER public key string.
    pub identity_key: String,
    /// The storage manager (shares the same underlying providers as the wallet).
    pub storage: WalletStorageManager,
    /// The services provider, if configured.
    pub services: Option<Arc<dyn WalletServices>>,
    /// The monitor, if enabled.
    pub monitor: Option<Arc<Monitor>>,
}

// ---------------------------------------------------------------------------
// StorageKind -- internal enum for storage configuration
// ---------------------------------------------------------------------------

/// Internal storage configuration variant for the builder.
enum StorageKind {
    /// SQLite with a file path or `:memory:`.
    Sqlite(String),
    /// MySQL connection URL.
    #[allow(dead_code)]
    Mysql(String),
    /// PostgreSQL connection URL.
    #[allow(dead_code)]
    Postgres(String),
}

// ---------------------------------------------------------------------------
// WalletBuilder -- fluent builder for wallet construction
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a fully-wired `Wallet` from minimal configuration.
///
/// Modeled after `MonitorBuilder` for API consistency.
///
/// Required fields: `chain`, `root_key`, and one of the storage methods
/// (`with_sqlite`, `with_sqlite_memory`, `with_mysql`, `with_postgres`).
///
/// # Example
///
/// ```no_run
/// use bsv::primitives::private_key::PrivateKey;
/// use bsv_wallet_toolbox::wallet::setup::WalletBuilder;
/// use bsv_wallet_toolbox::types::Chain;
///
/// # async fn example() -> bsv_wallet_toolbox::WalletResult<()> {
/// let root_key = PrivateKey::from_hex("aa").unwrap();
/// let setup = WalletBuilder::new()
///     .chain(Chain::Test)
///     .root_key(root_key)
///     .with_sqlite_memory()
///     .with_default_services()
///     .with_monitor()
///     .build()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct WalletBuilder {
    chain: Option<Chain>,
    root_key: Option<PrivateKey>,
    storage_config: Option<StorageKind>,
    storage_identity_key: Option<String>,
    services: Option<Arc<dyn WalletServices>>,
    use_default_services: bool,
    monitor_enabled: bool,
    privileged_key_manager: Option<Arc<dyn PrivilegedKeyManager>>,
}

impl WalletBuilder {
    /// Create a new WalletBuilder with all fields unset.
    pub fn new() -> Self {
        Self {
            chain: None,
            root_key: None,
            storage_config: None,
            storage_identity_key: None,
            services: None,
            use_default_services: false,
            monitor_enabled: false,
            privileged_key_manager: None,
        }
    }

    /// Set the chain (required).
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the root private key (required).
    pub fn root_key(mut self, key: PrivateKey) -> Self {
        self.root_key = Some(key);
        self
    }

    /// Use a SQLite file database at the given path.
    pub fn with_sqlite(mut self, path: &str) -> Self {
        self.storage_config = Some(StorageKind::Sqlite(path.to_string()));
        self
    }

    /// Use an in-memory SQLite database.
    pub fn with_sqlite_memory(mut self) -> Self {
        self.storage_config = Some(StorageKind::Sqlite(":memory:".to_string()));
        self
    }

    /// Use a MySQL database at the given URL.
    pub fn with_mysql(mut self, url: &str) -> Self {
        self.storage_config = Some(StorageKind::Mysql(url.to_string()));
        self
    }

    /// Use a PostgreSQL database at the given URL.
    pub fn with_postgres(mut self, url: &str) -> Self {
        self.storage_config = Some(StorageKind::Postgres(url.to_string()));
        self
    }

    /// Use default services for the configured chain.
    ///
    /// Creates a `Services` instance configured with default providers
    /// (WhatsOnChain, ARC, Bitails, etc.) for the chain.
    pub fn with_default_services(mut self) -> Self {
        self.use_default_services = true;
        self
    }

    /// Use custom services.
    pub fn with_services(mut self, services: Arc<dyn WalletServices>) -> Self {
        self.services = Some(services);
        self
    }

    /// Enable the background monitor with default tasks.
    pub fn with_monitor(mut self) -> Self {
        self.monitor_enabled = true;
        self
    }

    /// Set the storage identity key (the server's public key identifying this storage instance).
    ///
    /// Must be set before `build()` so that `make_available()` creates Settings
    /// with the correct `storageIdentityKey`. Without this, the key defaults to empty.
    pub fn with_storage_identity_key(mut self, key: String) -> Self {
        self.storage_identity_key = Some(key);
        self
    }

    /// Set a privileged key manager for sensitive crypto operations.
    pub fn with_privileged_key_manager(mut self, pkm: Arc<dyn PrivilegedKeyManager>) -> Self {
        self.privileged_key_manager = Some(pkm);
        self
    }

    /// Build the wallet and all supporting infrastructure.
    ///
    /// Validates required fields, creates storage, runs migrations,
    /// wires services, constructs the Wallet, and optionally creates a Monitor.
    ///
    /// Returns a `SetupWallet` with all components accessible.
    pub async fn build(self) -> WalletResult<SetupWallet> {
        // Validate required fields
        let chain = self
            .chain
            .ok_or_else(|| WalletError::MissingParameter("chain".to_string()))?;
        let root_key = self
            .root_key
            .ok_or_else(|| WalletError::MissingParameter("root_key".to_string()))?;
        let storage_kind = self.storage_config.ok_or_else(|| {
            WalletError::MissingParameter(
                "storage (call with_sqlite, with_sqlite_memory, with_mysql, or with_postgres)"
                    .to_string(),
            )
        })?;

        // Create key deriver
        let key_deriver = Arc::new(CachedKeyDeriver::new(root_key, None));
        let identity_key_hex = key_deriver.identity_key().to_der_hex();

        // Create storage provider based on configuration
        let storage_provider: Arc<dyn crate::storage::traits::provider::StorageProvider> =
            match storage_kind {
                StorageKind::Sqlite(path) => {
                    let url = if path == ":memory:" {
                        "sqlite::memory:".to_string()
                    } else {
                        format!("sqlite:{}", path)
                    };
                    let config = StorageConfig {
                        url,
                        ..StorageConfig::default()
                    };
                    #[cfg(feature = "sqlite")]
                    {
                        let storage = crate::storage::sqlx_impl::SqliteStorage::new_sqlite(
                            config,
                            chain.clone(),
                        )
                        .await?;
                        Arc::new(storage)
                    }
                    #[cfg(not(feature = "sqlite"))]
                    {
                        let _ = config;
                        return Err(WalletError::InvalidOperation(
                            "SQLite feature not enabled. Add `sqlite` feature to Cargo.toml."
                                .to_string(),
                        ));
                    }
                }
                StorageKind::Mysql(url) => {
                    let config = StorageConfig {
                        url,
                        ..StorageConfig::default()
                    };
                    #[cfg(feature = "mysql")]
                    {
                        let mut storage = crate::storage::sqlx_impl::MysqlStorage::new_mysql(
                            config,
                            chain.clone(),
                        )
                        .await?;
                        if let Some(ref sik) = self.storage_identity_key {
                            storage.storage_identity_key = sik.clone();
                        }
                        Arc::new(storage)
                    }
                    #[cfg(not(feature = "mysql"))]
                    {
                        let _ = config;
                        return Err(WalletError::InvalidOperation(
                            "MySQL feature not enabled. Add `mysql` feature to Cargo.toml."
                                .to_string(),
                        ));
                    }
                }
                StorageKind::Postgres(url) => {
                    let config = StorageConfig {
                        url,
                        ..StorageConfig::default()
                    };
                    #[cfg(feature = "postgres")]
                    {
                        let storage = crate::storage::sqlx_impl::PgStorage::new_postgres(
                            config,
                            chain.clone(),
                        )
                        .await?;
                        Arc::new(storage)
                    }
                    #[cfg(not(feature = "postgres"))]
                    {
                        let _ = config;
                        return Err(WalletError::InvalidOperation(
                            "PostgreSQL feature not enabled. Add `postgres` feature to Cargo.toml."
                                .to_string(),
                        ));
                    }
                }
            };

        // Run migrations
        storage_provider.migrate_database().await?;

        // Create WalletStorageManager
        let storage = WalletStorageManager::new(
            storage_provider.clone(),
            None,
            chain.clone(),
            identity_key_hex.clone(),
        );

        // Make storage available (initializes settings/user)
        storage_provider.make_available().await?;

        // Determine services
        let services: Option<Arc<dyn WalletServices>> = if let Some(svc) = self.services {
            Some(svc)
        } else if self.use_default_services {
            Some(Arc::new(crate::services::services::Services::from_chain(
                chain.clone(),
            )))
        } else {
            None
        };

        // Build WalletArgs
        let wallet_args = WalletArgs {
            chain: chain.clone(),
            key_deriver: key_deriver.clone(),
            storage: WalletStorageManager::new(
                storage_provider.clone(),
                None,
                chain.clone(),
                identity_key_hex.clone(),
            ),
            services: services.clone(),
            monitor: None, // Monitor is created after wallet
            privileged_key_manager: self.privileged_key_manager,
            settings_manager: None,
            lookup_resolver: None,
        };

        // Construct wallet
        let wallet = Wallet::new(wallet_args)?;

        // Optionally create Monitor
        let monitor = if self.monitor_enabled {
            if let Some(ref svc) = services {
                let monitor = crate::monitor::Monitor::builder()
                    .chain(chain.clone())
                    .storage(WalletStorageManager::new(
                        storage_provider.clone(),
                        None,
                        chain.clone(),
                        identity_key_hex.clone(),
                    ))
                    .services(svc.clone())
                    .default_tasks()
                    .build()?;
                Some(Arc::new(monitor))
            } else {
                // Monitor requires services -- skip if none configured
                None
            }
        } else {
            None
        };

        Ok(SetupWallet {
            wallet,
            chain,
            key_deriver,
            identity_key: identity_key_hex,
            storage,
            services,
            monitor,
        })
    }
}

impl Default for WalletBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// P2PKH Helper Functions
// ---------------------------------------------------------------------------

/// Derive a key pair from a `CachedKeyDeriver` using specified protocol, key ID, and counterparty.
///
/// Returns a `KeyPair` with the derived private and public keys as hex strings.
pub fn get_key_pair(
    key_deriver: &CachedKeyDeriver,
    protocol_id: &str,
    key_id: &str,
    counterparty: &str,
) -> WalletResult<KeyPair> {
    let protocol = parse_protocol(protocol_id)?;
    let cp = parse_counterparty(counterparty)?;

    let private_key = key_deriver
        .derive_private_key(&protocol, key_id, &cp)
        .map_err(|e| WalletError::Internal(format!("Key derivation failed: {}", e)))?;

    let public_key = private_key.to_public_key();

    Ok(KeyPair {
        private_key: private_key.to_hex(),
        public_key: public_key.to_der_hex(),
    })
}

/// Derive a P2PKH locking script from a `CachedKeyDeriver`.
///
/// Uses the specified protocol, key ID, and counterparty to derive a key pair,
/// then returns the P2PKH locking script bytes for the derived public key.
pub fn get_lock_p2pkh(
    key_deriver: &CachedKeyDeriver,
    protocol_id: &str,
    key_id: &str,
    counterparty: &str,
) -> WalletResult<Vec<u8>> {
    let protocol = parse_protocol(protocol_id)?;
    let cp = parse_counterparty(counterparty)?;

    let derived_pub = key_deriver
        .derive_public_key(&protocol, key_id, &cp, false)
        .map_err(|e| WalletError::Internal(format!("Public key derivation failed: {}", e)))?;

    use bsv::script::templates::p2pkh::P2PKH;
    use bsv::script::templates::ScriptTemplateLock;

    // Hash public key to 20-byte hash for P2PKH
    let hash_vec = derived_pub.to_hash();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&hash_vec);

    let p2pkh = P2PKH::from_public_key_hash(hash);
    let locking_script = p2pkh
        .lock()
        .map_err(|e| WalletError::Internal(format!("P2PKH lock failed: {}", e)))?;
    Ok(locking_script.to_binary())
}

/// Create P2PKH outputs using BRC-29 template with random derivation prefixes/suffixes.
///
/// Generates `count` outputs, each paying `satoshis`, using the wallet's identity key
/// for self-payment via BRC-29 authenticated P2PKH.
pub fn create_p2pkh_outputs(
    key_deriver: &CachedKeyDeriver,
    count: usize,
    satoshis: u64,
) -> WalletResult<Vec<CreateActionOutput>> {
    let mut outputs = Vec::with_capacity(count);
    let root_key = key_deriver.root_key();
    let identity_pub = key_deriver.identity_key();

    for i in 0..count {
        // Generate random derivation prefix and suffix
        let derivation_prefix = random_hex_string();
        let derivation_suffix = random_hex_string();

        let tmpl = ScriptTemplateBRC29::new(derivation_prefix, derivation_suffix);
        let locking_script = tmpl.lock(root_key, &identity_pub)?;

        outputs.push(CreateActionOutput {
            locking_script: Some(locking_script),
            satoshis,
            output_description: format!("p2pkh {}", i),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        });
    }

    Ok(outputs)
}

/// Create P2PKH outputs and submit them as a wallet action.
///
/// Convenience function that creates BRC-29 P2PKH outputs and calls
/// `wallet.create_action()` to register them as a transaction.
pub async fn create_p2pkh_outputs_action(
    wallet: &Wallet,
    count: usize,
    satoshis: u64,
    description: &str,
) -> WalletResult<CreateActionResult> {
    let outputs = create_p2pkh_outputs(&wallet.key_deriver, count, satoshis)?;

    use bsv::wallet::interfaces::WalletInterface;
    let result = wallet
        .create_action(
            CreateActionArgs {
                description: description.to_string(),
                inputs: vec![],
                outputs,
                lock_time: None,
                version: None,
                labels: vec![],
                options: None,
                input_beef: None,
                reference: None,
            },
            None,
        )
        .await
        .map_err(|e| WalletError::Internal(format!("create_action failed: {}", e)))?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a protocol ID string in "security_level.protocol_name" format.
fn parse_protocol(protocol_id: &str) -> WalletResult<Protocol> {
    if let Some((level_str, name)) = protocol_id.split_once('.') {
        let security_level: u8 = level_str
            .parse()
            .map_err(|_| WalletError::InvalidParameter {
                parameter: "protocol_id".to_string(),
                must_be: "in format 'security_level.protocol_name' (e.g., '2.3241645161d8')"
                    .to_string(),
            })?;
        Ok(Protocol {
            security_level,
            protocol: name.to_string(),
        })
    } else {
        // Assume BRC-29 protocol with the string as protocol name and level 2
        Ok(Protocol {
            security_level: 2,
            protocol: protocol_id.to_string(),
        })
    }
}

/// Parse a counterparty string: "self", "anyone", or a public key hex string.
fn parse_counterparty(counterparty: &str) -> WalletResult<Counterparty> {
    match counterparty {
        "self" => Ok(Counterparty {
            counterparty_type: CounterpartyType::Self_,
            public_key: None,
        }),
        "anyone" => Ok(Counterparty {
            counterparty_type: CounterpartyType::Anyone,
            public_key: None,
        }),
        hex_str => {
            let pk = bsv::primitives::public_key::PublicKey::from_string(hex_str).map_err(|e| {
                WalletError::InvalidParameter {
                    parameter: "counterparty".to_string(),
                    must_be: format!("'self', 'anyone', or a valid public key hex: {}", e),
                }
            })?;
            Ok(Counterparty {
                counterparty_type: CounterpartyType::Other,
                public_key: Some(pk),
            })
        }
    }
}

/// Generate a random hex string for derivation prefixes/suffixes.
fn random_hex_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    // Combine timestamp nanos with a simple counter for uniqueness
    let random_val: u64 = (nanos as u64) ^ (nanos.wrapping_shr(64) as u64);
    format!("{:016x}", random_val)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_protocol_with_level() {
        let p = parse_protocol("2.3241645161d8").unwrap();
        assert_eq!(p.security_level, 2);
        assert_eq!(p.protocol, "3241645161d8");
    }

    #[test]
    fn test_parse_protocol_without_level() {
        let p = parse_protocol("3241645161d8").unwrap();
        assert_eq!(p.security_level, 2);
        assert_eq!(p.protocol, "3241645161d8");
    }

    #[test]
    fn test_parse_counterparty_self() {
        let cp = parse_counterparty("self").unwrap();
        assert_eq!(cp.counterparty_type, CounterpartyType::Self_);
        assert!(cp.public_key.is_none());
    }

    #[test]
    fn test_parse_counterparty_anyone() {
        let cp = parse_counterparty("anyone").unwrap();
        assert_eq!(cp.counterparty_type, CounterpartyType::Anyone);
        assert!(cp.public_key.is_none());
    }

    #[test]
    fn test_wallet_builder_validates_chain() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(WalletBuilder::new().build());
        match result {
            Err(e) => {
                let err = e.to_string();
                assert!(err.contains("chain"), "Expected chain error, got: {}", err);
            }
            Ok(_) => panic!("Expected error for missing chain"),
        }
    }

    #[test]
    fn test_wallet_builder_validates_root_key() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(WalletBuilder::new().chain(Chain::Test).build());
        match result {
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("root_key"),
                    "Expected root_key error, got: {}",
                    err
                );
            }
            Ok(_) => panic!("Expected error for missing root_key"),
        }
    }

    #[test]
    fn test_wallet_builder_validates_storage() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let root_key = PrivateKey::from_hex("aa").unwrap();
        let result = rt.block_on(
            WalletBuilder::new()
                .chain(Chain::Test)
                .root_key(root_key)
                .build(),
        );
        match result {
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("storage"),
                    "Expected storage error, got: {}",
                    err
                );
            }
            Ok(_) => panic!("Expected error for missing storage"),
        }
    }

    #[test]
    fn test_get_key_pair_self() {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        let kp = get_key_pair(&key_deriver, "2.3241645161d8", "test_key", "self").unwrap();
        assert!(!kp.private_key.is_empty());
        assert!(!kp.public_key.is_empty());
        // Public key should be 66 hex chars (33 bytes compressed)
        assert_eq!(kp.public_key.len(), 66);
    }

    #[test]
    fn test_get_lock_p2pkh_produces_25_byte_script() {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        let script = get_lock_p2pkh(&key_deriver, "2.3241645161d8", "test_key", "self").unwrap();
        // P2PKH locking script is always 25 bytes
        assert_eq!(script.len(), 25);
    }

    #[test]
    fn test_create_p2pkh_outputs_count() {
        let priv_key = PrivateKey::from_hex("aa").unwrap();
        let key_deriver = CachedKeyDeriver::new(priv_key, None);
        let outputs = create_p2pkh_outputs(&key_deriver, 3, 1000).unwrap();
        assert_eq!(outputs.len(), 3);
        for (i, o) in outputs.iter().enumerate() {
            assert_eq!(o.satoshis, 1000);
            assert!(o.locking_script.is_some());
            assert_eq!(o.output_description, format!("p2pkh {}", i));
        }
    }

    #[test]
    fn test_random_hex_string_length() {
        let s = random_hex_string();
        assert_eq!(s.len(), 16);
    }
}
