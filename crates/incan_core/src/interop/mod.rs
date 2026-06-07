//! Rust interop vocabulary shared across compiler stages (RFC 005 / RFC 041).
//!
//! Host-specific extraction (rust-analyzer, Cargo) lives in the `rust_inspect` crate (wired by `incan` behind the
//! `rust_inspect` feature);
//! this module holds only portable data shapes.

pub mod capabilities;
pub mod coercions;
mod extension_traits;
pub mod metadata;

pub use capabilities::{RUST_CAPABILITY_BOUNDS, is_rust_capability_bound};
pub use coercions::{CoercionPolicy, admitted_builtin_coercion};
pub use extension_traits::fallback_rust_trait_methods;
pub use metadata::{
    METADATA_FREE_METHOD_BORROW_RULES, METADATA_FREE_METHOD_SIGNATURE_RULES, MetadataFreeArgClass,
    MetadataFreeMethodArgBorrowPolicy, MetadataFreeMethodBorrowRule, MetadataFreeMethodParamRule,
    MetadataFreeMethodSignatureRule, MetadataFreeReceiverClass, RustCollectionFamily, RustFieldInfo, RustFunctionSig,
    RustImplementedTrait, RustItemKind, RustItemMetadata, RustMethodSig, RustModuleChild, RustModuleChildKind,
    RustModuleInfo, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo, RustTypeMetadataCompleteness,
    RustTypeShape, RustTypeShapePathFallback, RustVariantInfo, RustVisibility, metadata_free_method_signature,
    parse_rust_type_shape_text, render_rust_type_shape, render_rust_type_shape_path, split_top_level_rust_args,
    strip_rust_borrow_lifetimes,
};
