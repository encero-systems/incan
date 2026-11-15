//! Trait-bound satisfaction and temporary capability bridges.

use std::collections::{HashMap, HashSet};

use super::TypeChecker;
use super::derive_requirements::DeriveSupport;
use crate::ast::{Receiver, Type, TypeParam};
use crate::diagnostics::errors::{self, CallableMarkerRefusal, GenericBoundTarget};
use crate::resolved_type_subst::substitute_resolved_type;
use crate::symbols::{ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo, resolve_type};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::interop::{
    is_rust_callable_capability_bound, is_rust_capability_bound, is_rust_future_capability_bound,
};
use incan_lang::lang::callables;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::stdlib;
use incan_lang::lang::trait_capabilities::{
    self, TraitCapabilityId, TraitCapabilityInfo, TraitCapabilityType, TraitCapabilityTypeArg,
};
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::canonical_module_identity;

mod explicit_bounds;
mod temporary_capabilities;
mod type_param_and_nominal_bounds;

/// Which declaration's type parameters [`TypeChecker::refuse_unsupported_callable_markers`] is reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typechecker) enum CallableMarkerOwner<'a> {
    /// A function or method: a call site exists to determine a marker's return type.
    Callable,
    /// A model, class, enum, trait, newtype or type alias, named by its declaration keyword and name.
    Nominal {
        /// The declaration keyword as the source spells it.
        kind: &'a str,
        /// The declaration's name.
        name: &'a str,
    },
}
