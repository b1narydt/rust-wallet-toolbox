//! Utility functions for the BSV wallet toolbox.
//!
//! Contains BRC-29 script template, transaction size estimation,
//! and commission key offset derivation.

/// Commission key offset derivation via EC point math.
pub mod offset_key;
/// BRC-29 authenticated P2PKH script template using Type-42 derivation.
pub mod script_template_brc29;
/// Transaction binary size estimation for fee calculation.
pub mod tx_size;

/// Derive an offset public key for a commission output.
pub use offset_key::generate_key_offset;
/// Generate a random key offset for commission outputs.
pub use offset_key::offset_pub_key;
/// Returns the BRC-29 Protocol struct.
pub use script_template_brc29::brc29_protocol;
/// BRC-29 script template for authenticated P2PKH payments.
pub use script_template_brc29::ScriptTemplateBRC29;
/// BRC-29 protocol identifier tuple (security_level, name).
pub use script_template_brc29::BRC29_PROTOCOL_ID;
/// Estimated byte length of a BRC-29 unlock script.
pub use script_template_brc29::BRC29_UNLOCK_LENGTH;
/// Estimated byte length of a serialized transaction input.
pub use tx_size::transaction_input_size;
/// Estimated byte length of a serialized transaction output.
pub use tx_size::transaction_output_size;
/// Compute total serialized transaction size.
pub use tx_size::transaction_size;
/// Byte size of a Bitcoin VarUint encoding.
pub use tx_size::var_uint_size;
