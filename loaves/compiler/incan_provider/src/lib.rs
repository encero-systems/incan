//! Incan's SDK providers as the compiler loads them: the active inventory and its discovery, provider requirements
//! and dependency resolution, the SDK store and builder, vocab extraction, the effect digest, and the semantic half of
//! `oven.lock`. The provider *contract* — the plan the typechecker reads — is `incan_frontend::provider` and is
//! re-exported here, so `incan_provider::ProviderPlan` is one name for one type.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
pub use incan_frontend::provider::*;

pub mod compiled_sdk;
pub mod dependency_resolver;
pub mod effect_digest;
pub mod inventory;
pub mod lock_semantics;
pub mod requirements;
pub mod sdk_build;
pub mod sdk_store;
#[cfg(any(test, feature = "test_support"))]
pub mod test_support;
pub mod vocab_extraction;
