//! Structured logging initialization for the BSV Wallet Toolbox.
//!
//! Uses the `tracing` crate for structured diagnostic instrumentation.
//! Configure log levels via the `RUST_LOG` environment variable.
//!
//! # Examples
//!
//! ```text
//! RUST_LOG=debug                          # debug-level output
//! RUST_LOG=bsv_wallet_toolbox=trace       # crate-specific trace output
//! # Default level is "info" if RUST_LOG is not set
//! ```

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize structured logging for the wallet toolbox.
///
/// Sets up a tracing subscriber with:
/// - `EnvFilter` respecting the `RUST_LOG` environment variable (default: "info")
/// - Formatted output layer with target names enabled
///
/// This function should be called once at application startup. Calling it
/// multiple times will result in a warning from tracing-subscriber but
/// will not panic.
pub fn init_logging() {
    let _ = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().with_target(true).with_thread_ids(false))
        .try_init();
}
