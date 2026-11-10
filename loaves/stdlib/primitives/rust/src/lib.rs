//! Native foundations required by the Incan-authored primitive standard package.

#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod channel;
pub mod collections;
pub mod errors;
pub mod file;
pub mod frozen;
pub mod iter;
pub mod locking;
pub mod mapping;
pub mod num;
pub mod reflection;
pub mod strings;
pub mod sync;
pub mod task;
pub mod time;
pub mod validation;

pub use reflection::{HasClassName, HasFieldInfo, HasFieldValueReflection, HasTypeClassName};

/// Internal re-exports used by compiler-generated code and primitive async sources.
///
/// These are toolchain-locked implementation details rather than user-facing standard-library API.
pub mod __private {
    pub use tokio;
}
