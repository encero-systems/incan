//! Compiler-owned Clone method surface vocabulary.

use super::LangItemInfo;
use crate::lang::registry::{RFC, Since, Stability};

/// Stable identifier for the compiler-owned Clone trait method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CloneMethodId {
    /// Clone one value through its selected compiler-owned Clone contract.
    Clone,
}

/// Metadata for one compiler-owned Clone method.
pub type CloneMethodInfo = LangItemInfo<CloneMethodId>;

/// Registry of compiler-owned Clone methods.
pub const CLONE_METHODS: &[CloneMethodInfo] = &[LangItemInfo {
    id: CloneMethodId::Clone,
    canonical: "clone",
    aliases: &[],
    description: "Clone one value through its compiler-owned Clone contract.",
    introduced_in_rfc: RFC::_000,
    since: Since(0, 1),
    stability: Stability::Stable,
    examples: &[],
}];

/// Resolve a Clone method spelling to its stable identity.
pub fn from_str(name: &str) -> Option<CloneMethodId> {
    super::from_str_impl(CLONE_METHODS, name)
}

/// Return the canonical spelling for a Clone method.
pub fn as_str(id: CloneMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a Clone method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: CloneMethodId) -> &'static CloneMethodInfo {
    super::info_for_impl(CLONE_METHODS, id, "Clone method info missing")
}
