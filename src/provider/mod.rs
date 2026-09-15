//! Backend-neutral compiled-provider, SDK-component, and package-feature resolution.

pub use incan_frontend::provider::*;

pub mod effect_digest;
pub mod inventory;
pub mod lock_semantics;
pub mod requirements;
pub mod sdk_build;
pub mod sdk_store;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod vocab_extraction;
