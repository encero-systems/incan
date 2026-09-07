//! Builtin type vocabularies.
//!
//! This module defines registries for builtin/blessed type names (and their aliases) that are
//! recognized by the compiler.
//!
//! ## Notes
//! - These registries are vocabulary only: they define spellings + metadata, not type system semantics.
//! - Each submodule groups a small family of types for readability.
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
