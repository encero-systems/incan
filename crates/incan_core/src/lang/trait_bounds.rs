//! RFC 023: Incan-to-Rust trait bound mapping.
//!
//! This registry maps Incan trait names used in `with` clauses (e.g., `[T with Eq]`) to their corresponding Rust trait
//! paths. The distinction matters because some Incan names differ from their Rust equivalents (e.g., Incan `Eq` maps to
//! Rust `PartialEq`).
//!
//! ## Notes
//! - Lookup via [`incan_to_rust`] is **case-sensitive**.
//! - This registry only covers traits used as *bounds* on type parameters; the full trait vocabulary is in
//!   [`crate::lang::traits`].
//! - Unknown names are passed through as-is during lowering (allowing user-defined trait bounds).

use super::registry::{RFC, RfcId, Since, Stability};

/// Stable identifier for an Incan trait bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitBoundId {
    Eq,
    Ord,
    Hash,
    Clone,
    Debug,
    Display,
    Serialize,
    Deserialize,
}

/// Metadata for a trait bound mapping entry.
#[derive(Debug, Clone, Copy)]
pub struct TraitBoundMapping {
    pub id: TraitBoundId,
    /// Incan-side name (as written in `with` clauses).
    pub incan_name: &'static str,
    /// Rust-side trait path (as emitted in generated Rust code).
    pub rust_path: &'static str,
    pub description: &'static str,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
}

/// Registry of all known Incan → Rust trait bound mappings.
pub const TRAIT_BOUNDS: &[TraitBoundMapping] = &[
    mapping(
        TraitBoundId::Eq,
        "Eq",
        "PartialEq",
        "Equality comparison — Incan `Eq` maps to Rust `PartialEq`.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Ord,
        "Ord",
        "PartialOrd",
        "Ordering comparison — Incan `Ord` maps to Rust `PartialOrd`.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Hash,
        "Hash",
        "Hash",
        "Hashing support.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Clone,
        "Clone",
        "Clone",
        "Cloning support.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Debug,
        "Debug",
        "std::fmt::Debug",
        "Debug formatting.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Display,
        "Display",
        "std::fmt::Display",
        "User-facing string formatting.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Serialize,
        "Serialize",
        "serde::Serialize",
        "Serde serialization.",
        RFC::_023,
        Since(0, 2),
    ),
    mapping(
        TraitBoundId::Deserialize,
        "Deserialize",
        "serde::de::DeserializeOwned",
        "Serde deserialization (owned).",
        RFC::_023,
        Since(0, 2),
    ),
];

// ============================================================================
// Rust trait path constants for inference (avoids stringly-typed literals in compiler layers)
// ============================================================================

/// Rust trait paths emitted by the trait bound inference engine.
///
/// These constants are the single source of truth for the Rust paths used when scanning generic function bodies for
/// operations on type parameters.
pub mod rust {
    // Comparison
    pub const PARTIAL_EQ: &str = "PartialEq";
    pub const PARTIAL_ORD: &str = "PartialOrd";
    pub const EQ: &str = "Eq";
    pub const HASH: &str = "Hash";

    // Cloning
    pub const CLONE: &str = "Clone";

    // Formatting
    pub const DISPLAY: &str = "std::fmt::Display";

    // Arithmetic ops
    pub const ADD: &str = "std::ops::Add";
    pub const SUB: &str = "std::ops::Sub";
    pub const MUL: &str = "std::ops::Mul";
    pub const DIV: &str = "std::ops::Div";
    pub const REM: &str = "std::ops::Rem";

    // Async
    pub const FUTURE: &str = "std::future::Future";
}

/// Look up the Rust trait path for an Incan trait bound name.
///
/// Returns `Some(rust_path)` for known mappings, `None` for unknown names.
///
/// ## Examples
/// ```rust
/// use incan_core::lang::trait_bounds;
///
/// assert_eq!(trait_bounds::incan_to_rust("Eq"), Some("PartialEq"));
/// assert_eq!(
///     trait_bounds::incan_to_rust("Display"),
///     Some("std::fmt::Display")
/// );
/// assert_eq!(trait_bounds::incan_to_rust("MyTrait"), None);
/// ```
pub fn incan_to_rust(incan_name: &str) -> Option<&'static str> {
    TRAIT_BOUNDS
        .iter()
        .find(|m| m.incan_name == incan_name)
        .map(|m| m.rust_path)
}

/// Look up the Incan name for a Rust trait path.
///
/// Returns `Some(incan_name)` for known mappings, `None` for unknown paths.
pub fn rust_to_incan(rust_path: &str) -> Option<&'static str> {
    TRAIT_BOUNDS
        .iter()
        .find(|m| m.rust_path == rust_path)
        .map(|m| m.incan_name)
}

/// Resolve an Incan trait bound name to a [`TraitBoundId`].
pub fn from_str(name: &str) -> Option<TraitBoundId> {
    TRAIT_BOUNDS.iter().find(|m| m.incan_name == name).map(|m| m.id)
}

/// Return the Incan name for a trait bound.
pub fn as_str(id: TraitBoundId) -> Option<&'static str> {
    info_for(id).map(|m| m.incan_name)
}

/// Return the Rust trait path for a trait bound.
pub fn rust_path(id: TraitBoundId) -> Option<&'static str> {
    info_for(id).map(|m| m.rust_path)
}

/// Return the full metadata entry for a trait bound.
///
/// Returns `None` if the registry is missing an entry for `id` (should not happen with a well-formed registry, but
/// avoids panicking per project policy).
pub fn info_for(id: TraitBoundId) -> Option<&'static TraitBoundMapping> {
    TRAIT_BOUNDS.iter().find(|m| m.id == id)
}

const fn mapping(
    id: TraitBoundId,
    incan_name: &'static str,
    rust_path: &'static str,
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> TraitBoundMapping {
    TraitBoundMapping {
        id,
        incan_name,
        rust_path,
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
    }
}
