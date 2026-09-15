//! Backend-neutral compiled-provider, SDK-component, and package-feature resolution.

pub use incan_frontend::provider::*;

pub mod effect_digest;
pub mod inventory;
pub mod requirements;
pub mod sdk_build;
pub mod sdk_store;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod vocab_extraction;

/// Internal marker set only while the toolchain bootstraps one official SDK provider from Incan source.
pub(crate) const SDK_PROVIDER_BUILD_ENV: &str = "INCAN_INTERNAL_SDK_PROVIDER_BUILD";
