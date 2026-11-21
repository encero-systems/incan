//! The compiler record types a plan names: a checked carrier type and a canonical source-trait identity.
//!
//! The record models own these types; a plan re-exports them so its consumers name one type.

pub use internal_models_native::checked_types::{
    CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType, CheckedPrimitiveType,
    CheckedRuntimeBuiltin, CheckedType,
};
pub use internal_models_native::compiler_records::{
    CanonicalSymbolId, DeclarationKind, HirSourceSpan, PackageIdentity, SymbolNamespace, SymbolOrigin,
};
