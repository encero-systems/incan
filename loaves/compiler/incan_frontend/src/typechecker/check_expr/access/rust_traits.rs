//! Select retained trait callees after inherent lookup, using the extractor's impl templates and signatures.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::ops::contextual_integer_literal_fits;
use super::{RustPathMethodCall, RustTraitQualifiedCall, TypeChecker};

mod native_values;
mod trait_calls;
mod trait_candidates;
use crate::ast::{Expr, Spanned};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type};
use crate::symbols::ResolvedType;
use crate::typechecker::IdentKind;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use incan_lang::interop::{
    RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata, RustTraitAssoc, RustTraitInfo, RustTypeInfo,
};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::numerics;

/// A selected trait reference and signature, with closed impl binders and any still-open method result parameters.
struct TraitCandidate {
    path: String,
    signature: RustFunctionSig,
    arguments: Vec<ResolvedType>,
    self_type: ResolvedType,
    borrow_cost: usize,
    /// Whether the trait declares its result as `Self`, which a rusttype receiver keeps as the rusttype.
    returns_self: bool,
    /// Method binders still requiring a typed use, distinct from equally named enclosing source binders.
    open_parameters: Vec<String>,
}
