//! Concrete service providers for BSV network communication.
//!
//! WhatsOnChain and Bitails are HTTP API providers that implement
//! the provider traits defined in `crate::services::traits`.
//! ARC, SSE client, and exchange rate providers complete the provider ecosystem.

pub mod arc;
pub mod arc_sse_client;
pub mod bitails;
pub mod exchange_rates;
pub mod whats_on_chain;

// Re-exports
pub use arc::ArcProvider;
pub use arc_sse_client::ArcSseClient;
pub use bitails::Bitails;
pub use exchange_rates::{
    fetch_bsv_exchange_rate, fetch_fiat_exchange_rate, fetch_fiat_exchange_rates,
};
pub use whats_on_chain::WhatsOnChain;
