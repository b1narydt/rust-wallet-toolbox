//! WalletServices trait and provider traits for the services layer.
//!
//! Ported from wallet-toolbox/src/sdk/WalletServices.interfaces.ts
//! and wallet-toolbox/src/services/Services.ts.

use async_trait::async_trait;
use bsv::transaction::chain_tracker::ChainTracker;
use bsv::transaction::Beef;

use crate::error::WalletResult;
use crate::types::Chain;

use super::types::{
    BlockHeader, BsvExchangeRate, FiatExchangeRates, GetMerklePathResult, GetRawTxResult,
    GetScriptHashHistoryResult, GetStatusForTxidsResult, GetUtxoStatusOutputFormat,
    GetUtxoStatusResult, NLockTimeInput, PostBeefResult, ServicesCallHistory,
};

// ---------------------------------------------------------------------------
// WalletServices Trait
// ---------------------------------------------------------------------------

/// Main trait for BSV wallet network services.
///
/// Consumers use `Arc<dyn WalletServices>` for dynamic dispatch.
/// All methods take `&self` -- implementations use interior mutability
/// (e.g., `tokio::sync::Mutex`) for mutable state like ServiceCollection indexes.
#[async_trait]
pub trait WalletServices: Send + Sync {
    /// The chain being serviced.
    fn chain(&self) -> Chain;

    /// Returns a standard ChainTracker service.
    async fn get_chain_tracker(&self) -> WalletResult<Box<dyn ChainTracker>>;

    /// Attempts to obtain the merkle proof for a transaction.
    async fn get_merkle_path(&self, txid: &str, use_next: bool) -> GetMerklePathResult;

    /// Attempts to obtain the raw transaction bytes for a txid.
    async fn get_raw_tx(&self, txid: &str, use_next: bool) -> GetRawTxResult;

    /// Broadcasts a BEEF transaction to configured services.
    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> Vec<PostBeefResult>;

    /// Determines the UTXO status of a transaction output.
    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
        use_next: bool,
    ) -> GetUtxoStatusResult;

    /// Returns the status (mined/known/unknown) for an array of txids.
    async fn get_status_for_txids(
        &self,
        txids: &[String],
        use_next: bool,
    ) -> GetStatusForTxidsResult;

    /// Returns transaction history for a script hash.
    async fn get_script_hash_history(
        &self,
        hash: &str,
        use_next: bool,
    ) -> GetScriptHashHistoryResult;

    /// Returns a block header given a block hash.
    async fn hash_to_header(&self, hash: &str) -> WalletResult<BlockHeader>;

    /// Returns the serialized block header bytes for a given height.
    async fn get_header_for_height(&self, height: u32) -> WalletResult<Vec<u8>>;

    /// Returns the current chain tip height.
    async fn get_height(&self) -> WalletResult<u32>;

    /// Checks whether the locktime value allows the transaction to be mined
    /// at the current chain height.
    async fn n_lock_time_is_final(&self, input: NLockTimeInput) -> WalletResult<bool>;

    /// Returns the approximate BSV/USD exchange rate.
    async fn get_bsv_exchange_rate(&self) -> WalletResult<BsvExchangeRate>;

    /// Returns the approximate exchange rate for a fiat currency pair.
    async fn get_fiat_exchange_rate(&self, currency: &str, base: Option<&str>)
        -> WalletResult<f64>;

    /// Returns fiat exchange rates for the given target currencies.
    async fn get_fiat_exchange_rates(
        &self,
        target_currencies: &[String],
    ) -> WalletResult<FiatExchangeRates>;

    /// Returns a history of service calls made to the configured services.
    fn get_services_call_history(&self, reset: bool) -> ServicesCallHistory;

    /// Constructs a Beef for the given txid using only external data retrieval.
    async fn get_beef_for_txid(&self, txid: &str) -> WalletResult<Beef>;

    /// Hashes an output script: SHA-256 then reverse to big-endian hex.
    fn hash_output_script(&self, script: &[u8]) -> String;

    /// Checks if an output is currently an unspent UTXO.
    async fn is_utxo(&self, locking_script: &[u8], txid: &str, vout: u32) -> WalletResult<bool>;
}

// ---------------------------------------------------------------------------
// Provider Traits
// ---------------------------------------------------------------------------

/// Provider trait for obtaining merkle paths (proofs).
#[async_trait]
pub trait GetMerklePathProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to get a merkle path for a transaction.
    async fn get_merkle_path(
        &self,
        txid: &str,
        services: &dyn WalletServices,
    ) -> GetMerklePathResult;
}

/// Provider trait for obtaining raw transaction bytes.
#[async_trait]
pub trait GetRawTxProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to get raw transaction bytes for a txid.
    async fn get_raw_tx(&self, txid: &str) -> GetRawTxResult;
}

/// Provider trait for broadcasting BEEF transactions.
#[async_trait]
pub trait PostBeefProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to broadcast a BEEF transaction.
    async fn post_beef(&self, beef: &[u8], txids: &[String]) -> PostBeefResult;
}

/// Provider trait for checking UTXO status.
#[async_trait]
pub trait GetUtxoStatusProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to determine the UTXO status of a transaction output.
    async fn get_utxo_status(
        &self,
        output: &str,
        output_format: Option<GetUtxoStatusOutputFormat>,
        outpoint: Option<&str>,
    ) -> GetUtxoStatusResult;
}

/// Provider trait for checking transaction status by txid.
#[async_trait]
pub trait GetStatusForTxidsProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to get the status for a batch of txids.
    async fn get_status_for_txids(&self, txids: &[String]) -> GetStatusForTxidsResult;
}

/// Provider trait for querying script hash transaction history.
#[async_trait]
pub trait GetScriptHashHistoryProvider: Send + Sync {
    /// Provider name for call history tracking.
    fn name(&self) -> &str;

    /// Attempt to get the transaction history for a script hash.
    async fn get_script_hash_history(&self, hash: &str) -> GetScriptHashHistoryResult;
}
