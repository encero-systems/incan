//! Check calls, constructors, and builtins.
//!
//! This module keeps the call-expression coordinator (`foo(...)`) thin and delegates argument binding, constructor
//! handling, generic inference, builtin dispatch, and Rust boundary validation to focused child modules.

use crate::ast::{CallArg, Expr, ParamKind, Span, Spanned, Type};
use crate::diagnostics::errors::TypeArgumentOrigin;
use crate::diagnostics::{CompileError, errors};
use crate::resolved_type_subst::substitute_resolved_type;
use crate::symbols::{
    CallableParam, FieldInfo, FunctionInfo, FunctionOverloadInfo, ResolvedType, RustItemInfo, SymbolKind, TypeInfo,
};
use crate::typechecker::type_info::{
    CAbiBridgeCall, CAbiOutputSlot, CAbiSpanKind, CBindingRawCall, CBindingSymbol, CBindingType, COutputMode,
    CResourceAccess,
};
use crate::typechecker::{CAbiRawCallResult, IdentKind, PendingCAbiOutputSlot, canonical_public_library_type_name};
use incan_lang::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustTypeInfo, compiler_owned_function_signature,
    metadata_free_function_signature,
};
use incan_lang::lang::c_abi;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::types::SurfaceTypeId;
use incan_lang::lang::traits::{self, TraitId};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_semantics_core::body_ir::CheckedBridgeOperation;
use std::collections::{HashMap, HashSet};

use super::TypeChecker;

mod args;
mod body_ir_factories;
mod body_ir_generics;
mod builtins;
mod c_abi_constructors;
mod c_raw_calls;
mod constructors;
mod declared_direct_calls;
mod direct_calls;
mod function_overloads;
mod generic_bounds;
mod open_values;
mod rust_boundary;
mod rust_callable_bounds;
mod rust_layouts;
mod rust_predicate_inference;
mod task_arguments;
mod type_constructors;

pub(in crate::typechecker::check_expr) use generic_bounds::first_open_type_param;

/// Source-facing and canonical identity for one constructor reached through a public package namespace.
pub(super) struct PublicModuleConstructorContext<'a> {
    pub display_name: &'a str,
    pub canonical_name: &'a str,
    pub type_info: &'a TypeInfo,
}

/// Borrow the written call and its destination while the checker selects a declaration and checks arguments.
struct DirectCallSite<'a> {
    callee: &'a Spanned<Expr>,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    span: Span,
    expected_return_ty: Option<&'a ResolvedType>,
}

/// Return whether the last Rust path segment looks like a type name.
fn rust_path_last_segment_looks_like_type(path: &str) -> bool {
    path.rsplit("::")
        .next()
        .unwrap_or(path)
        .trim_start_matches("r#")
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}
