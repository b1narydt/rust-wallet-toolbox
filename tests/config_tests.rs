//! Tests for StorageConfig default values.

use std::time::Duration;

use bsv_wallet_toolbox::storage::StorageConfig;

/// Test 2: StorageConfig::default() has correct values.
#[test]
fn storage_config_defaults() {
    let config = StorageConfig::default();

    assert_eq!(config.sqlite_read_connections, 4);
    assert_eq!(config.min_connections, 2);
    assert_eq!(config.max_connections, 50);
    assert_eq!(config.idle_timeout, Duration::from_secs(600)); // 10 minutes
    assert_eq!(config.connect_timeout, Duration::from_secs(5));
}
