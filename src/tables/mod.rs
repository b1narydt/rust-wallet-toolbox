//! Database table structs for the BSV wallet.
//!
//! Each struct maps 1:1 to a TypeScript table interface from `wallet-toolbox/src/storage/schema/tables/`.
//! All structs use `#[serde(rename_all = "camelCase")]` and `#[sqlx(rename_all = "camelCase")]`
//! to map snake_case Rust fields to camelCase database column names.

/// Certificate table struct.
pub mod certificate;
/// Certificate field table struct (key-value pairs for certificate data).
pub mod certificate_field;
/// Commission table struct (mining commissions).
pub mod commission;
/// Monitor event table struct (background task events).
pub mod monitor_event;
/// Output table struct (transaction outputs / UTXOs).
pub mod output;
/// Output basket table struct (named groups of outputs).
pub mod output_basket;
/// Output tag table struct (user-defined labels for outputs).
pub mod output_tag;
/// Output-to-tag mapping join table struct.
pub mod output_tag_map;
/// Proven transaction table struct (on-chain proofs).
pub mod proven_tx;
/// Proven transaction request table struct (proof acquisition workflow).
pub mod proven_tx_req;
/// Settings table struct (wallet configuration).
pub mod settings;
/// Sync state table struct (multi-device synchronization).
pub mod sync_state;
/// Transaction table struct.
pub mod transaction;
/// Transaction label table struct.
pub mod tx_label;
/// Transaction-to-label mapping join table struct.
pub mod tx_label_map;
/// User table struct (wallet identity).
pub mod user;

/// Certificate database record.
pub use certificate::Certificate;
/// Certificate field database record.
pub use certificate_field::CertificateField;
/// Commission database record.
pub use commission::Commission;
/// Monitor event database record.
pub use monitor_event::MonitorEvent;
/// Transaction output database record.
pub use output::Output;
/// Output basket database record.
pub use output_basket::OutputBasket;
/// Output tag database record.
pub use output_tag::OutputTag;
/// Output-to-tag mapping database record.
pub use output_tag_map::OutputTagMap;
/// Proven transaction database record.
pub use proven_tx::ProvenTx;
/// Proven transaction request database record.
pub use proven_tx_req::ProvenTxReq;
/// Settings database record.
pub use settings::Settings;
/// Sync state database record.
pub use sync_state::SyncState;
/// Transaction database record.
pub use transaction::Transaction;
/// Transaction label database record.
pub use tx_label::TxLabel;
/// Transaction-to-label mapping database record.
pub use tx_label_map::TxLabelMap;
/// User database record.
pub use user::User;
