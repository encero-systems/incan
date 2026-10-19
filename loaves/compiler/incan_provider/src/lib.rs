//! Incan's SDK providers as the compiler loads them: the active inventory and its discovery, provider requirements
//! and dependency resolution, the SDK store and builder, vocab extraction, the effect digest, and the semantic half of
//! `oven.lock`. The provider *contract* — the plan the typechecker reads — is `incan_frontend::provider` and is
//! re-exported here, so `incan_provider::ProviderPlan` is one name for one type.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod dependency_resolver;
pub mod inventory;
pub mod lock_semantics;
pub mod requirements;
