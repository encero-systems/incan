//! Rust interop vocabulary shared across compiler stages (RFC 005 / RFC 041).
//!
//! Host-specific extraction (rust-analyzer, Cargo) lives in the `incan` crate behind the `rust-metadata` feature; this
//! module holds only portable data shapes.

pub mod capabilities;
pub mod coercions;
pub mod metadata;

pub use capabilities::{RUST_CAPABILITY_BOUNDS, is_rust_capability_bound};
pub use coercions::{CoercionPolicy, admitted_builtin_coercion};
pub use metadata::{
    RustCollectionFamily, RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig,
    RustModuleChild, RustModuleChildKind, RustModuleInfo, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo,
    RustTypeShape, RustVariantInfo, RustVisibility,
};
