//! Explicit ownership-mode vocabulary. Semantic legality belongs to the checker; these names are not runtime wrappers.

/// Stable identity of one prelude access contract from RFC 164.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OwnershipMode {
    /// Shared access to existing storage without transferring its owner.
    Borrowed,
    /// Exclusive borrowed access, with writes controlled separately by `mut`.
    BorrowedMut,
    /// Transfer ownership, retaining any dependencies already held by the value.
    Owned,
    /// Exclusive borrowed result used to continue operations on the original place.
    Reference,
}

/// Canonical prelude declarations. Ordinary source lookup may shadow these identities.
pub const OWNERSHIP_MODES: &[OwnershipMode] = &[
    OwnershipMode::Borrowed,
    OwnershipMode::BorrowedMut,
    OwnershipMode::Owned,
    OwnershipMode::Reference,
];

impl OwnershipMode {
    /// Return the canonical spelling registered in the prelude.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Borrowed => "Borrowed",
            Self::BorrowedMut => "BorrowedMut",
            Self::Owned => "Owned",
            Self::Reference => "Reference",
        }
    }
}

/// Resolve a registered spelling; callers must still prove that lookup selected the builtin declaration.
pub fn from_str(name: &str) -> Option<OwnershipMode> {
    OWNERSHIP_MODES
        .iter()
        .copied()
        .find(|mode| super::spelling_matches(mode.as_str(), name))
}
