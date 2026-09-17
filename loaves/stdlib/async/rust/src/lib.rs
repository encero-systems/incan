//! Task, timer, channel and synchronisation runtime: the Rust facet of the `async` standard library component.
//!
//! The `std.async` sources import these modules with `from rust::incan_std_async::…`; generated code links the crate
//! when its program reaches `std.async`, and the compiler writes the checked `std.async` facade into every generated
//! crate today, so in practice every program does. The modules are thin, Python-shaped wrappers over tokio: task
//! spawning and joining, timeouts, racing, channels and the synchronisation primitives.

#![deny(clippy::unwrap_used)]

pub mod channel;
pub mod race;
pub mod runtime;
pub mod sync;
pub mod task;
pub mod time;

/// Internal re-exports used by compiler-generated code and the `std.async` sources.
///
/// These are **not** part of the user-facing stdlib API and may change alongside the compiler (toolchain-locked).
pub mod __private {
    pub use tokio;
}
