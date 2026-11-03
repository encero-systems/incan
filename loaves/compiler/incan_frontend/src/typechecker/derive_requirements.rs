//! The compiler's one relation for the builtin derives, and the refusals built on it.
//!
//! [`TypeChecker::derive_support`] answers whether a checked type implements `Clone`, `Debug`, `Copy`, `Default`,
//! `PartialEq`, `Eq`, `PartialOrd`, `Ord` or `Hash` in the generated program, reading the surface-type registry for
//! runtime types ([`surface_types::derive_support`]), the derive implication table for declared types
//! ([`derives::implied_derives`]), and the automatic derives lowering gives a `model`, `class`, `enum` or newtype
//! (#1870). Generic bound checks (`type_satisfies_explicit_bound`), the clone requirement of collection methods
//! (`is_clone_type`) and the comparison operators consult it before their own fallbacks, and the refusals below are
//! built on it:
//!
//! - a `model` or `class` field, or an `enum` payload, whose type cannot carry the automatic `Clone` and `Debug`
//!   derives (`INCAN-T0113`, #1754);
//! - a set element or dict key type without `Eq` and `Hash`, including the type argument of a generic function or
//!   method whose body hashes its parameter, a requirement inferred into its signature by `hash_key_inference`
//!   (`INCAN-T0114`, #1758).
//!
//! The relation answers [`DeriveSupport::Unknown`] rather than guess: for a type parameter, a Rust-origin type without
//! an unconditional native Clone proof, a `rusttype`, a runtime type the registry records nothing for, and a derive of
//! a type declared in another module that its manifest does not list, since a manifest may not list every derive. The
//! refusals never refuse on an unknown answer.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, DictEntry, Expr, ListEntry, ParamKind, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::diagnostics::errors::{self, DerivedMember, HashRemedy, HashedCollectionRole};
use crate::symbols::{
    CallableParam, ClassInfo, EnumInfo, MethodInfo, NewtypeInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo,
};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::interop::RustItemKind;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceDeriveSupport, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use incan_semantics_core::CanonicalSymbolId;

mod derive_support;
mod hash_requirements;

/// What the compiler knows about one type's implementation of a builtin derive.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::typechecker) enum DeriveSupport {
    /// The type implements the derive in the generated program.
    Supported,
    /// The type does not implement it; the payload is the type inside it that lacks the derive (the type itself, or
    /// `JoinHandle[int]` inside `list[JoinHandle[int]]`).
    Missing(ResolvedType),
    /// The compiler cannot tell; each consumer applies its own policy for an unknown type.
    Unknown,
}

/// `@rust.derive(...)` facts about one nominal type declared in the module being checked.
///
/// Recorded during collection for every local `model`, `class`, `enum` and newtype, so the presence of an entry is
/// also what marks a declaration as local: the derive list of a local declaration is complete, while an imported one
/// comes from a manifest that may omit derives.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct LocalDeriveFacts {
    /// Builtin derives spelled in `@rust.derive(...)`, by bare name (`Hash`) or by `std`/`core` path.
    pub rust_builtin_derives: Vec<DeriveId>,
    /// Whether `@rust.derive(...)` names a derive macro the compiler cannot classify, which may implement anything.
    pub has_unclassified_rust_derive: bool,
}

/// Whether `derive` is one the relation answers for: every builtin derive that is a Rust trait of the generated
/// program. `Display` is not (the display rule answers for it, see `check_expr::printed_form`), nor are `Validate` and
/// `Descriptor`.
fn is_relation_derive(derive: DeriveId) -> bool {
    matches!(
        derive,
        DeriveId::Clone
            | DeriveId::Debug
            | DeriveId::Copy
            | DeriveId::Default
            | DeriveId::PartialEq
            | DeriveId::Eq
            | DeriveId::PartialOrd
            | DeriveId::Ord
            | DeriveId::Hash
    )
}

/// Whether `derive` needs a total order or a hash, which a binary float does not have: `Eq`, `Ord` and `Hash`.
fn needs_total_comparison(derive: DeriveId) -> bool {
    matches!(derive, DeriveId::Eq | DeriveId::Ord | DeriveId::Hash)
}

/// Whether an enum payload type lets the enum take lowering's automatic `PartialEq`: a number, `bool`, `str` or
/// `bytes`, or a `list`, `set`, `dict`, `Option`, `Result` or tuple of those. A declared type never does, whatever it
/// derives, since lowering decides on the lowered payload type alone.
fn payload_defaults_partial_eq(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Unit
        | ResolvedType::Bool
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes => true,
        ResolvedType::Tuple(items) => items.iter().all(payload_defaults_partial_eq),
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) | ResolvedType::TypeToken(inner) => {
            payload_defaults_partial_eq(inner)
        }
        ResolvedType::Generic(name, args) => {
            if numerics::decimal_constructor_from_str(name).is_some() {
                return true;
            }
            match collection_type_id(name) {
                Some(CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Option) => {
                    args.len() == 1 && args.iter().all(payload_defaults_partial_eq)
                }
                Some(CollectionTypeId::Dict | CollectionTypeId::Result) => {
                    args.len() == 2 && args.iter().all(payload_defaults_partial_eq)
                }
                Some(CollectionTypeId::Tuple) => args.iter().all(payload_defaults_partial_eq),
                _ => false,
            }
        }
        _ => false,
    }
}
