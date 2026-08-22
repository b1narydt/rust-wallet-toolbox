//! Setup helpers for ergonomic wallet construction and P2PKH utilities.
//!
//! Provides `WalletBuilder` for fluent wallet construction from minimal configuration,
//! `SetupWallet` as the rich return type exposing all wired components, and P2PKH
//! helper functions for key derivation and output creation.
//!
//! Ported from wallet-toolbox/src/Setup.ts.

use std::sync::Arc;

use bsv::primitives::private_key::PrivateKey;
use bsv::services::overlay_tools::LookupResolver;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use bsv::wallet::interfaces::{CreateActionArgs, CreateActionOutput, CreateActionResult};
use bsv::wallet::types::{Counterparty, CounterpartyType, Protocol};
use bsv::wallet::KeyDeriverApi;

use crate::error::{WalletError, WalletResult};
use crate::monitor::{AsyncResultCallback, Monitor};
use crate::services::traits::WalletServices;
use crate::signer::signing_provider::SigningProvider;
use crate::storage::manager::WalletStorageManager;
use crate::storage::traits::wallet_provider::WalletStorageProvider;
use crate::storage::StorageConfig;
use crate::types::Chain;
use crate::utility::script_template_brc29::ScriptTemplateBRC29;
use crate::wallet::privileged::PrivilegedKeyManager;
use crate::wallet::types::AuthId;
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
    pub key_deriver: Arc<dyn KeyDeriverApi>,
    /// The wallet's identity key as a hex DER public key string.
    pub identity_key: String,
    /// The storage manager (shared Arc -- same instance used by wallet, monitor, and caller).
    pub storage: Arc<WalletStorageManager>,
    /// The services provider, if configured.
    pub services: Option<Arc<dyn WalletServices>>,
    /// The monitor, if enabled.
    pub monitor: Option<Arc<Monitor>>,
}

// ---------------------------------------------------------------------------
// StorageKind -- internal enum for storage configuration
// ---------------------------------------------------------------------------

/// Internal storage configuration variant for the builder.
#[derive(Clone)]
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

/// Connection-pool settings the builder applies to every store it opens.
#[derive(Clone, Copy, Default)]
struct PoolOverrides {
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    idle_timeout: Option<std::time::Duration>,
    connect_timeout: Option<std::time::Duration>,
}

impl PoolOverrides {
    fn apply(&self, config: &mut StorageConfig) {
        if let Some(max) = self.max_connections {
            config.max_connections = max;
        }
        if let Some(min) = self.min_connections {
            config.min_connections = min;
        }
        if let Some(timeout) = self.idle_timeout {
            config.idle_timeout = timeout;
        }
        if let Some(timeout) = self.connect_timeout {
            config.connect_timeout = timeout;
        }
    }
}

/// Open one storage provider and run its migrations.
///
/// Shared by the active store and every backup so a backup is opened exactly
/// the way the active store is — same pool settings, same migrations. A backup
/// built differently from the store it mirrors is a backup that fails when it
/// is finally needed.
async fn open_storage_provider(
    kind: StorageKind,
    chain: &Chain,
    storage_identity_key: Option<&str>,
    pool: PoolOverrides,
) -> WalletResult<Arc<dyn WalletStorageProvider>> {
    let provider: Arc<dyn WalletStorageProvider> = match kind {
        StorageKind::Sqlite(path) => {
            let url = if path == ":memory:" {
                "sqlite::memory:".to_string()
            } else {
                format!("sqlite:{path}")
            };
            let mut config = StorageConfig {
                url,
                ..StorageConfig::default()
            };
            pool.apply(&mut config);
            #[cfg(feature = "sqlite")]
            {
                let storage =
                    crate::storage::sqlx_impl::SqliteStorage::new_sqlite(config, chain.clone())
                        .await?;
                Arc::new(storage) as Arc<dyn WalletStorageProvider>
            }
            #[cfg(not(feature = "sqlite"))]
            {
                let _ = config;
                return Err(WalletError::InvalidOperation(
                    "SQLite feature not enabled. Add `sqlite` feature to Cargo.toml.".to_string(),
                ));
            }
        }
        StorageKind::Mysql(url) => {
            let mut config = StorageConfig {
                url,
                ..StorageConfig::default()
            };
            pool.apply(&mut config);
            #[cfg(feature = "mysql")]
            {
                let mut storage =
                    crate::storage::sqlx_impl::MysqlStorage::new_mysql(config, chain.clone())
                        .await?;
                if let Some(sik) = storage_identity_key {
                    storage.storage_identity_key = sik.to_string();
                }
                Arc::new(storage) as Arc<dyn WalletStorageProvider>
            }
            #[cfg(not(feature = "mysql"))]
            {
                let _ = config;
                let _ = storage_identity_key;
                return Err(WalletError::InvalidOperation(
                    "MySQL feature not enabled. Add `mysql` feature to Cargo.toml.".to_string(),
                ));
            }
        }
        StorageKind::Postgres(url) => {
            let mut config = StorageConfig {
                url,
                ..StorageConfig::default()
            };
            pool.apply(&mut config);
            #[cfg(feature = "postgres")]
            {
                let storage =
                    crate::storage::sqlx_impl::PgStorage::new_postgres(config, chain.clone())
                        .await?;
                Arc::new(storage) as Arc<dyn WalletStorageProvider>
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

    provider.migrate("setup", "").await?;
    Ok(provider)
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
///     .build() // creates AND starts the monitor (default since 0.3.4)
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct WalletBuilder {
    chain: Option<Chain>,
    root_key: Option<PrivateKey>,
    key_deriver: Option<Arc<dyn KeyDeriverApi>>,
    signing_provider: Option<Arc<dyn SigningProvider>>,
    storage_config: Option<StorageKind>,
    backup_configs: Vec<StorageKind>,
    backup_providers: Vec<Arc<dyn WalletStorageProvider>>,
    storage_identity_key: Option<String>,
    services: Option<Arc<dyn WalletServices>>,
    use_default_services: bool,
    monitor_enabled: bool,
    monitor_after_task: Option<AsyncResultCallback<String>>,
    privileged_key_manager: Option<Arc<dyn PrivilegedKeyManager>>,
    lookup_resolver: Option<Arc<LookupResolver>>,
    arcade_url: Option<String>,
    arcade_callback_token: Option<String>,
    pool_max_connections: Option<u32>,
    pool_min_connections: Option<u32>,
    pool_idle_timeout: Option<std::time::Duration>,
    pool_connect_timeout: Option<std::time::Duration>,
}

impl WalletBuilder {
    /// Create a new WalletBuilder with all fields unset.
    pub fn new() -> Self {
        Self {
            chain: None,
            root_key: None,
            key_deriver: None,
            signing_provider: None,
            storage_config: None,
            backup_configs: Vec::new(),
            backup_providers: Vec::new(),
            storage_identity_key: None,
            services: None,
            use_default_services: false,
            // ON by default (0.3.4): the toolbox stores every createAction tx as an
            // `unsent` ProvenTxReq and ONLY the monitor's TaskSendWaiting ever
            // broadcasts it. A wallet built without a monitor silently never posts
            // its transactions to the network (the rust-mpc#147 faucet phantom-spend)
            // — "works out of the box" means the monitor runs unless the caller
            // explicitly opts out with `without_monitor()`.
            monitor_enabled: true,
            monitor_after_task: None,
            privileged_key_manager: None,
            lookup_resolver: None,
            arcade_url: None,
            arcade_callback_token: None,
            pool_max_connections: None,
            pool_min_connections: None,
            pool_idle_timeout: None,
            pool_connect_timeout: None,
        }
    }

    /// Supply the overlay `LookupResolver` used by `discoverByIdentityKey` /
    /// `discoverByAttributes`.
    ///
    /// Optional: a wallet built without one resolves against the default
    /// trackers for its chain, matching TS (`Wallet.ts`:
    /// `args.lookupResolver || new LookupResolver({networkPreset})`). Pass one
    /// to point discovery at a private overlay.
    pub fn lookup_resolver(mut self, resolver: Arc<LookupResolver>) -> Self {
        self.lookup_resolver = Some(resolver);
        self
    }

    /// Enable ARC/Arcade SSE status streaming for the monitor.
    ///
    /// Both halves are required — the base URL of the Arcade instance and the
    /// stable callback token that broadcasts carry as `X-CallbackToken`.
    /// Without them the SSE task stays dormant and transaction status is
    /// discovered by the proof poll instead.
    pub fn arcade_sse(mut self, arcade_url: String, callback_token: String) -> Self {
        self.arcade_url = Some(arcade_url);
        self.arcade_callback_token = Some(callback_token);
        self
    }

    /// Set the chain (required).
    pub fn chain(mut self, chain: Chain) -> Self {
        self.chain = Some(chain);
        self
    }

    /// Set the root private key.
    ///
    /// Required unless a `key_deriver` is supplied instead.
    pub fn root_key(mut self, key: PrivateKey) -> Self {
        self.root_key = Some(key);
        self
    }

    /// Supply the key deriver directly, instead of a root private key.
    ///
    /// This is the entry point for a wallet whose identity key is not backed by
    /// a local root key — a joint or threshold public key, say. Such a deriver
    /// cannot lock or sign anything on its own, so pair it with
    /// [`with_signing_provider`](Self::with_signing_provider).
    ///
    /// Takes precedence over [`root_key`](Self::root_key) if both are set.
    pub fn key_deriver(mut self, key_deriver: Arc<dyn KeyDeriverApi>) -> Self {
        self.key_deriver = Some(key_deriver);
        self
    }

    /// Delegate BRC-29 derivation and input signing to a custody backend.
    ///
    /// With a provider set, `create_action`, `sign_action` and
    /// `internalize_action` never reach for the key deriver's root key. See
    /// [`WalletArgs::signing_provider`] for what the provider does *not* cover.
    pub fn with_signing_provider(mut self, provider: Arc<dyn SigningProvider>) -> Self {
        self.signing_provider = Some(provider);
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

    /// Replicate the active store into a SQLite file at the given path.
    ///
    /// The wallet database is key material, not a cache: BRC-42 output
    /// derivation is not enumerable, so losing `derivation_prefix` /
    /// `derivation_suffix` leaves UTXOs unspendable even with every key. A
    /// backup store is a live replica of that metadata.
    ///
    /// Replication happens at build time and on `set_active` /
    /// `add_wallet_storage_provider`. It is **not** periodic — there is no
    /// background task pushing later writes, so call
    /// [`WalletStorageManager::update_backups`] when you want the replica
    /// brought current.
    ///
    /// May be called more than once to configure several backups.
    pub fn with_backup_sqlite(mut self, path: &str) -> Self {
        self.backup_configs
            .push(StorageKind::Sqlite(path.to_string()));
        self
    }

    /// Replicate the active store into a MySQL database. See
    /// [`Self::with_backup_sqlite`] for replication timing.
    pub fn with_backup_mysql(mut self, url: &str) -> Self {
        self.backup_configs
            .push(StorageKind::Mysql(url.to_string()));
        self
    }

    /// Replicate the active store into a PostgreSQL database. See
    /// [`Self::with_backup_sqlite`] for replication timing.
    pub fn with_backup_postgres(mut self, url: &str) -> Self {
        self.backup_configs
            .push(StorageKind::Postgres(url.to_string()));
        self
    }

    /// Replicate the active store into an already-constructed provider.
    ///
    /// For backups the builder cannot open itself — a remote store, or one
    /// wrapped in custom middleware. The caller owns its migrations; the
    /// builder runs none against it. See [`Self::with_backup_sqlite`] for
    /// replication timing.
    pub fn with_backup_provider(mut self, provider: Arc<dyn WalletStorageProvider>) -> Self {
        self.backup_providers.push(provider);
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
    ///
    /// Since 0.3.4 the monitor is ON by default and `build()` starts it, so this
    /// is a no-op kept for back-compatibility with 0.3.3-era callers.
    pub fn with_monitor(mut self) -> Self {
        self.monitor_enabled = true;
        self
    }

    /// Run a fallible hook after each completed background monitor task.
    ///
    /// An error pauses the monitor and retries before it starts another task.
    /// This is useful when an embedding needs a durable replica or external
    /// acknowledgement to keep pace with background storage changes.
    pub fn with_monitor_after_task(mut self, callback: AsyncResultCallback<String>) -> Self {
        self.monitor_after_task = Some(callback);
        self
    }

    /// Opt OUT of the background monitor.
    ///
    /// Without a monitor, transactions created with the default
    /// `accept_delayed_broadcast` are stored as `unsent` ProvenTxReqs and are
    /// NEVER broadcast — nothing else posts them to the network. Use this only
    /// when something else owns broadcasting (or in offline tests); a wallet
    /// that pays anyone must keep the monitor.
    pub fn without_monitor(mut self) -> Self {
        self.monitor_enabled = false;
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

    /// Set the maximum number of connections in the database pool.
    ///
    /// Default: 50. For Railway replicas, divide your MySQL server's
    /// `max_connections` by the number of replicas.
    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.pool_max_connections = Some(max);
        self
    }

    /// Set the minimum number of connections in the database pool.
    ///
    /// Default: 2.
    pub fn with_min_connections(mut self, min: u32) -> Self {
        self.pool_min_connections = Some(min);
        self
    }

    /// Set the idle timeout for database connections.
    ///
    /// Default: 600 seconds.
    pub fn with_pool_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.pool_idle_timeout = Some(timeout);
        self
    }

    /// Set the connection timeout for the database pool.
    ///
    /// Default: 5 seconds.
    pub fn with_pool_connect_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.pool_connect_timeout = Some(timeout);
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
        // Either an explicit deriver or a root key to build one from.
        let key_deriver: Arc<dyn KeyDeriverApi> = match self.key_deriver {
            Some(kd) => kd,
            None => {
                let root_key = self.root_key.ok_or_else(|| {
                    WalletError::MissingParameter("root_key (or key_deriver)".to_string())
                })?;
                Arc::new(CachedKeyDeriver::new(root_key, None))
            }
        };
        let storage_kind = self.storage_config.ok_or_else(|| {
            WalletError::MissingParameter(
                "storage (call with_sqlite, with_sqlite_memory, with_mysql, or with_postgres)"
                    .to_string(),
            )
        })?;

        let identity_key_hex = key_deriver.identity_key().to_der_hex();

        let pool = PoolOverrides {
            max_connections: self.pool_max_connections,
            min_connections: self.pool_min_connections,
            idle_timeout: self.pool_idle_timeout,
            connect_timeout: self.pool_connect_timeout,
        };

        // Open the active store, then every backup, through one path.
        let provider = open_storage_provider(
            storage_kind,
            &chain,
            self.storage_identity_key.as_deref(),
            pool,
        )
        .await?;

        let mut backups: Vec<Arc<dyn WalletStorageProvider>> = self.backup_providers;
        for kind in self.backup_configs {
            backups.push(open_storage_provider(kind, &chain, None, pool).await?);
        }
        let has_backups = !backups.is_empty();

        // Declare each backup a backup *of this active store* before the
        // manager partitions them.
        //
        // A store's user record names the store it considers active, and a
        // freshly created one names itself. The manager reads that as two
        // stores each claiming to be active — a genuine conflict when both
        // hold data, but wrong for a store the caller has just designated as a
        // backup. Configuring a backup on the builder IS the declaration, so
        // record it rather than leaving the manager to guess.
        if has_backups {
            let active_sik = provider.make_available().await?.storage_identity_key;
            for backup in &backups {
                backup.make_available().await?;
                let (user, _) = backup.find_or_insert_user(&identity_key_hex).await?;
                if user.active_storage != active_sik {
                    let auth = AuthId {
                        identity_key: identity_key_hex.clone(),
                        user_id: Some(user.user_id),
                        is_active: None,
                    };
                    backup.set_active(&auth, &active_sik).await?;
                }
            }
        }

        // Create ONE storage manager and wrap in Arc -- shared by wallet, monitor, and caller.
        let storage = Arc::new(WalletStorageManager::new(
            identity_key_hex.clone(),
            Some(provider.clone()),
            backups,
        ));
        storage.make_available().await?;

        // Replicate the active store into every backup before handing the
        // wallet out. `make_available` only partitions the stores; without
        // this a freshly-configured backup stays empty until the next
        // `set_active` or `add_wallet_storage_provider`, which is a backup
        // that exists but holds nothing. Subsequent boots are incremental —
        // the sync engine resumes from the persisted sync_states row.
        //
        // A backup that cannot be reached is a degraded wallet, not a broken
        // one: the ACTIVE store holds the funds, and refusing to boot because
        // a REPLICA is down makes redundancy a liability — the one configured
        // store that is still healthy becomes unusable. TS does not replicate
        // at boot at all. So this warns loudly and hands the wallet out; the
        // next `update_backups` retries, and the error names the backup.
        if has_backups {
            if let Err(e) = storage.update_backups(None).await {
                tracing::warn!(
                    "boot replication to a configured backup failed: {e}. The wallet is USABLE \
                     — the active store is authoritative and holds the funds — but it is running \
                     UNREPLICATED, so a loss of the active store now loses everything since the \
                     last successful sync. Fix the backup and call update_backups."
                );
            }
        }

        // Determine services
        let services: Option<Arc<dyn WalletServices>> = if let Some(svc) = self.services {
            Some(svc)
        } else if self.use_default_services {
            Some(Arc::new(
                crate::services::services::Services::from_chain_with_arcade(
                    chain.clone(),
                    self.arcade_url.clone(),
                    self.arcade_callback_token.clone(),
                ),
            ))
        } else {
            None
        };

        // Build WalletArgs -- wallet shares the same Arc<WalletStorageManager>.
        let wallet_args = WalletArgs {
            chain: chain.clone(),
            key_deriver: key_deriver.clone(),
            signing_provider: self.signing_provider,
            storage: storage.clone(),
            services: services.clone(),
            monitor: None, // Monitor is created after wallet
            privileged_key_manager: self.privileged_key_manager,
            settings_manager: None,
            lookup_resolver: self.lookup_resolver,
        };

        // Construct wallet
        let wallet = Wallet::new(wallet_args)?;

        // Create AND START the Monitor (default) -- shares the same
        // Arc<WalletStorageManager>. Started here, before the Arc wrap, so the
        // wallet broadcasts/proves out of the box: the caller no longer needs the
        // Arc::get_mut + start_tasks() dance every embedder used to forget (the
        // rust-mpc#147 faucet never broadcast a single tx that way).
        let monitor = if self.monitor_enabled {
            if let Some(ref svc) = services {
                let mut builder = crate::monitor::Monitor::builder()
                    .chain(chain.clone())
                    .storage(storage.clone())
                    .services(svc.clone())
                    .default_tasks();
                if let Some(callback) = self.monitor_after_task {
                    builder = builder.after_task(callback);
                }
                if let Some(url) = self.arcade_url {
                    builder = builder.arcade_url(url);
                }
                if let Some(token) = self.arcade_callback_token {
                    builder = builder.callback_token(token);
                }
                let mut monitor = builder.build()?;
                monitor.start_tasks()?;
                tracing::info!("wallet monitor started (default; opt out with without_monitor())");
                Some(Arc::new(monitor))
            } else {
                // Monitor requires services -- skip if none configured (offline /
                // storage-only wallets have nothing to broadcast WITH anyway).
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

/// Derive a key pair from a key deriver using specified protocol, key ID, and counterparty.
///
/// Returns a `KeyPair` with the derived private and public keys as hex strings.
pub fn get_key_pair(
    key_deriver: &dyn KeyDeriverApi,
    protocol_id: &str,
    key_id: &str,
    counterparty: &str,
) -> WalletResult<KeyPair> {
    let protocol = parse_protocol(protocol_id)?;
    let cp = parse_counterparty(counterparty)?;

    let private_key = key_deriver
        .derive_private_key(&protocol, key_id, &cp)
        .map_err(|e| WalletError::Internal(format!("Key derivation failed: {e}")))?;

    let public_key = private_key.to_public_key();

    Ok(KeyPair {
        private_key: private_key.to_hex(),
        public_key: public_key.to_der_hex(),
    })
}

/// Derive a P2PKH locking script from a key deriver.
///
/// Uses the specified protocol, key ID, and counterparty to derive a key pair,
/// then returns the P2PKH locking script bytes for the derived public key.
pub fn get_lock_p2pkh(
    key_deriver: &dyn KeyDeriverApi,
    protocol_id: &str,
    key_id: &str,
    counterparty: &str,
) -> WalletResult<Vec<u8>> {
    let protocol = parse_protocol(protocol_id)?;
    let cp = parse_counterparty(counterparty)?;

    let derived_pub = key_deriver
        .derive_public_key(&protocol, key_id, &cp, false)
        .map_err(|e| WalletError::Internal(format!("Public key derivation failed: {e}")))?;

    use bsv::script::templates::p2pkh::P2PKH;
    use bsv::script::templates::ScriptTemplateLock;

    // Hash public key to 20-byte hash for P2PKH
    let hash_vec = derived_pub.to_hash();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&hash_vec);

    let p2pkh = P2PKH::from_public_key_hash(hash);
    let locking_script = p2pkh
        .lock()
        .map_err(|e| WalletError::Internal(format!("P2PKH lock failed: {e}")))?;
    Ok(locking_script.to_binary())
}

/// Create P2PKH outputs using BRC-29 template with random derivation prefixes/suffixes.
///
/// Generates `count` outputs, each paying `satoshis`, using the wallet's identity key
/// for self-payment via BRC-29 authenticated P2PKH.
///
/// Locks with `key_deriver.root_key()`, so this is only meaningful for a deriver
/// that holds one. Under delegated custody the provider owns BRC-29 locking and
/// this helper does not apply.
pub fn create_p2pkh_outputs(
    key_deriver: &dyn KeyDeriverApi,
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
            output_description: format!("p2pkh {i}"),
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
    let outputs = create_p2pkh_outputs(wallet.key_deriver.as_ref(), count, satoshis)?;

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
        .map_err(|e| WalletError::Internal(format!("create_action failed: {e}")))?;

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
                    must_be: format!("'self', 'anyone', or a valid public key hex: {e}"),
                }
            })?;
            Ok(Counterparty {
                counterparty_type: CounterpartyType::Other,
                public_key: Some(pk),
            })
        }
    }
}

/// Generate a random base64 string for derivation prefixes/suffixes.
/// BRC-42 requires these to be valid base64 (matching TS `randomBytesBase64(8)`).
fn random_hex_string() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(buf)
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
                assert!(err.contains("chain"), "Expected chain error, got: {err}");
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
                    "Expected root_key error, got: {err}"
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
                    "Expected storage error, got: {err}"
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
            assert_eq!(o.output_description, format!("p2pkh {i}"));
        }
    }

    #[test]
    fn test_random_hex_string_length() {
        let s = random_hex_string();
        // 8 random bytes → base64 = 12 chars
        assert_eq!(s.len(), 12);
        // Must be valid base64
        use base64::Engine;
        assert!(base64::engine::general_purpose::STANDARD.decode(&s).is_ok());
    }
}
