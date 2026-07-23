//! Canonical fixed-arity callable trait vocabulary.
//!
//! `std.traits.callable` owns the source contracts. This registry lets semantic checking and backend lowering agree on
//! which generic bound describes a closure-compatible callable without scattering trait-name or arity checks.

/// Stable identifier for a source callable trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallableTraitId {
    Callable0,
    Callable1,
    Callable2,
}

/// Canonical metadata for one fixed-arity callable trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallableTraitInfo {
    /// Stable identifier for the callable trait.
    pub id: CallableTraitId,
    /// Canonical source declaration name.
    pub name: &'static str,
    /// Number of input parameters before the final return-type argument.
    pub arity: usize,
}

const CALLABLE_TRAITS: &[CallableTraitInfo] = &[
    CallableTraitInfo {
        id: CallableTraitId::Callable0,
        name: "Callable0",
        arity: 0,
    },
    CallableTraitInfo {
        id: CallableTraitId::Callable1,
        name: "Callable1",
        arity: 1,
    },
    CallableTraitInfo {
        id: CallableTraitId::Callable2,
        name: "Callable2",
        arity: 2,
    },
];

/// Canonical method set implemented by every fixed-arity callable trait.
pub const METHOD_NAMES: &[&str] = &["__call__"];

/// Canonical module path that owns the fixed-arity callable traits.
pub const MODULE_PATH: &[&str] = &["std", "traits", "callable"];

/// Resolve a canonical source spelling to its callable trait identifier.
pub fn from_str(name: &str) -> Option<CallableTraitId> {
    CALLABLE_TRAITS
        .iter()
        .find(|info| info.name == name)
        .map(|info| info.id)
}

/// Return the canonical metadata for a callable trait.
pub fn info_for(id: CallableTraitId) -> &'static CallableTraitInfo {
    match id {
        CallableTraitId::Callable0 => &CALLABLE_TRAITS[0],
        CallableTraitId::Callable1 => &CALLABLE_TRAITS[1],
        CallableTraitId::Callable2 => &CALLABLE_TRAITS[2],
    }
}

/// Return whether a segmented source module path is `std.traits.callable`.
pub fn module_path_matches(module_path: &[String]) -> bool {
    module_path.len() == MODULE_PATH.len()
        && module_path
            .iter()
            .map(String::as_str)
            .zip(MODULE_PATH.iter().copied())
            .all(|(actual, expected)| actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_preserves_callable_arity() {
        assert_eq!(info_for(CallableTraitId::Callable0).arity, 0);
        assert_eq!(info_for(CallableTraitId::Callable1).arity, 1);
        assert_eq!(info_for(CallableTraitId::Callable2).arity, 2);
        assert_eq!(from_str("Callable1"), Some(CallableTraitId::Callable1));
        assert_eq!(from_str("Callable3"), None);
        assert_eq!(METHOD_NAMES, ["__call__"]);
    }
}
