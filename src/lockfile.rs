//! `oven.lock` for the callers that still spell `crate::lockfile::…`: the lock model is `oven_model::lock` and the
//! semantic-state computation is `provider::lock_semantics`; both are re-exported here under their old names.

pub use oven_model::lock::*;

pub(crate) use crate::provider::lock_semantics::provider_semantic_identities;
pub use crate::provider::lock_semantics::semantic_lock_state;
