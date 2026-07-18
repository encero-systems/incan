//! Backend-neutral compiled-provider, SDK-component, and package-feature resolution.

mod features;
mod plan;
mod sdk;

pub use features::*;
pub use plan::*;
pub use sdk::*;

/// Internal marker set only while the toolchain bootstraps one official SDK provider from Incan source.
pub(crate) const SDK_PROVIDER_BUILD_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_BUILD";
