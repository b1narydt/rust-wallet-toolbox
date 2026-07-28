//! Signer method implementations.
//!
//! Each method handles one of the four signer pipeline operations:
//! create_action, sign_action, internalize_action, abort_action.
//!
//! `create_action` and `sign_action` take a
//! [`SigningBackend`](crate::signer::SigningBackend), which selects between
//! local root-key derivation and delegation to a
//! [`SigningProvider`](crate::signer::SigningProvider) — one pipeline with two
//! custody modes, rather than a parallel implementation per backend.

pub mod abort_action;
pub mod create_action;
pub mod internalize_action;
pub mod sign_action;
