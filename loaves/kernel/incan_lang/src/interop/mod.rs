//! Rust interop vocabulary shared across compiler stages (RFC 005 / RFC 041).
//!
//! The compiler reads Rust facts from rustc's crate metadata (`incan-rustc-driver`); this module holds only portable
//! data shapes and the ABI cache they are stored in.

pub mod abi_cache;
pub mod capabilities;
pub mod coercions;
mod extension_traits;
pub mod metadata;

pub use abi_cache::{RUST_ABI_CACHE_VERSION, RustAbiCache, encode_rust_abi_cache};
pub use capabilities::{
    RUST_CALLABLE_CAPABILITY_BOUNDS, RUST_CAPABILITY_BOUNDS, RUST_FUTURE_CAPABILITY_BOUNDS,
    is_rust_callable_capability_bound, is_rust_capability_bound, is_rust_future_capability_bound,
};
pub use coercions::{CoercionPolicy, admitted_builtin_coercion};
pub use extension_traits::fallback_rust_trait_methods;
pub use metadata::{
    COMPILER_OWNED_FUNCTION_SIGNATURE_RULES, METADATA_FREE_FUNCTION_SIGNATURE_RULES, RUST_NEVER_TYPE_DISPLAY,
    RustAssociatedTypeBinding, RustAssociatedTypeRequirement, RustCollectionFamily, RustExpandedDeriveTrait,
    RustFieldInfo, RustFunctionParamRule, RustFunctionSig, RustFunctionSignatureRule, RustImplementedTrait,
    RustItemKind, RustItemMetadata, RustMacroInfo, RustMethodSig, RustModuleChild, RustModuleChildKind, RustModuleInfo,
    RustMutableReferenceCandidate, RustMutableReferenceTypeParam, RustParam, RustPayloadCarrier, RustReceiverContract,
    RustTraitAssoc, RustTraitBound, RustTraitInfo, RustTypeInfo, RustTypeMetadataCompleteness, RustTypeShape,
    RustTypeShapePathFallback, RustVariantInfo, RustVisibility, ancestral_rust_display, ancestral_rust_path,
    compiler_owned_function_signature, is_box_carrier_path, metadata_free_function_signature,
    parse_rust_type_shape_text, render_rust_type_shape, render_rust_type_shape_path, rust_display_is_callable_bound,
    rust_display_is_owned_string, rust_source_borrowed_type_param_bound_display,
    rust_source_callable_bound_for_type_param, rust_source_type_param_has_as_fd_bound, split_top_level_rust_args,
    strip_rust_borrow_lifetimes,
};
