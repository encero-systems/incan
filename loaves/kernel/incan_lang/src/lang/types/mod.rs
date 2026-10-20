//! Builtin type vocabularies.
//!
//! This module defines registries for builtin/blessed type names (and their aliases) that are recognized by the
//! compiler.
//!
//! ## Notes
//! - These registries are vocabulary only: they define spellings + metadata, not type system semantics.
//! - Each submodule groups a small family of types for readability.
//! - One spelling rule ([`spelling_matches`]) and one identity ([`builtin_type_name`]) serve every registry; each entry
//!   also names the `descriptor` spelling shown to people (`Int`, `Str`, `Tuple`).
//!
//! ## See also
//! - [`crate::lang::registry`] for shared metadata types
//! - [`crate::lang::keywords`] and [`crate::lang::operators`] for other language vocabularies

pub mod collections;
pub mod numerics;
pub mod stringlike;

/// Canonical semantic constructor name for anonymous union types.
///
/// Frontend, IR, and replacement consumers share this identity instead of independently spelling `Union` at their
/// phase boundaries.
pub const UNION_TYPE_NAME: &str = "Union";

pub use collections::{COLLECTION_TYPES, CollectionTypeId, CollectionTypeInfo};
pub use numerics::{
    DECIMAL_TYPE_CONSTRUCTORS, DecimalTypeConstructorId, DecimalTypeConstructorInfo, NUMERIC_TYPES, NumericFamily,
    NumericTypeId, NumericTypeInfo,
};
pub use stringlike::{STRING_LIKE_TYPES, StringLikeId, StringLikeInfo};

/// The one spelling rule for builtin type names: ASCII case never tells two spellings apart, so `str`, `Str` and `STR`
/// name one type. Every registry lookup in this module goes through it.
pub(crate) fn spelling_matches(registered: &str, written: &str) -> bool {
    registered.eq_ignore_ascii_case(written)
}

/// One builtin type name, whichever registry owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTypeName {
    Numeric(NumericTypeId),
    StringLike(StringLikeId),
    Collection(CollectionTypeId),
}

impl BuiltinTypeName {
    /// The canonical spelling every alias of this type resolves to.
    pub fn canonical(self) -> &'static str {
        match self {
            BuiltinTypeName::Numeric(id) => numerics::as_str(id),
            BuiltinTypeName::StringLike(id) => stringlike::as_str(id),
            BuiltinTypeName::Collection(id) => collections::as_str(id),
        }
    }

    /// The spelling type descriptors show (diagnostics, hover, generated reference): `Int`, `Str`, `Tuple`.
    pub fn descriptor(self) -> &'static str {
        match self {
            BuiltinTypeName::Numeric(id) => numerics::descriptor(id),
            BuiltinTypeName::StringLike(id) => stringlike::descriptor(id),
            BuiltinTypeName::Collection(id) => collections::descriptor(id),
        }
    }
}

/// Resolve any spelling of a builtin type name (canonical or alias, in any ASCII case) to its one identity.
pub fn builtin_type_name(spelling: &str) -> Option<BuiltinTypeName> {
    numerics::from_str(spelling)
        .map(BuiltinTypeName::Numeric)
        .or_else(|| stringlike::from_str(spelling).map(BuiltinTypeName::StringLike))
        .or_else(|| collections::from_str(spelling).map(BuiltinTypeName::Collection))
}
