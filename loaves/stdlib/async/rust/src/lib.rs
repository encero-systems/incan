//! Racing and runtime entry adapters: the Rust facet of the higher-level `async` standard library component.
//!
//! Foundational task, timer, channel and synchronization adapters live in `incan_std_primitives`. This facet preserves
//! their established module paths through identity-preserving re-exports and owns only the higher-level race and
//! runtime entry mechanisms.

#![deny(clippy::unwrap_used)]

pub mod race;
pub mod runtime;
pub use incan_std_primitives::{channel, sync, task, time};

/// Internal re-exports used by compiler-generated code and the `std.async` sources.
///
/// These are **not** part of the user-facing stdlib API and may change alongside the compiler (toolchain-locked).
pub mod __private {
    pub use tokio;
}
