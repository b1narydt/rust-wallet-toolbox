//! Core wallet types used by the Wallet struct and its methods.
//!
//! Defines WalletArgs (construction arguments), AuthId (caller identity),
//! KeyPair, StorageIdentity, PendingSignAction re-export, and specOp
//! label/basket constants used for internal wallet operations.

use std::sync::Arc;

use bsv::services::overlay_tools::LookupResolver;
use bsv::wallet::cached_key_deriver::CachedKeyDeriver;
use serde::{Deserialize, Serialize};

use crate::services::traits::WalletServices;
use crate::storage::manager::WalletStorageManager;
use crate::types::Chain;

use super::privileged::PrivilegedKeyManager;
use super::settings::WalletSettingsManager;

// Re-export PendingSignAction from the signer module.
pub use crate::signer::types::PendingSignAction;

// ---------------------------------------------------------------------------
// WalletArgs -- construction arguments for the Wallet struct
// ---------------------------------------------------------------------------

/// Arguments for constructing a new `Wallet` instance.
///
/// Matches the TypeScript `WalletArgs` interface from wallet-toolbox.
pub struct WalletArgs {
    /// Which chain (main, test, etc.) this wallet operates on.
    pub chain: Chain,
    /// Key deriver for deriving child keys from the root private key.
    pub key_deriver: Arc<CachedKeyDeriver>,
    /// Storage manager providing active + optional backup persistence.
    pub storage: WalletStorageManager,
    /// Optional wallet services (broadcasting, chain lookups, etc.).
    pub services: Option<Arc<dyn WalletServices>>,
    /// Chain monitor for background transaction lifecycle management (Phase 6).
    pub monitor: Option<Arc<crate::monitor::Monitor>>,
    /// Optional privileged key manager for sensitive crypto operations.
    pub privileged_key_manager: Option<Arc<dyn PrivilegedKeyManager>>,
    /// Optional settings manager with cached TTL.
    pub settings_manager: Option<WalletSettingsManager>,
    /// Optional overlay lookup resolver for identity certificate discovery.
    pub lookup_resolver: Option<Arc<LookupResolver>>,
}

// ---------------------------------------------------------------------------
// AuthId -- caller identity for authorized operations
// ---------------------------------------------------------------------------

/// Identifies the caller for authorized wallet operations.
///
/// The `identity_key` is a compressed public key hex string representing
/// the caller's identity. The optional `user_id` and `is_active` fields
/// match the TypeScript `AuthId` interface for Phase 3 JSON serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthId {
    /// Compressed public key hex (66 chars) of the caller.
    pub identity_key: String,
    /// Database user_id for the caller, if already resolved.
    /// Maps to TypeScript `AuthId.userId?`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    /// Whether this user has active storage access.
    /// Maps to TypeScript `AuthId.isActive?`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
}

// ---------------------------------------------------------------------------
// KeyPair -- public/private key pair
// ---------------------------------------------------------------------------

/// A simple public/private key pair, represented as hex strings.
#[derive(Debug, Clone)]
pub struct KeyPair {
    /// Private key as hex string.
    pub private_key: String,
    /// Compressed public key as hex string.
    pub public_key: String,
}

// ---------------------------------------------------------------------------
// StorageIdentity -- identity for storage operations
// ---------------------------------------------------------------------------

/// Storage identity information for the wallet.
#[derive(Debug, Clone)]
pub struct StorageIdentity {
    /// The identity key used for storage operations.
    pub storage_identity_key: String,
    /// The name of the storage instance.
    pub storage_name: String,
}

// ---------------------------------------------------------------------------
// specOp constants -- special operation basket/label identifiers
// ---------------------------------------------------------------------------

/// Special operation: throw review actions (label).
/// Used to trigger dummy review actions for testing.
pub const SPEC_OP_THROW_REVIEW_ACTIONS: &str =
    "a496e747fc3ad5fabdd4ae8f91184e71f87539bd3d962aa2548942faaaf0047a";

/// Special operation: wallet balance (basket).
/// Used as the basket name for wallet balance queries.
pub const SPEC_OP_WALLET_BALANCE: &str =
    "893b7646de0e1c9f741bd6e9169b76a8847ae34adef7bef1e6a285371206d2e8";

/// Special operation: invalid change (basket).
/// Used as the basket name for invalid change outputs.
pub const SPEC_OP_INVALID_CHANGE: &str =
    "5a76fd430a311f8bc0553859061710a4475c19fed46e2ff95969aa918e612e57";

/// Special operation: set wallet change params (basket).
/// Used as the basket name for wallet change parameter configuration.
pub const SPEC_OP_SET_WALLET_CHANGE_PARAMS: &str =
    "a4979d28ced8581e9c1c92f1001cc7cb3aabf8ea32e10888ad898f0a509a3929";

/// Special operation: no-send actions (label).
/// Applied as a label to actions that should not be broadcast.
pub const SPEC_OP_NO_SEND_ACTIONS: &str =
    "ac6b20a3bb320adafecd637b25c84b792ad828d3aa510d05dc841481f664277d";

/// Special operation: failed actions (label).
/// Applied as a label to actions that have failed.
pub const SPEC_OP_FAILED_ACTIONS: &str =
    "97d4eb1e49215e3374cc2c1939a7c43a55e95c7427bf2d45ed63e3b4e0c88153";

/// Returns `true` if the given basket name is a specOp basket.
pub fn is_spec_op_basket(basket: &str) -> bool {
    matches!(
        basket,
        SPEC_OP_WALLET_BALANCE | SPEC_OP_INVALID_CHANGE | SPEC_OP_SET_WALLET_CHANGE_PARAMS
    )
}

/// Returns `true` if the given label is a specOp label.
pub fn is_spec_op_label(label: &str) -> bool {
    matches!(label, SPEC_OP_NO_SEND_ACTIONS | SPEC_OP_FAILED_ACTIONS)
}

/// Returns `true` if the given label is a specOp throw label.
pub fn is_spec_op_throw_label(label: &str) -> bool {
    label == SPEC_OP_THROW_REVIEW_ACTIONS
}

// ---------------------------------------------------------------------------
// Convenience method types
// ---------------------------------------------------------------------------

/// Wallet balance result returned by `balance_and_utxos()`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WalletBalance {
    /// Total satoshis across all spendable UTXOs.
    pub total: u64,
    /// Individual UTXO details.
    pub utxos: Vec<UtxoInfo>,
}

/// Individual UTXO information for balance queries.
#[derive(Debug, Clone, Serialize)]
pub struct UtxoInfo {
    /// Satoshi value of this UTXO.
    pub satoshis: u64,
    /// Outpoint string in "txid.vout" format.
    pub outpoint: String,
}

/// Aggregate deployment statistics returned by `admin_stats()`.
///
/// Each metric has `_day`, `_week`, `_month`, and `_total` variants
/// matching the TypeScript `AdminStats` interface.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AdminStatsResult {
    /// Identity key of the requester.
    pub requested_by: String,
    /// Timestamp when stats were generated.
    pub when: String,

    /// Users created in the last day.
    pub users_day: u64,
    /// Users created in the last week.
    pub users_week: u64,
    /// Users created in the last month.
    pub users_month: u64,
    /// Total users.
    pub users_total: u64,

    /// Transactions created in the last day.
    pub transactions_day: u64,
    /// Transactions created in the last week.
    pub transactions_week: u64,
    /// Transactions created in the last month.
    pub transactions_month: u64,
    /// Total transactions.
    pub transactions_total: u64,

    /// Completed transactions in the last day.
    pub tx_completed_day: u64,
    /// Completed transactions in the last week.
    pub tx_completed_week: u64,
    /// Completed transactions in the last month.
    pub tx_completed_month: u64,
    /// Total completed transactions.
    pub tx_completed_total: u64,

    /// Failed transactions in the last day.
    pub tx_failed_day: u64,
    /// Failed transactions in the last week.
    pub tx_failed_week: u64,
    /// Failed transactions in the last month.
    pub tx_failed_month: u64,
    /// Total failed transactions.
    pub tx_failed_total: u64,

    /// Unprocessed transactions in the last day.
    pub tx_unprocessed_day: u64,
    /// Unprocessed transactions in the last week.
    pub tx_unprocessed_week: u64,
    /// Unprocessed transactions in the last month.
    pub tx_unprocessed_month: u64,
    /// Total unprocessed transactions.
    pub tx_unprocessed_total: u64,

    /// Sending transactions in the last day.
    pub tx_sending_day: u64,
    /// Sending transactions in the last week.
    pub tx_sending_week: u64,
    /// Sending transactions in the last month.
    pub tx_sending_month: u64,
    /// Total sending transactions.
    pub tx_sending_total: u64,

    /// Unproven transactions in the last day.
    pub tx_unproven_day: u64,
    /// Unproven transactions in the last week.
    pub tx_unproven_week: u64,
    /// Unproven transactions in the last month.
    pub tx_unproven_month: u64,
    /// Total unproven transactions.
    pub tx_unproven_total: u64,

    /// Unsigned transactions in the last day.
    pub tx_unsigned_day: u64,
    /// Unsigned transactions in the last week.
    pub tx_unsigned_week: u64,
    /// Unsigned transactions in the last month.
    pub tx_unsigned_month: u64,
    /// Total unsigned transactions.
    pub tx_unsigned_total: u64,

    /// No-send transactions in the last day.
    pub tx_nosend_day: u64,
    /// No-send transactions in the last week.
    pub tx_nosend_week: u64,
    /// No-send transactions in the last month.
    pub tx_nosend_month: u64,
    /// Total no-send transactions.
    pub tx_nosend_total: u64,

    /// Non-final transactions in the last day.
    pub tx_nonfinal_day: u64,
    /// Non-final transactions in the last week.
    pub tx_nonfinal_week: u64,
    /// Non-final transactions in the last month.
    pub tx_nonfinal_month: u64,
    /// Total non-final transactions.
    pub tx_nonfinal_total: u64,

    /// Unfailed transactions in the last day.
    pub tx_unfail_day: u64,
    /// Unfailed transactions in the last week.
    pub tx_unfail_week: u64,
    /// Unfailed transactions in the last month.
    pub tx_unfail_month: u64,
    /// Total unfailed transactions.
    pub tx_unfail_total: u64,

    /// Default basket satoshis in the last day.
    pub satoshis_default_day: u64,
    /// Default basket satoshis in the last week.
    pub satoshis_default_week: u64,
    /// Default basket satoshis in the last month.
    pub satoshis_default_month: u64,
    /// Total default basket satoshis.
    pub satoshis_default_total: u64,

    /// Other basket satoshis in the last day.
    pub satoshis_other_day: u64,
    /// Other basket satoshis in the last week.
    pub satoshis_other_week: u64,
    /// Other basket satoshis in the last month.
    pub satoshis_other_month: u64,
    /// Total other basket satoshis.
    pub satoshis_other_total: u64,

    /// Output baskets created in the last day.
    pub baskets_day: u64,
    /// Output baskets created in the last week.
    pub baskets_week: u64,
    /// Output baskets created in the last month.
    pub baskets_month: u64,
    /// Total output baskets.
    pub baskets_total: u64,

    /// Transaction labels created in the last day.
    pub labels_day: u64,
    /// Transaction labels created in the last week.
    pub labels_week: u64,
    /// Transaction labels created in the last month.
    pub labels_month: u64,
    /// Total transaction labels.
    pub labels_total: u64,

    /// Output tags created in the last day.
    pub tags_day: u64,
    /// Output tags created in the last week.
    pub tags_week: u64,
    /// Output tags created in the last month.
    pub tags_month: u64,
    /// Total output tags.
    pub tags_total: u64,
}
