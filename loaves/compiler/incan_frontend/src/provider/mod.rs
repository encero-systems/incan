//! The provider contract the checked program is typed against: the immutable compiled-provider catalog and its
//! active projection, the SDK inventory and its descriptors, and package feature plans. Loading, building and
//! publishing providers is the provider crate's; this half only describes them.

pub mod error;
mod features;
mod plan;
mod sdk;
pub mod stdlib_sources;

pub use features::*;
pub use plan::*;
pub use sdk::*;
pub use stdlib_sources::{StdlibSources, find_stdlib_root, find_stdlib_source_file};

/// Internal marker set only while the toolchain bootstraps one official SDK provider from Incan source.
pub const SDK_PROVIDER_BUILD_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_BUILD";
