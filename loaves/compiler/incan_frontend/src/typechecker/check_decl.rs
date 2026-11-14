//! Second-pass declaration checking: validate models, classes, traits, enums, functions, methods.

use std::sync::Arc;

use crate::ast::*;
use crate::ast_walk::any_expr_in_body;
use crate::diagnostics::errors::{DerivedMember, StoredTypeParamOwner};
use crate::diagnostics::{CompileError, errors};
use crate::resolved_type_subst::{
    substitute_method_info, substitute_property_info, substitute_resolved_type, type_param_subst_map,
};
use crate::symbols::*;
use crate::testing_markers::{TestingFixtureMarkerArgs, TestingMarkerSemantics, resolve_testing_fixture_marker_args};
use crate::typechecker::helpers::{collection_type_id, dict_ty, list_ty};

use super::collect::decorators::resolve_decorator_id;
use super::collect::{capability_description_text, dotted_path_segments};
use super::decorated_method_receivers::DecoratedMethodReceiver;
use super::derive_contract::DerivedKind;
use super::trait_bound_relations::CallableMarkerOwner;
use super::type_info::{
    BodyIrDecoratorApplication, BodyIrDecoratorStaticReceiver, CapabilityDeclarationInfo,
    ProviderOperationDeclarationInfo, RegistryDefinitionInfo, RegistryDescriptionInfo, RegistryDescriptionRegistry,
    RegistryExplicitEntryInfo,
};
use super::{
    DecoratedFunctionBindingInfo, DecoratedMethodBindingInfo, FunctionBindingInfo, MemberBindingSurface, TypeChecker,
    YieldContext,
};
use incan_lang::interop::{RustItemKind, RustItemMetadata, RustTraitAssoc};
use incan_lang::lang::decorators::{self, DecoratorId};
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::magic_methods;
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::testing;
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};
use incan_semantics_core::{SemanticRegistrySubjectKind, SemanticRegistryValue, SurfaceModifierTypeCheck};
use std::collections::{HashMap, HashSet};

use super::ownership_contracts::mutation;
use super::{AnnotationOwner, PUBLIC_LIBRARY_NAMESPACE, ownership_contracts, reachability, type_info};

mod callables;
mod classes;
mod declaration_dispatch;
mod decorator_application;
mod descriptors_and_derives;
mod enums;
mod interop_adapters;
mod member_preparation;
mod method_visibility;
mod models;
mod newtypes;
mod registry_decorators;
mod statics_and_registries;
mod trait_adoptions;
mod trait_declarations;
mod trait_method_resolution;
mod trait_obligations;

/// Prepared lexical inputs of a validated callable, detached between declaration and statement producers.
/// Declaration validation has completed before this value crosses into statement checking. The bootstrap adapter
/// retains the exact symbol scope so generic and parameter identities are not recreated at the boundary.
struct PreparedCallableCheck {
    scope: crate::symbols::PreparedScope,
    annotation_owner: Option<super::AnnotationOwner>,
    bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    mutable_bindings: HashSet<String>,
    mut_param_body: mutation::MutParamBodyState,
    return_error_type: Option<ResolvedType>,
    yield_context: YieldContext,
    has_statement_body: bool,
}

/// Enclosing scratch restored after a prepared callable's statement producer returns.
struct CallableCheckRestore {
    annotation_owner: Option<super::AnnotationOwner>,
    mutable_bindings: HashSet<String>,
    mut_param_body: mutation::MutParamBodyState,
}

/// A function's final declaration inputs and its native-call and receiver context.
pub(super) struct PreparedFunctionCheck {
    callable: PreparedCallableCheck,
    function: Option<Span>,
    c_abi_raw_call_owner: Option<super::CBindingRawCallOwner>,
}

/// Checked property receiver and return inputs, retaining the exact lexical scope without re-resolving annotations.
struct PreparedPropertyCheck {
    scope: crate::symbols::PreparedScope,
    return_error_type: Option<ResolvedType>,
}

/// A method's final declaration inputs and its receiver and classmethod context.
pub(super) struct PreparedMethodCheck {
    callable: PreparedCallableCheck,
    immutable_self_method: Option<String>,
    classmethod_self_ty: Option<ResolvedType>,
}

/// Structural equality for trait method signatures (RFC 042 diamond / obligation merging).
fn method_infos_identical(a: &MethodInfo, b: &MethodInfo) -> bool {
    a.receiver == b.receiver
        && a.is_async == b.is_async
        && a.has_body == b.has_body
        && a.params == b.params
        && a.return_type == b.return_type
}

/// Return whether two trait property requirements are structurally identical.
fn property_infos_identical(a: &PropertyInfo, b: &PropertyInfo) -> bool {
    a.has_body == b.has_body && a.return_type == b.return_type
}

/// Resolve the local type used for a checked function parameter.
fn local_type_for_param(kind: ParamKind, ty: ResolvedType) -> ResolvedType {
    match kind {
        ParamKind::Normal => ty,
        ParamKind::RestPositional => list_ty(ty),
        ParamKind::RestKeyword => dict_ty(ResolvedType::Str, ty),
    }
}

/// Convert a collected function signature into the callable value type that decorators receive.
fn function_info_callable_type(info: &FunctionInfo) -> ResolvedType {
    ResolvedType::Function(info.params.clone(), Box::new(info.return_type.clone()))
}

/// Convert a collected method signature into the callable value type that method decorators receive.
fn method_info_callable_type(info: &MethodInfo, receiver: CallableParam) -> ResolvedType {
    let mut params = Vec::with_capacity(info.params.len() + 1);
    params.push(receiver);
    params.extend(info.params.clone());
    ResolvedType::Function(params, Box::new(info.return_type.clone()))
}

/// Callable signature resolved for one `interop:` adapter reference.
#[derive(Debug, Clone)]
struct InteropAdapterSig {
    name: String,
    receiver: Option<Receiver>,
    params: Vec<ResolvedType>,
    return_type: ResolvedType,
}

#[derive(Debug, Clone)]
struct ResolvedTraitAdoption {
    name: String,
    info: TraitInfo,
    args: Vec<ResolvedType>,
    module_path: Option<Vec<String>>,
    span: Span,
}

#[derive(Debug, Clone)]
pub(in crate::typechecker) struct TraitMethodEntry {
    pub method_name: String,
    pub origin_trait: String,
    pub origin_type_args: Vec<ResolvedType>,
    pub origin_module_path: Option<Vec<String>>,
    pub implementation_type_params: Vec<ImplementationTypeParamInfo>,
    pub info: MethodInfo,
}

type TraitMethodMatch = (String, Option<Vec<String>>, Vec<ResolvedType>, MethodInfo);

enum AsyncFixtureYieldShape {
    Valid { has_teardown: bool },
    Missing,
    Invalid(Span),
    MissingValue(Span),
}

/// Return a top-level fixture `yield` expression when the statement is exactly `yield ...`.
fn top_level_fixture_yield(stmt: &Spanned<Statement>) -> Option<&Spanned<Expr>> {
    if let Statement::Expr(expr) = &stmt.node
        && matches!(expr.node, Expr::Yield(_))
    {
        return Some(expr);
    }
    None
}

/// Validate the declaration-only async fixture yield shape required before runner execution can be async-aware.
fn validate_async_fixture_yield_shape(body: &[Spanned<Statement>]) -> AsyncFixtureYieldShape {
    let top_level_yields: Vec<(usize, &Spanned<Expr>)> = body
        .iter()
        .enumerate()
        .filter_map(|(index, stmt)| top_level_fixture_yield(stmt).map(|expr| (index, expr)))
        .collect();

    if top_level_yields.is_empty() {
        if any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_))) {
            return AsyncFixtureYieldShape::Invalid(body.first().map_or_else(Span::default, |stmt| stmt.span));
        }
        return AsyncFixtureYieldShape::Missing;
    }
    if top_level_yields.len() != 1 {
        return AsyncFixtureYieldShape::Invalid(top_level_yields[1].1.span);
    }

    let (yield_index, yield_expr) = top_level_yields[0];
    let has_yield_elsewhere = body.iter().enumerate().any(|(index, stmt)| {
        index != yield_index && any_expr_in_body(std::slice::from_ref(stmt), |expr| matches!(expr, Expr::Yield(_)))
    });
    if has_yield_elsewhere {
        return AsyncFixtureYieldShape::Invalid(yield_expr.span);
    }

    if matches!(yield_expr.node, Expr::Yield(None)) {
        return AsyncFixtureYieldShape::MissingValue(yield_expr.span);
    }

    AsyncFixtureYieldShape::Valid { has_teardown: true }
}

/// Return whether a function body contains any valued `return` statement.
fn body_has_return_value(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|stmt| match &stmt.node {
        Statement::Return(Some(_)) => true,
        Statement::If(if_stmt) => {
            body_has_return_value(&if_stmt.then_body)
                || if_stmt
                    .elif_branches
                    .iter()
                    .any(|(_, body)| body_has_return_value(body))
                || if_stmt.else_body.as_deref().is_some_and(body_has_return_value)
        }
        Statement::Loop(loop_stmt) => body_has_return_value(&loop_stmt.body),
        Statement::While(while_stmt) => body_has_return_value(&while_stmt.body),
        Statement::For(for_stmt) => body_has_return_value(&for_stmt.body),
        _ => false,
    })
}

/// Pick a stable declaration span for fixture-level diagnostics when the AST helper receives only the function node.
fn fixture_function_span(func: &FunctionDecl) -> Span {
    func.decorators.first().map_or_else(
        || func.body.first().map_or_else(Span::default, |stmt| stmt.span),
        |dec| dec.span,
    )
}

/// Return whether a decorator resolves to the RFC 004 `std.testing.fixture` marker path.
fn is_possible_testing_fixture_decorator(
    dec: &Decorator,
    aliases: &impl crate::decorator_resolution::DecoratorPrefixLookup,
) -> bool {
    let resolved = crate::decorator_resolution::resolve_decorator_path(dec, aliases);
    resolved.len() == 3
        && resolved[0] == stdlib::STDLIB_ROOT
        && resolved[1] == testing::STDLIB_TESTING_MODULE
        && resolved[2] == testing::TESTING_MARKER_FIXTURE
}

/// Return whether any declaration in this slice of AST may be a `std.testing.fixture`.
fn declarations_may_contain_testing_fixture(
    declarations: &[Spanned<Declaration>],
    aliases: &impl crate::decorator_resolution::DecoratorPrefixLookup,
) -> Option<Span> {
    for decl in declarations {
        match &decl.node {
            Declaration::Function(func) => {
                if let Some(decorator) = func
                    .decorators
                    .iter()
                    .find(|decorator| is_possible_testing_fixture_decorator(&decorator.node, aliases))
                {
                    return Some(decorator.span);
                }
            }
            Declaration::TestModule(test_module) => {
                if let Some(span) = declarations_may_contain_testing_fixture(&test_module.body, aliases) {
                    return Some(span);
                }
            }
            _ => {}
        }
    }

    None
}
