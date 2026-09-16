//! The mandatory Incan standard library runtime: the Rust facet of the `core` component.
//!
//! Every generated Incan program links this crate. It holds what compiler-generated Rust calls for language features
//! rather than for any one `std.*` namespace: reflection traits and their field records, the frozen constant types,
//! Python-shaped numeric and string operations, collection helpers that raise canonical errors instead of Rust's
//! indexing panics, static storage, validation, and the stdlib version check every generated crate carries. The
//! optional runtime surfaces live in the facets of the components that own their namespaces — `incan_std_data`,
//! `incan_std_async`, `incan_std_web`, `incan_std_testing` — and depend on this crate; nothing here reaches into them.
//!
//! The stable boundary for Incan users is the `std.*` API declared by the component sources beside this crate. Rust
//! modules here are runtime support for compiler-generated Rust and may contain transitional host implementations
//! while the corresponding Incan-language surface is still being built.

#![deny(clippy::unwrap_used)]

pub mod collections;
pub mod conversions;
pub mod errors;
pub mod frozen;
pub mod iter;
pub mod num;
pub mod prelude;
pub mod reflection;
pub mod storage;
pub mod strings;
pub mod validation;
pub mod version;

// Re-export commonly used items
pub use reflection::{
    FieldInfo, HasClassName, HasFieldInfo, HasFieldMetadata, HasFieldValueReflection, HasTypeClassName,
    HasTypeFieldMetadata,
};
