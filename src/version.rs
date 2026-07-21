//! Incan compiler version information.
//!
//! This module exposes the compiler version as a single constant so all subsystems
//! (CLI, codegen headers, project generator) agree on the same value.
//!
//! ## Notes
//!
//! - The value is taken from Cargo metadata (`CARGO_PKG_VERSION`) at compile time.
//! - Prefer this constant over repeating `env!("CARGO_PKG_VERSION")` in multiple places.

/// The Incan compiler version string (for example, `0.1.0`).
pub const INCAN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Compatibility revision for generated SDK provider Rust.
///
/// This is deliberately independent of the public compiler version: an installed SDK
/// seed is immutable and may outlive a version-neutral compiler code-generation fix.
/// Increase it whenever a change can require every SDK provider to be regenerated.
pub const SDK_PROVIDER_CODEGEN_REVISION: u32 = 4;
