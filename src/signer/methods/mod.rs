//! Signer method implementations.
//!
//! Each method handles one of the four signer pipeline operations:
//! create_action, sign_action, internalize_action, abort_action.
//!
//! Provider-based variants (e.g., `create_action_provider`) use the
//! [`SigningProvider`](crate::signer::SigningProvider) trait for async
//! signing backends.

pub mod abort_action;
pub mod create_action;
pub mod create_action_provider;
pub mod internalize_action;
pub mod sign_action;
