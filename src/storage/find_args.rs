//! FindArgs structs for all 16 table types.
//!
//! Each `FindXxxArgs` struct follows the TS `FindPartialSincePagedArgs` pattern:
//! - `partial`: A struct with all-Option fields matching table columns for equality filtering.
//! - `since`: Optional datetime for filtering records updated since a given time.
//! - `paged`: Optional pagination with limit and offset.
//!
//! Some args have additional fields matching their TS counterparts
//! (e.g., `status` filter on transactions, `include_basket`/`include_tags` on outputs).

use chrono::NaiveDateTime;

use crate::status::{ProvenTxReqStatus, SyncStatus, TransactionStatus};
use crate::types::{Chain, StorageProvidedBy};

/// Pagination parameters.
#[derive(Debug, Clone)]
pub struct Paged {
    /// Maximum number of records to return.
    pub limit: i64,
    /// Number of records to skip before returning results.
    pub offset: i64,
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// Partial filter for the `users` table.
#[derive(Debug, Default)]
pub struct UserPartial {
    /// Filter by user primary key.
    pub user_id: Option<i64>,
    /// Filter by identity key (hex public key).
    pub identity_key: Option<String>,
    /// Filter by active storage name.
    pub active_storage: Option<String>,
}

/// Arguments for querying users.
#[derive(Debug, Default)]
pub struct FindUsersArgs {
    /// Column equality filters.
    pub partial: UserPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// Certificate
// ---------------------------------------------------------------------------

/// Partial filter for the `certificates` table.
#[derive(Debug, Default)]
pub struct CertificatePartial {
    /// Filter by certificate primary key.
    pub certificate_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by certificate type identifier.
    pub cert_type: Option<String>,
    /// Filter by serial number.
    pub serial_number: Option<String>,
    /// Filter by certifier identity key.
    pub certifier: Option<String>,
    /// Filter by subject identity key.
    pub subject: Option<String>,
    /// Filter by verifier identity key.
    pub verifier: Option<String>,
    /// Filter by revocation outpoint (txid.vout).
    pub revocation_outpoint: Option<String>,
    /// Filter by signature value.
    pub signature: Option<String>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
}

/// Arguments for querying certificates.
#[derive(Debug, Default)]
pub struct FindCertificatesArgs {
    /// Column equality filters.
    pub partial: CertificatePartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// CertificateField
// ---------------------------------------------------------------------------

/// Partial filter for the `certificate_fields` table.
#[derive(Debug, Default)]
pub struct CertificateFieldPartial {
    /// Filter by parent certificate.
    pub certificate_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by field name.
    pub field_name: Option<String>,
    /// Filter by field value.
    pub field_value: Option<String>,
    /// Filter by master key used for encryption.
    pub master_key: Option<String>,
}

/// Arguments for querying certificate fields.
#[derive(Debug, Default)]
pub struct FindCertificateFieldsArgs {
    /// Column equality filters.
    pub partial: CertificateFieldPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// Commission
// ---------------------------------------------------------------------------

/// Partial filter for the `commissions` table.
#[derive(Debug, Default)]
pub struct CommissionPartial {
    /// Filter by commission primary key.
    pub commission_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by parent transaction.
    pub transaction_id: Option<i64>,
    /// Filter by satoshi amount.
    pub satoshis: Option<i64>,
    /// Filter by key offset string.
    pub key_offset: Option<String>,
    /// Filter by redeemed flag.
    pub is_redeemed: Option<bool>,
}

/// Arguments for querying commissions.
#[derive(Debug, Default)]
pub struct FindCommissionsArgs {
    /// Column equality filters.
    pub partial: CommissionPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// MonitorEvent
// ---------------------------------------------------------------------------

/// Partial filter for the `monitor_events` table.
#[derive(Debug, Default)]
pub struct MonitorEventPartial {
    /// Filter by event primary key.
    pub id: Option<i64>,
    /// Filter by event type string.
    pub event: Option<String>,
}

/// Arguments for querying monitor events.
#[derive(Debug, Default)]
pub struct FindMonitorEventsArgs {
    /// Column equality filters.
    pub partial: MonitorEventPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// OutputBasket
// ---------------------------------------------------------------------------

/// Partial filter for the `output_baskets` table.
#[derive(Debug, Default)]
pub struct OutputBasketPartial {
    /// Filter by basket primary key.
    pub basket_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by basket name.
    pub name: Option<String>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
    /// Number of desired UTXOs for change management.
    pub number_of_desired_utxos: Option<i64>,
    /// Minimum desired UTXO value in satoshis for change management.
    pub minimum_desired_utxo_value: Option<i64>,
}

/// Arguments for querying output baskets.
#[derive(Debug, Default)]
pub struct FindOutputBasketsArgs {
    /// Column equality filters.
    pub partial: OutputBasketPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// OutputTag
// ---------------------------------------------------------------------------

/// Partial filter for the `output_tags` table.
#[derive(Debug, Default)]
pub struct OutputTagPartial {
    /// Filter by output tag primary key.
    pub output_tag_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by tag name.
    pub tag: Option<String>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
}

/// Arguments for querying output tags.
#[derive(Debug, Default)]
pub struct FindOutputTagsArgs {
    /// Column equality filters.
    pub partial: OutputTagPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// OutputTagMap
// ---------------------------------------------------------------------------

/// Partial filter for the `output_tag_maps` join table.
#[derive(Debug, Default)]
pub struct OutputTagMapPartial {
    /// Filter by tag foreign key.
    pub output_tag_id: Option<i64>,
    /// Filter by output foreign key.
    pub output_id: Option<i64>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
}

/// Arguments for querying output tag mappings.
#[derive(Debug, Default)]
pub struct FindOutputTagMapsArgs {
    /// Column equality filters.
    pub partial: OutputTagMapPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Partial filter for the `outputs` table.
#[derive(Debug, Default)]
pub struct OutputPartial {
    /// Filter by output primary key.
    pub output_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by parent transaction.
    pub transaction_id: Option<i64>,
    /// Filter by output basket.
    pub basket_id: Option<i64>,
    /// Filter by spendable flag.
    pub spendable: Option<bool>,
    /// Filter by change output flag.
    pub change: Option<bool>,
    /// Filter by output index within the transaction.
    pub vout: Option<i32>,
    /// Filter by satoshi amount.
    pub satoshis: Option<i64>,
    /// Filter by who provided the storage.
    pub provided_by: Option<StorageProvidedBy>,
    /// Filter by purpose string.
    pub purpose: Option<String>,
    /// Filter by output type descriptor.
    pub output_type: Option<String>,
    /// Filter by transaction ID hex.
    pub txid: Option<String>,
    /// Filter by sender identity key.
    pub sender_identity_key: Option<String>,
    /// Filter by the transaction ID that spent this output.
    pub spent_by: Option<i64>,
}

/// Arguments for querying outputs.
#[derive(Debug, Default)]
pub struct FindOutputsArgs {
    /// Column equality filters.
    pub partial: OutputPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
    /// Filter by transaction status (joined through the transactions table).
    pub tx_status: Option<Vec<TransactionStatus>>,
    /// If true, do not return the locking_script field (performance optimization).
    pub no_script: bool,
    /// If true, include the basket details for each output.
    pub include_basket: bool,
    /// If true, include the tags for each output.
    pub include_tags: bool,
}

// ---------------------------------------------------------------------------
// ProvenTx
// ---------------------------------------------------------------------------

/// Partial filter for the `proven_txs` table.
#[derive(Debug, Default)]
pub struct ProvenTxPartial {
    /// Filter by proven tx primary key.
    pub proven_tx_id: Option<i64>,
    /// Filter by transaction ID hex.
    pub txid: Option<String>,
    /// Filter by block height.
    pub height: Option<i32>,
    /// Filter by block hash hex.
    pub block_hash: Option<String>,
}

/// Arguments for querying proven transactions.
#[derive(Debug, Default)]
pub struct FindProvenTxsArgs {
    /// Column equality filters.
    pub partial: ProvenTxPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// ProvenTxReq
// ---------------------------------------------------------------------------

/// Partial filter for the `proven_tx_reqs` table.
#[derive(Debug, Default)]
pub struct ProvenTxReqPartial {
    /// Filter by proven tx req primary key.
    pub proven_tx_req_id: Option<i64>,
    /// Filter by associated proven tx.
    pub proven_tx_id: Option<i64>,
    /// Filter by request status.
    pub status: Option<ProvenTxReqStatus>,
    /// Filter by transaction ID hex.
    pub txid: Option<String>,
    /// Filter by batch identifier.
    pub batch: Option<String>,
    /// Filter by notification flag.
    pub notified: Option<bool>,
}

/// Arguments for querying proven transaction requests.
#[derive(Debug, Default)]
pub struct FindProvenTxReqsArgs {
    /// Column equality filters.
    pub partial: ProvenTxReqPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
    /// Multi-status filter: if set and non-empty, generates `status IN (...)` clause.
    /// Takes precedence over `partial.status` if both are set.
    pub statuses: Option<Vec<ProvenTxReqStatus>>,
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Partial filter for the `settings` table.
#[derive(Debug, Default)]
pub struct SettingsPartial {
    /// Filter by storage identity key.
    pub storage_identity_key: Option<String>,
    /// Filter by storage name.
    pub storage_name: Option<String>,
    /// Filter by BSV chain type.
    pub chain: Option<Chain>,
    /// Wallet settings JSON blob to write (used by update_settings).
    /// When set to Some("null") during update, clears the stored settings.
    pub wallet_settings_json: Option<String>,
}

/// Arguments for querying settings.
#[derive(Debug, Default)]
pub struct FindSettingsArgs {
    /// Column equality filters.
    pub partial: SettingsPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// SyncState
// ---------------------------------------------------------------------------

/// Partial filter for the `sync_states` table.
#[derive(Debug, Default)]
pub struct SyncStatePartial {
    /// Filter by sync state primary key.
    pub sync_state_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by storage identity key.
    pub storage_identity_key: Option<String>,
    /// Filter by storage name.
    pub storage_name: Option<String>,
    /// Filter by sync status.
    pub status: Option<SyncStatus>,
    /// Filter by initialization flag.
    pub init: Option<bool>,
    /// Updated sync map JSON (used by update_sync_state).
    pub sync_map: Option<String>,
}

/// Arguments for querying sync states.
#[derive(Debug, Default)]
pub struct FindSyncStatesArgs {
    /// Column equality filters.
    pub partial: SyncStatePartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

/// Partial filter for the `transactions` table.
#[derive(Debug, Default)]
pub struct TransactionPartial {
    /// Filter by transaction primary key.
    pub transaction_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by associated proven tx.
    pub proven_tx_id: Option<i64>,
    /// Filter by transaction status.
    pub status: Option<TransactionStatus>,
    /// Filter by reference string.
    pub reference: Option<String>,
    /// Filter by outgoing direction flag.
    pub is_outgoing: Option<bool>,
    /// Filter by transaction ID hex.
    pub txid: Option<String>,
    /// Update raw transaction bytes.
    pub raw_tx: Option<Vec<u8>>,
}

/// Arguments for querying transactions.
#[derive(Debug, Default)]
pub struct FindTransactionsArgs {
    /// Column equality filters.
    pub partial: TransactionPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
    /// Filter by multiple statuses.
    pub status: Option<Vec<TransactionStatus>>,
    /// If true, do not return raw_tx and input_beef fields.
    pub no_raw_tx: bool,
}

// ---------------------------------------------------------------------------
// TxLabel
// ---------------------------------------------------------------------------

/// Partial filter for the `tx_labels` table.
#[derive(Debug, Default)]
pub struct TxLabelPartial {
    /// Filter by tx label primary key.
    pub tx_label_id: Option<i64>,
    /// Filter by owning user.
    pub user_id: Option<i64>,
    /// Filter by label text.
    pub label: Option<String>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
}

/// Arguments for querying transaction labels.
#[derive(Debug, Default)]
pub struct FindTxLabelsArgs {
    /// Column equality filters.
    pub partial: TxLabelPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// TxLabelMap
// ---------------------------------------------------------------------------

/// Partial filter for the `tx_label_maps` join table.
#[derive(Debug, Default)]
pub struct TxLabelMapPartial {
    /// Filter by label foreign key.
    pub tx_label_id: Option<i64>,
    /// Filter by transaction foreign key.
    pub transaction_id: Option<i64>,
    /// Filter by soft-delete flag.
    pub is_deleted: Option<bool>,
}

/// Arguments for querying transaction label mappings.
#[derive(Debug, Default)]
pub struct FindTxLabelMapsArgs {
    /// Column equality filters.
    pub partial: TxLabelMapPartial,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}

// ---------------------------------------------------------------------------
// PurgeParams (used by Monitor purge operations)
// ---------------------------------------------------------------------------

/// Parameters controlling data purge operations.
///
/// Used by the Monitor's TaskPurge to clean up old records.
#[derive(Debug, Clone)]
pub struct PurgeParams {
    /// Whether to purge spent transaction records.
    pub purge_spent: bool,
    /// Whether to purge completed transaction records.
    pub purge_completed: bool,
    /// Whether to purge failed transaction records.
    pub purge_failed: bool,
    /// Age threshold (in msecs) for purging spent records.
    pub purge_spent_age: u64,
    /// Age threshold (in msecs) for purging completed records.
    pub purge_completed_age: u64,
    /// Age threshold (in msecs) for purging failed records.
    pub purge_failed_age: u64,
}

impl Default for PurgeParams {
    fn default() -> Self {
        Self {
            purge_spent: false,
            purge_completed: false,
            purge_failed: true,
            purge_spent_age: 2 * 604_800_000, // 2 weeks
            purge_completed_age: 2 * 604_800_000,
            purge_failed_age: 5 * 86_400_000, // 5 days
        }
    }
}

// ---------------------------------------------------------------------------
// ForUser (used by getProvenTxsForUser, etc.)
// ---------------------------------------------------------------------------

/// Arguments for querying records belonging to a specific user with pagination.
#[derive(Debug)]
pub struct FindForUserSincePagedArgs {
    /// The user to query records for.
    pub user_id: i64,
    /// Only return records updated after this timestamp.
    pub since: Option<NaiveDateTime>,
    /// Pagination (limit/offset).
    pub paged: Option<Paged>,
}
