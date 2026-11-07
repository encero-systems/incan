//! Transitional Rust support retained by the `core` standard-library component.
//!
//! Static storage, conversions and the standard-library version check remain here until the final core fold.
//! Foundational errors, reflection, validation, strings, numerics, collections, iterators and frozen carriers live in
//! `incan_std_primitives`; their re-exports below preserve bootstrap and higher-facet paths without creating a second
//! compiler identity.
//!
//! The stable boundary for Incan users is the `std.*` API declared by the component sources beside this crate. Rust
//! modules here are runtime support for compiler-generated Rust and may contain transitional host implementations
//! while the corresponding Incan-language surface is still being built.

#![deny(clippy::unwrap_used)]

pub mod conversions;
pub mod prelude;
pub mod storage;
pub mod version;

/// Preserve the previous core-facet module paths while installed compilers bootstrap the primitive identities.
pub use incan_std_primitives::{collections, errors, frozen, iter, num, reflection, strings, validation};

// Re-export commonly used items
pub use reflection::{
    FieldInfo, HasClassName, HasFieldInfo, HasFieldMetadata, HasFieldValueReflection, HasTypeClassName,
    HasTypeFieldMetadata,
};
