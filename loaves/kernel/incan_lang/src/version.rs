//! Incan compiler version information.
//!
//! This module exposes the compiler version as a single constant so all subsystems (CLI, LSP, codegraph, manifests)
//! agree on the same value.
//!
//! ## Notes
//!
//! - The value is this Loaf's `loaf.toml` version, which Oven passes as `CARGO_PKG_VERSION` at compile time.
//! - Prefer this constant over repeating `env!("CARGO_PKG_VERSION")` in multiple places.

/// The Incan compiler version string (for example, `0.6.0-dev.8`).
pub const INCAN_VERSION: &str = env!("CARGO_PKG_VERSION");
