//! Second-pass declaration checking: validate models, classes, traits, enums, functions, methods.

use crate::frontend::ast::*;
use crate::frontend::ast_walk::any_expr_in_body;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::resolved_type_subst::{
    substitute_method_info, substitute_property_info, substitute_resolved_type, type_param_subst_map,
};
use crate::frontend::symbols::*;
use crate::frontend::testing_markers::{
    TestingFixtureMarkerArgs, TestingMarkerSemantics, load_testing_marker_semantics,
    resolve_testing_fixture_marker_args,
};
use crate::frontend::typechecker::helpers::{collection_type_id, dict_ty, list_ty};

use super::{DecoratedFunctionBindingInfo, DecoratedMethodBindingInfo, TestingFixtureInfo, TypeChecker, YieldContext};
use incan_core::interop::{RustItemKind, RustItemMetadata, RustTraitAssoc};
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;
use incan_core::lang::stdlib;
use incan_core::lang::testing;
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_semantics_core::SurfaceModifierTypeCheck;
use std::collections::{HashMap, HashSet};

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
fn method_info_callable_type(info: &MethodInfo, receiver_ty: ResolvedType) -> ResolvedType {
    let mut params = Vec::with_capacity(info.params.len() + 1);
    params.push(CallableParam::named("self", receiver_ty, ParamKind::Normal));
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
pub(in crate::frontend::typechecker) struct TraitMethodEntry {
    pub method_name: String,
    pub origin_trait: String,
    pub origin_type_args: Vec<ResolvedType>,
    pub origin_module_path: Option<Vec<String>>,
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

/// Return whether any yield expression appears in a fixture body.
fn fixture_body_has_yield(body: &[Spanned<Statement>]) -> bool {
    any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_)))
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
fn is_possible_testing_fixture_decorator(dec: &Decorator, aliases: &HashMap<String, Vec<String>>) -> bool {
    let resolved = crate::frontend::decorator_resolution::resolve_decorator_path(dec, aliases);
    resolved.len() == 3
        && resolved[0] == stdlib::STDLIB_ROOT
        && resolved[1] == testing::STDLIB_TESTING_MODULE
        && resolved[2] == testing::TESTING_MARKER_FIXTURE
}

/// Return whether any declaration in this slice of AST may be a `std.testing.fixture`.
fn declarations_may_contain_testing_fixture(
    declarations: &[Spanned<Declaration>],
    aliases: &HashMap<String, Vec<String>>,
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

impl TypeChecker {
    /// Collect fixture names before function bodies are checked so dependency metadata is independent of declaration
    /// order.
    pub(crate) fn collect_testing_fixture_names(&mut self, program: &Program) {
        self.testing_fixture_names.clear();
        let Some(span) = declarations_may_contain_testing_fixture(&program.declarations, &self.import_aliases) else {
            return;
        };
        let semantics = match load_testing_marker_semantics() {
            Ok(semantics) => semantics,
            Err(err) => {
                self.errors
                    .push(errors::invalid_std_testing_marker_metadata(&err.to_string(), span));
                return;
            }
        };
        self.collect_testing_fixture_names_from_decls(&program.declarations, &semantics);
    }

    /// Recursively collect fixture names from top-level and inline test-module declarations.
    fn collect_testing_fixture_names_from_decls(
        &mut self,
        declarations: &[Spanned<Declaration>],
        semantics: &TestingMarkerSemantics,
    ) {
        for decl in declarations {
            match &decl.node {
                Declaration::Function(func)
                    if resolve_testing_fixture_marker_args(&func.decorators, &self.import_aliases, semantics)
                        .is_some() =>
                {
                    self.testing_fixture_names.insert(func.name.clone());
                }
                Declaration::TestModule(test_module) => {
                    self.collect_testing_fixture_names_from_decls(&test_module.body, semantics);
                }
                _ => {}
            }
        }
    }

    /// Resolve fixture marker arguments for a declaration, reporting stdlib metadata failures through type diagnostics.
    fn testing_fixture_marker_args(
        &mut self,
        decorators: &[Spanned<Decorator>],
        span: Span,
    ) -> Option<TestingFixtureMarkerArgs> {
        if !decorators
            .iter()
            .any(|decorator| is_possible_testing_fixture_decorator(&decorator.node, &self.import_aliases))
        {
            return None;
        }

        match load_testing_marker_semantics() {
            Ok(semantics) => resolve_testing_fixture_marker_args(decorators, &self.import_aliases, &semantics),
            Err(err) => {
                self.errors
                    .push(errors::invalid_std_testing_marker_metadata(&err.to_string(), span));
                None
            }
        }
    }

    /// Enforce source-language ordering and default rules for `*args` / `**kwargs` declarations.
    fn validate_callable_rest_params(&mut self, params: &[Spanned<Param>]) {
        let mut saw_rest_positional = false;
        let mut saw_rest_keyword = false;
        let mut saw_rest = false;

        for param in params {
            match param.node.kind {
                ParamKind::Normal => {
                    if saw_rest_keyword {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "Normal parameters cannot appear after a `**kwargs` rest parameter",
                            param.span,
                        ));
                    } else if saw_rest {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "Normal parameters cannot appear after a rest parameter",
                            param.span,
                        ));
                    }
                }
                ParamKind::RestPositional => {
                    if saw_rest_positional {
                        self.errors.push(errors::duplicate_rest_parameter("*args", param.span));
                    }
                    if saw_rest_keyword {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "`*args` must appear before `**kwargs`",
                            param.span,
                        ));
                    }
                    if param.node.default.is_some() {
                        self.errors
                            .push(errors::rest_parameter_default_not_allowed(&param.node.name, param.span));
                    }
                    saw_rest_positional = true;
                    saw_rest = true;
                }
                ParamKind::RestKeyword => {
                    if saw_rest_keyword {
                        self.errors
                            .push(errors::duplicate_rest_parameter("**kwargs", param.span));
                    }
                    if param.node.default.is_some() {
                        self.errors
                            .push(errors::rest_parameter_default_not_allowed(&param.node.name, param.span));
                    }
                    saw_rest_keyword = true;
                    saw_rest = true;
                }
            }
        }
    }

    /// Run declaration-level typecheck actions selected by the surface semantics registry.
    fn validate_surface_modifier_typecheck_actions(
        &mut self,
        modifiers: &[SurfaceModifier],
        return_type: &Spanned<Type>,
    ) {
        use crate::semantics_registry::semantics_registry;

        for modifier in modifiers {
            let Some(action) = semantics_registry().typecheck_surface_modifier_action(&modifier.key) else {
                continue;
            };
            match action {
                SurfaceModifierTypeCheck::AsyncCallable => self.validate_async_callable_return_type(return_type),
            }
        }
    }

    /// Validate return annotations for an async callable.
    ///
    /// Incan async declarations spell the callable's logical output type, not an explicit `Future[T]` wrapper, so no
    /// additional return-type rejection is currently applicable. Keeping this routed through the semantics registry
    /// prevents `async` from becoming another hardcoded declaration special case.
    fn validate_async_callable_return_type(&mut self, return_type: &Spanned<Type>) {
        let _ = return_type;
    }

    /// Return whether a method carries a resolved builtin decorator.
    fn method_has_decorator(method: &MethodDecl, id: DecoratorId) -> bool {
        method
            .decorators
            .iter()
            .any(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(id))
    }

    /// Replace every nested `Self` occurrence in an annotation with the concrete owner type used for this method body.
    ///
    /// Method return validation runs against the concrete owning type, not the abstract declaration surface, so
    /// containers like `List[Self]` must be concretized recursively before compatibility checks.
    ///
    /// **Call sites** use `substitute_self_in_resolved_type` in `check_expr/access.rs` instead: there `Self` is
    /// resolved from the **instantiated** receiver expression (e.g. `Carrier[Order]` for `x.filter(...)`).
    fn concretize_self_type_in_annotation(ty: &ResolvedType, self_ty: &ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::SelfType => self_ty.clone(),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| Self::concretize_self_type_in_annotation(arg, self_ty))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .iter()
                    .map(|item| Self::concretize_self_type_in_annotation(item, self_ty))
                    .collect(),
            ),
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .iter()
                    .map(|param| crate::frontend::symbols::CallableParam {
                        name: param.name.clone(),
                        ty: Self::concretize_self_type_in_annotation(&param.ty, self_ty),
                        kind: param.kind,
                        has_default: param.has_default,
                    })
                    .collect(),
                Box::new(Self::concretize_self_type_in_annotation(ret, self_ty)),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(Self::concretize_self_type_in_annotation(key, self_ty)),
                Box::new(Self::concretize_self_type_in_annotation(value, self_ty)),
            ),
            ResolvedType::Ref(inner) => {
                ResolvedType::Ref(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(Self::concretize_self_type_in_annotation(inner, self_ty)))
            }
            _ => ty.clone(),
        }
    }

    /// Build the resolved surface type that represents `nt` in interop signature validation.
    ///
    /// Generic rusttypes are represented as `Generic(Name, TypeVar...)` so adapter checks compare against the
    /// instantiated surface, not a monomorphic `Named`.
    fn rusttype_decl_resolved_type(&self, nt: &NewtypeDecl) -> ResolvedType {
        if nt.type_params.is_empty() {
            return ResolvedType::Named(nt.name.clone());
        }
        let args = nt
            .type_params
            .iter()
            .map(|tp| ResolvedType::TypeVar(tp.name.clone()))
            .collect();
        ResolvedType::Generic(nt.name.clone(), args)
    }

    /// Human-readable display form for an `interop:` adapter reference expression.
    ///
    /// Used in diagnostics and duplicate-edge bookkeeping so messages show the same spelling users wrote (`name`,
    /// `Owner.member`, or a conservative `<expr>` fallback).
    fn interop_adapter_ref_display(adapter: &Spanned<Expr>) -> String {
        match &adapter.node {
            Expr::Ident(name) => name.clone(),
            Expr::Field(base, member) => match &base.node {
                Expr::Ident(owner) => format!("{owner}.{member}"),
                _ => format!("<expr>.{member}"),
            },
            _ => "<expr>".to_string(),
        }
    }

    /// Resolve all callable adapter candidates for a short adapter name on a rusttype declaration.
    ///
    /// Candidate order is intentional:
    /// 1. Local rusttype-declared methods.
    /// 2. Backing Rust methods from metadata (when available).
    ///
    /// Returns `(candidates, rust_inspect_available)`. The second flag lets callers distinguish "no match because
    /// metadata is unavailable" from "no match with authoritative metadata".
    fn interop_adapter_candidates_for_name(&self, nt: &NewtypeDecl, name: &str) -> (Vec<InteropAdapterSig>, bool) {
        let mut candidates: Vec<InteropAdapterSig> = Vec::new();
        let mut rust_inspect_available = false;

        if let Some(TypeInfo::Newtype(info)) = self.lookup_type_info(&nt.name) {
            if let Some(method) = info.methods.get(name) {
                candidates.push(InteropAdapterSig {
                    name: format!("{}.{}", nt.name, name),
                    receiver: method.receiver,
                    params: method.params.iter().map(|param| param.ty.clone()).collect(),
                    return_type: method.return_type.clone(),
                });
            }

            if info.is_rusttype
                && let ResolvedType::RustPath(path) = &info.underlying
                && let Some(meta) = self.rust_item_metadata_for_path(path)
            {
                rust_inspect_available = true;
                if let RustItemKind::Type(type_info) = &meta.kind {
                    for method in type_info.methods.iter().filter(|m| m.name == name) {
                        let has_receiver = Self::rust_signature_has_receiver(&method.signature);
                        let receiver = if has_receiver { Some(Receiver::Immutable) } else { None };
                        let skip = usize::from(has_receiver);
                        let params = method
                            .signature
                            .params
                            .iter()
                            .skip(skip)
                            .map(|p| {
                                self.resolved_param_type_from_rust_display_for_owner_path(p.type_display.as_str(), path)
                            })
                            .collect();
                        let return_display =
                            self.rust_display_for_owner_path(method.signature.return_type.as_str(), path);
                        candidates.push(InteropAdapterSig {
                            name: format!("rust::{}.{name}", path),
                            receiver,
                            params,
                            return_type: self.resolved_type_from_rust_display(return_display.as_str()),
                        });
                    }
                }
            }
        }

        (candidates, rust_inspect_available)
    }

    /// Decide whether missing-adapter diagnostics should be deferred until metadata is available.
    ///
    /// For rusttypes backed by a Rust path, missing metadata should not become a hard error in the typechecker;
    /// lowering/rustc will still validate concrete callability later.
    fn maybe_defer_interop_adapter_missing(&self, nt: &NewtypeDecl, rust_inspect_available: bool) -> bool {
        if rust_inspect_available {
            return false;
        }
        self.lookup_type_info(&nt.name).is_some_and(|info| {
            matches!(
                info,
                TypeInfo::Newtype(NewtypeInfo {
                    is_rusttype: true,
                    underlying: ResolvedType::RustPath(_),
                    ..
                })
            )
        })
    }

    /// Resolve one `interop:` adapter reference to an unambiguous callable signature.
    ///
    /// Supports short-form (`name`) and qualified (`Type.name`) references, enforces owner checks for qualified forms,
    /// and emits ambiguity/missing diagnostics when metadata is authoritative.
    fn resolve_interop_adapter_signature(
        &mut self,
        nt: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
    ) -> Option<InteropAdapterSig> {
        match &edge.node.adapter.node {
            Expr::Ident(name) => {
                let (mut candidates, rust_inspect_available) = self.interop_adapter_candidates_for_name(nt, name);
                if candidates.is_empty() {
                    if self.maybe_defer_interop_adapter_missing(nt, rust_inspect_available) {
                        return None;
                    }
                    self.errors.push(errors::unknown_symbol(name, edge.node.adapter.span));
                    return None;
                }
                if candidates.len() > 1 {
                    self.errors.push(errors::ambiguous_interop_adapter_short_name(
                        &nt.name,
                        name,
                        candidates.len(),
                        edge.node.adapter.span,
                    ));
                    return None;
                }
                candidates.pop()
            }
            Expr::Field(base, method) => {
                let Expr::Ident(owner) = &base.node else {
                    self.errors.push(errors::interop_adapter_ref_must_be_name_or_member(
                        edge.node.adapter.span,
                    ));
                    return None;
                };
                if owner != &nt.name {
                    self.errors.push(errors::interop_adapter_wrong_owner(
                        &nt.name,
                        owner,
                        edge.node.adapter.span,
                    ));
                    return None;
                }
                let (candidates, rust_inspect_available) = self.interop_adapter_candidates_for_name(nt, method);
                if candidates.is_empty() {
                    if self.maybe_defer_interop_adapter_missing(nt, rust_inspect_available) {
                        return None;
                    }
                    self.errors
                        .push(errors::missing_method(&nt.name, method, edge.node.adapter.span));
                    return None;
                }
                // Qualified references follow ordinary lookup precedence:
                // local rusttype methods first, then backing rust-inspect methods.
                candidates.into_iter().next()
            }
            _ => {
                self.errors.push(errors::interop_adapter_ref_must_be_name_or_member(
                    edge.node.adapter.span,
                ));
                None
            }
        }
    }

    /// Validate adapter shape against one `interop:` edge contract.
    ///
    /// This enforces:
    /// - receiver/arity constraints for `from` vs `into` edges,
    /// - input type compatibility at the boundary,
    /// - `via` (infallible) vs `try` (Result/Option) return-shape rules,
    /// - adapted output compatibility with the target edge direction.
    fn validate_interop_adapter_signature(
        &mut self,
        nt: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
        boundary_ty: &ResolvedType,
        rusttype_ty: &ResolvedType,
        adapter: &InteropAdapterSig,
    ) {
        let (expected_input, expected_output) = match edge.node.direction {
            InteropDirection::From => (boundary_ty, rusttype_ty),
            InteropDirection::Into => (rusttype_ty, boundary_ty),
        };

        match edge.node.direction {
            InteropDirection::From if adapter.receiver.is_some() => {
                self.errors
                    .push(errors::interop_from_adapter_requires_associated_callable(
                        &nt.name,
                        adapter.name.as_str(),
                        edge.node.adapter.span,
                    ));
                return;
            }
            InteropDirection::Into if adapter.receiver.is_some() => {
                if !adapter.params.is_empty() {
                    self.errors.push(errors::interop_adapter_arity_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        0,
                        adapter.params.len(),
                        edge.node.adapter.span,
                    ));
                    return;
                }
            }
            _ => {
                if adapter.params.len() != 1 {
                    self.errors.push(errors::interop_adapter_arity_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        1,
                        adapter.params.len(),
                        edge.node.adapter.span,
                    ));
                    return;
                }
                let Some(found_input) = adapter.params.first() else {
                    return;
                };
                if !self.types_compatible(expected_input, found_input) {
                    self.errors.push(errors::interop_adapter_input_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        &expected_input.to_string(),
                        &found_input.to_string(),
                        edge.node.adapter.span,
                    ));
                }
            }
        }

        let adapted_return = match edge.node.adapter_kind {
            InteropAdapterKind::Via => {
                if adapter.return_type.is_result() || adapter.return_type.is_option() {
                    self.errors.push(errors::interop_via_adapter_must_be_infallible(
                        &nt.name,
                        adapter.name.as_str(),
                        &adapter.return_type.to_string(),
                        edge.node.adapter.span,
                    ));
                }
                adapter.return_type.clone()
            }
            InteropAdapterKind::Try => {
                if adapter.return_type.is_result() {
                    adapter
                        .return_type
                        .result_ok_type()
                        .cloned()
                        .unwrap_or(ResolvedType::Unknown)
                } else if adapter.return_type.is_option() {
                    adapter
                        .return_type
                        .option_inner_type()
                        .cloned()
                        .unwrap_or(ResolvedType::Unknown)
                } else {
                    self.errors.push(errors::interop_try_adapter_requires_result_or_option(
                        &nt.name,
                        adapter.name.as_str(),
                        &adapter.return_type.to_string(),
                        edge.node.adapter.span,
                    ));
                    ResolvedType::Unknown
                }
            }
        };

        if !self.types_compatible(&adapted_return, expected_output) {
            self.errors.push(errors::interop_adapter_output_mismatch(
                &nt.name,
                adapter.name.as_str(),
                &expected_output.to_string(),
                &adapted_return.to_string(),
                edge.node.adapter.span,
            ));
        }
    }

    /// Union of method names reachable through the given traits, including transitive supertrait methods (RFC 042).
    ///
    /// Used when validating field `@alias` metadata so aliases cannot collide with callable members surfaced through
    /// trait adoption.
    fn collect_trait_method_names(&self, traits: &[Spanned<TraitBound>]) -> HashSet<String> {
        let mut names = HashSet::new();
        for trait_ref in traits {
            let trait_name = trait_ref.node.name.as_str();
            if let Some(trait_info) = self.lookup_semantic_trait_info(trait_name) {
                names.extend(trait_info.methods.keys().cloned());
            }
            for (supertrait_name, _) in self.semantic_supertrait_closure(trait_name) {
                if let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) {
                    names.extend(supertrait_info.methods.keys().cloned());
                }
            }
        }
        names
    }

    /// Validate per-field metadata (`@alias`, etc.) on a model-like type against canonical field names and trait method
    /// names.
    fn validate_field_metadata(
        &mut self,
        type_name: &str,
        fields: &[Spanned<FieldDecl>],
        method_names: &HashSet<String>,
    ) {
        let canonical_names: HashSet<String> = fields.iter().map(|f| f.node.name.clone()).collect();
        let mut builtin_member_names: HashSet<&'static str> = HashSet::new();
        for info in magic_methods::MAGIC_METHODS {
            builtin_member_names.insert(info.canonical);
            builtin_member_names.extend(info.aliases);
        }
        let mut seen_aliases: HashMap<String, Span> = HashMap::new();

        for field in fields {
            let Some(alias) = field.node.metadata.alias.as_ref() else {
                continue;
            };

            if alias.trim().is_empty() {
                self.errors.push(errors::empty_alias(field.span));
                continue;
            }

            if canonical_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_canonical(type_name, alias, field.span));
            }

            if method_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_method(type_name, alias, field.span));
            }
            if builtin_member_names.contains(alias.as_str()) {
                self.errors
                    .push(errors::alias_collides_with_builtin(type_name, alias, field.span));
            }

            if let Some(prev_span) = seen_aliases.get(alias) {
                self.errors
                    .push(errors::duplicate_alias(type_name, alias, *prev_span, field.span));
            } else {
                seen_aliases.insert(alias.clone(), field.span);
            }
        }
    }
    /// Render a named method signature for compatibility diagnostics.
    fn method_sig_string_named(&self, method_name: &str, m: &MethodInfo) -> String {
        let recv = match m.receiver {
            Some(Receiver::Mutable) => "mut self",
            Some(Receiver::Immutable) => "self",
            None => "",
        };
        let mut parts: Vec<String> = Vec::new();
        if !recv.is_empty() {
            parts.push(recv.to_string());
        }
        for param in &m.params {
            if let Some(name) = param.name() {
                parts.push(format!("{name}: {}", param.ty));
            }
        }
        let async_kw = if m.is_async { "async " } else { "" };
        format!(
            "{async_kw}def {name}({params}) -> {ret}",
            name = method_name,
            params = parts.join(", "),
            ret = m.return_type
        )
    }

    /// Return whether two method signatures are compatible.
    pub(in crate::frontend::typechecker) fn method_sigs_compatible(
        &self,
        expected: &MethodInfo,
        found: &MethodInfo,
    ) -> bool {
        if expected.receiver != found.receiver {
            return false;
        }
        if expected.is_async != found.is_async {
            return false;
        }
        if expected.params.len() != found.params.len() {
            return false;
        }
        for (expected_param, found_param) in expected.params.iter().zip(found.params.iter()) {
            if expected_param.kind != found_param.kind {
                return false;
            }
            if !self.types_compatible(&expected_param.ty, &found_param.ty) {
                return false;
            }
        }
        self.types_compatible(&expected.return_type, &found.return_type)
    }

    /// Return whether two methods have the same call-time parameter shape, ignoring return type.
    fn method_call_shapes_same(&self, left: &MethodInfo, right: &MethodInfo) -> bool {
        if left.receiver != right.receiver {
            return false;
        }
        if left.is_async != right.is_async {
            return false;
        }
        if left.params.len() != right.params.len() {
            return false;
        }
        left.params
            .iter()
            .zip(right.params.iter())
            .all(|(left_param, right_param)| left_param.kind == right_param.kind && left_param.ty == right_param.ty)
    }

    /// True if `ancestor` appears in the transitive supertrait closure of trait `descendant` (RFC 042).
    fn is_strict_supertrait_name(&self, ancestor: &str, descendant: &str) -> bool {
        self.semantic_supertrait_closure(descendant)
            .iter()
            .any(|(name, _)| name == ancestor)
    }

    /// Drop supertrait obligations shadowed by a more derived trait in the same obligation group.
    fn filter_supertrait_dominated_entries(&self, entries: Vec<(String, MethodInfo)>) -> Vec<(String, MethodInfo)> {
        let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        entries
            .into_iter()
            .filter(|(ta, _)| {
                !names
                    .iter()
                    .any(|tb| ta != tb && self.is_strict_supertrait_name(ta, tb))
            })
            .collect()
    }

    /// Drop supertrait property obligations shadowed by a more derived trait in the same obligation group.
    fn filter_supertrait_dominated_property_entries(
        &self,
        entries: Vec<(String, PropertyInfo)>,
    ) -> Vec<(String, PropertyInfo)> {
        let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        entries
            .into_iter()
            .filter(|(ta, _)| {
                !names
                    .iter()
                    .any(|tb| ta != tb && self.is_strict_supertrait_name(ta, tb))
            })
            .collect()
    }

    /// Apply resolved type arguments to one trait definition so conformance uses the instantiated contract.
    fn instantiate_trait_info(&self, trait_info: &TraitInfo, args: &[ResolvedType]) -> TraitInfo {
        let subst = type_param_subst_map(&trait_info.type_params, args);
        TraitInfo {
            type_params: trait_info.type_params.clone(),
            supertraits: trait_info
                .supertraits
                .iter()
                .map(|(name, super_args)| {
                    (
                        name.clone(),
                        super_args
                            .iter()
                            .map(|arg| substitute_resolved_type(arg, &subst))
                            .collect(),
                    )
                })
                .collect(),
            methods: trait_info
                .methods
                .iter()
                .map(|(name, info)| (name.clone(), substitute_method_info(info, &subst)))
                .collect(),
            method_aliases: trait_info.method_aliases.clone(),
            properties: trait_info
                .properties
                .iter()
                .map(|(name, info)| (name.clone(), substitute_property_info(info, &subst)))
                .collect(),
            requires: trait_info
                .requires
                .iter()
                .map(|(field, ty)| (field.clone(), substitute_resolved_type(ty, &subst)))
                .collect(),
        }
    }

    /// Resolve one model/class trait adoption and instantiate generic trait parameters from explicit `with Trait[T]`.
    fn resolve_adopted_trait_info(
        &mut self,
        bound: &TraitBound,
        adoption_span: Span,
    ) -> Option<(TraitInfo, Vec<ResolvedType>)> {
        let trait_name = bound.name.as_str();
        let trait_info = self.lookup_trait_info(trait_name)?.clone();
        if bound.type_args.is_empty() {
            return Some((trait_info, Vec::new()));
        }

        let resolved_args: Vec<ResolvedType> = bound
            .type_args
            .iter()
            .map(|arg| self.resolve_type_checked(arg))
            .collect();
        if resolved_args.len() != trait_info.type_params.len() {
            self.errors.push(errors::trait_adoption_bound_arity_mismatch(
                trait_name,
                trait_info.type_params.len(),
                resolved_args.len(),
                adoption_span,
            ));
            return None;
        }

        let instantiated = self.instantiate_trait_info(&trait_info, &resolved_args);
        Some((instantiated, resolved_args))
    }

    /// Resolve a previously collected trait adoption into instantiated trait metadata for validation.
    fn resolve_collected_trait_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        adoption_span: Span,
    ) -> Option<ResolvedTraitAdoption> {
        let trait_info = self.lookup_trait_info(&adoption.name)?.clone();
        if adoption.type_args.len() != trait_info.type_params.len() {
            self.errors.push(errors::trait_adoption_bound_arity_mismatch(
                &adoption.name,
                trait_info.type_params.len(),
                adoption.type_args.len(),
                adoption_span,
            ));
            return None;
        }
        let info = if adoption.type_args.is_empty() {
            trait_info
        } else {
            self.instantiate_trait_info(&trait_info, &adoption.type_args)
        };
        Some(ResolvedTraitAdoption {
            name: adoption.name.clone(),
            info,
            args: adoption.type_args.clone(),
            module_path: adoption.module_path.clone(),
            span: adoption_span,
        })
    }

    /// Return the first `@derive` span for diagnostics tied to synthetic derive adoptions.
    fn first_derive_span(decorators: &[Spanned<Decorator>]) -> Span {
        decorators
            .iter()
            .find(|decorator| decorators::from_str(decorator.node.name.as_str()) == Some(DecoratorId::Derive))
            .map(|decorator| decorator.span)
            .unwrap_or_default()
    }

    /// Recursively collect abstract trait methods after applying any explicit adoption-time type arguments.
    fn collect_instantiated_trait_abstract_method_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, String, MethodInfo)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (method_name, method_info) in &trait_info.methods {
            if !method_info.has_body {
                out.push((method_name.clone(), trait_name.to_string(), method_info.clone()));
            }
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_abstract_method_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                seen,
                out,
            );
        }
    }

    /// Recursively collect abstract trait properties after applying adoption-time type arguments.
    fn collect_instantiated_trait_abstract_property_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, String, PropertyInfo)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (property_name, property_info) in &trait_info.properties {
            if !property_info.has_body {
                out.push((property_name.clone(), trait_name.to_string(), property_info.clone()));
            }
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_abstract_property_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                seen,
                out,
            );
        }
    }

    /// Collect all methods from an instantiated trait and its supertraits with type arguments applied.
    fn collect_instantiated_trait_method_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        origin_module_path: Option<&[String]>,
        seen: &mut HashSet<String>,
        out: &mut Vec<TraitMethodEntry>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (method_name, method_info) in &trait_info.methods {
            out.push(TraitMethodEntry {
                method_name: method_name.clone(),
                origin_trait: trait_name.to_string(),
                origin_type_args: trait_args.to_vec(),
                origin_module_path: origin_module_path.map(<[String]>::to_vec),
                info: method_info.clone(),
            });
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_method_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                None,
                seen,
                out,
            );
        }
    }

    /// Collect all properties from an instantiated trait and its supertraits with type arguments applied.
    fn collect_instantiated_trait_property_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
        seen: &mut HashSet<String>,
        out: &mut Vec<(String, String, PropertyInfo)>,
    ) {
        let key = format!(
            "{trait_name}<{}>",
            trait_args
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        if !seen.insert(key) {
            return;
        }

        for (property_name, property_info) in &trait_info.properties {
            out.push((property_name.clone(), trait_name.to_string(), property_info.clone()));
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_property_entries(
                supertrait_name,
                &instantiated,
                supertrait_args,
                seen,
                out,
            );
        }
    }

    /// Return all trait-backed method entries required by one resolved adoption.
    fn trait_method_entries_for_adoption(&self, adoption: &ResolvedTraitAdoption) -> Vec<TraitMethodEntry> {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        self.collect_instantiated_trait_method_entries(
            &adoption.name,
            &adoption.info,
            &adoption.args,
            adoption.module_path.as_deref(),
            &mut seen,
            &mut entries,
        );
        entries
    }

    /// Collect abstract (`...`) methods from a trait and its transitive supertraits with supertrait type args applied.
    fn raw_trait_abstract_method_entries(
        &self,
        trait_name: &str,
        explicit_root: Option<(&TraitInfo, &[ResolvedType])>,
    ) -> Vec<(String, String, MethodInfo)> {
        if let Some((trait_info, trait_args)) = explicit_root {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            self.collect_instantiated_trait_abstract_method_entries(
                trait_name, trait_info, trait_args, &mut seen, &mut out,
            );
            return out;
        }

        let mut out = Vec::new();
        if let Some(root) = self.lookup_semantic_trait_info(trait_name) {
            for (m, info) in &root.methods {
                if !info.has_body {
                    out.push((m.clone(), trait_name.to_string(), info.clone()));
                }
            }
        }
        for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
            for (m, info) in &supertrait_info.methods {
                if !info.has_body {
                    out.push((m.clone(), supertrait_name.clone(), substitute_method_info(info, &subst)));
                }
            }
        }
        out
    }

    /// Resolve a trait method visible when a concrete type adopts `adopted_trait`, including methods from transitive
    /// supertraits with type arguments substituted per the supertrait closure (RFC 042).
    ///
    /// Supertrait shadowing matches [`Self::grouped_trait_abstract_method_obligations`]: along a refinement chain, a
    /// subtrait's declaration dominates its supertrait's same-named method for lookup purposes.
    ///
    /// When multiple origins remain after filtering (diamond shapes), signatures must be mutually compatible; otherwise
    /// a [`errors::trait_conflict`] diagnostic is recorded and `None` is returned.
    pub(in crate::frontend::typechecker) fn trait_method_info_resolved(
        &mut self,
        adopted_trait: &str,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<MethodInfo> {
        let mut entries: Vec<(String, MethodInfo)> = Vec::new();
        if let Some(root) = self.lookup_semantic_trait_info(adopted_trait)
            && let Some(info) = root.methods.get(method)
        {
            entries.push((adopted_trait.to_string(), info.clone()));
        }
        for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(adopted_trait) {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let Some(info) = supertrait_info.methods.get(method) else {
                continue;
            };
            let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
            entries.push((supertrait_name, substitute_method_info(info, &subst)));
        }
        let filtered = self.filter_supertrait_dominated_entries(entries);
        if filtered.is_empty()
            && let Some(segments) = stdlib::trait_method_module_segments(adopted_trait)
            && let Some(full_trait) = self.stdlib_cache.lookup_trait(&segments, adopted_trait)
            && let Some(info) = full_trait.methods.get(method)
        {
            return Some(info.clone());
        }
        match filtered.as_slice() {
            [] => None,
            [(_, info)] => Some(info.clone()),
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest
                    .iter()
                    .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
                if !all_mutually_compat {
                    self.errors
                        .push(errors::trait_conflict(&rest[0].0, &rest[1].0, method, ambiguity_span));
                    return None;
                }
                let all_identical =
                    !exp0.has_body && rest.iter().all(|(_, e)| !e.has_body && method_infos_identical(exp0, e));
                if !all_identical {
                    self.errors.push(errors::supertrait_method_ambiguity(
                        adopted_trait,
                        method,
                        &rest[0].0,
                        &rest[1].0,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(exp0.clone())
            }
        }
    }

    /// Resolve one method through a concrete trait adoption that may include generic trait arguments.
    pub(in crate::frontend::typechecker) fn trait_method_info_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<MethodInfo> {
        self.trait_method_entry_resolved_for_adoption(adoption, method, ambiguity_span)
            .map(|entry| entry.info)
    }

    /// Resolve one method entry through a concrete trait adoption, preserving the originating trait provenance.
    pub(in crate::frontend::typechecker) fn trait_method_entry_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<TraitMethodEntry> {
        if adoption.module_path.is_none() && adoption.type_args.is_empty() {
            return self
                .trait_method_info_resolved(&adoption.name, method, ambiguity_span)
                .map(|info| TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: adoption.name.clone(),
                    origin_type_args: Vec::new(),
                    origin_module_path: None,
                    info,
                });
        }
        let root = self
            .lookup_semantic_trait_info(&adoption.name)
            .cloned()
            .or_else(|| {
                adoption
                    .module_path
                    .as_ref()
                    .and_then(|module_path| self.stdlib_cache.lookup_trait(module_path, &adoption.name))
            })
            .or_else(|| {
                stdlib::trait_method_module_segments(&adoption.name)
                    .and_then(|module_path| self.stdlib_cache.lookup_trait(&module_path, &adoption.name))
            })?;
        let instantiated = if adoption.type_args.is_empty() {
            root
        } else {
            self.instantiate_trait_info(&root, &adoption.type_args)
        };
        let resolved = ResolvedTraitAdoption {
            name: adoption.name.clone(),
            info: instantiated,
            args: adoption.type_args.clone(),
            module_path: adoption.module_path.clone(),
            span: ambiguity_span,
        };
        let entries = self.trait_method_entries_for_adoption(&resolved);
        let matching: Vec<TraitMethodMatch> = entries
            .into_iter()
            .filter(|entry| entry.method_name == method)
            .map(|entry| {
                (
                    entry.origin_trait,
                    entry.origin_module_path,
                    entry.origin_type_args,
                    entry.info,
                )
            })
            .collect();
        let filtered = self.filter_supertrait_dominated_entries(
            matching
                .iter()
                .map(|(origin, _, _, info)| (origin.clone(), info.clone()))
                .collect(),
        );
        match filtered.as_slice() {
            [] => None,
            [(origin_trait, info)] => {
                let (origin_type_args, origin_module_path) = matching
                    .iter()
                    .find(|(origin, _, _, candidate)| {
                        origin == origin_trait && self.method_sigs_compatible(candidate, info)
                    })
                    .map(|(_, module_path, args, _)| (args.clone(), module_path.clone()))
                    .unwrap_or_default();
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: origin_trait.clone(),
                    origin_type_args,
                    origin_module_path,
                    info: info.clone(),
                })
            }
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest
                    .iter()
                    .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
                if !all_mutually_compat {
                    self.errors
                        .push(errors::trait_conflict(&rest[0].0, &rest[1].0, method, ambiguity_span));
                    return None;
                }
                let all_identical =
                    !exp0.has_body && rest.iter().all(|(_, e)| !e.has_body && method_infos_identical(exp0, e));
                if !all_identical {
                    self.errors.push(errors::supertrait_method_ambiguity(
                        &adoption.name,
                        method,
                        &rest[0].0,
                        &rest[1].0,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: rest[0].0.clone(),
                    origin_type_args: matching
                        .iter()
                        .find(|(origin, _, _, candidate)| {
                            origin == &rest[0].0 && self.method_sigs_compatible(candidate, exp0)
                        })
                        .map(|(_, _, args, _)| args.clone())
                        .unwrap_or_default(),
                    origin_module_path: matching
                        .iter()
                        .find(|(origin, _, _, candidate)| {
                            origin == &rest[0].0 && self.method_sigs_compatible(candidate, exp0)
                        })
                        .and_then(|(_, module_path, _, _)| module_path.clone()),
                    info: exp0.clone(),
                })
            }
        }
    }

    /// Resolve a trait property visible through an adopted trait, including transitive supertraits.
    pub(in crate::frontend::typechecker) fn trait_property_info_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        property: &str,
        ambiguity_span: Span,
    ) -> Option<PropertyInfo> {
        let root = self.lookup_semantic_trait_info(&adoption.name)?.clone();
        let root_info = if adoption.type_args.is_empty() {
            root
        } else {
            self.instantiate_trait_info(&root, &adoption.type_args)
        };
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        self.collect_instantiated_trait_property_entries(
            &adoption.name,
            &root_info,
            &adoption.type_args,
            &mut seen,
            &mut entries,
        );
        let matching: Vec<(String, PropertyInfo)> = entries
            .into_iter()
            .filter(|(name, _, _)| name == property)
            .map(|(_, origin, info)| (origin, info))
            .collect();
        let filtered = self.filter_supertrait_dominated_property_entries(matching);
        match filtered.as_slice() {
            [] => None,
            [(_, info)] => Some(info.clone()),
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest.iter().all(|(_, e)| {
                    self.types_compatible(&exp0.return_type, &e.return_type)
                        && self.types_compatible(&e.return_type, &exp0.return_type)
                });
                if !all_mutually_compat {
                    self.errors.push(errors::trait_property_conflict(
                        &rest[0].0,
                        &rest[1].0,
                        property,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(exp0.clone())
            }
        }
    }

    /// Group abstract (`...`) methods required by `trait_name` and its transitive supertraits by method name.
    ///
    /// Each group lists `(declaring_trait, signature)` after supertrait shadowing so diamonds can be merged or rejected
    /// consistently with [`Self::enforce_trait_abstract_methods`].
    fn grouped_trait_abstract_method_obligations(
        &self,
        trait_name: &str,
        explicit_root: Option<(&TraitInfo, &[ResolvedType])>,
    ) -> HashMap<String, Vec<(String, MethodInfo)>> {
        let raw = self.raw_trait_abstract_method_entries(trait_name, explicit_root);
        let mut map: HashMap<String, Vec<(String, MethodInfo)>> = HashMap::new();
        for (method, origin, info) in raw {
            map.entry(method).or_default().push((origin, info));
        }
        let mut out = HashMap::new();
        for (m, entries) in map {
            let filtered = self.filter_supertrait_dominated_entries(entries);
            if !filtered.is_empty() {
                out.insert(m, filtered);
            }
        }
        out
    }

    /// Group abstract (`...`) properties required by `trait_name` and its transitive supertraits.
    fn grouped_trait_abstract_property_obligations(
        &self,
        trait_name: &str,
        explicit_root: Option<(&TraitInfo, &[ResolvedType])>,
    ) -> HashMap<String, Vec<(String, PropertyInfo)>> {
        let raw = if let Some((trait_info, trait_args)) = explicit_root {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            self.collect_instantiated_trait_abstract_property_entries(
                trait_name, trait_info, trait_args, &mut seen, &mut out,
            );
            out
        } else {
            let mut out = Vec::new();
            if let Some(root) = self.lookup_semantic_trait_info(trait_name) {
                for (p, info) in &root.properties {
                    if !info.has_body {
                        out.push((p.clone(), trait_name.to_string(), info.clone()));
                    }
                }
            }
            for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
                let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                    continue;
                };
                let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
                for (p, info) in &supertrait_info.properties {
                    if !info.has_body {
                        out.push((
                            p.clone(),
                            supertrait_name.clone(),
                            substitute_property_info(info, &subst),
                        ));
                    }
                }
            }
            out
        };

        let mut map: HashMap<String, Vec<(String, PropertyInfo)>> = HashMap::new();
        for (property, origin, info) in raw {
            map.entry(property).or_default().push((origin, info));
        }
        let mut out = HashMap::new();
        for (property, entries) in map {
            let filtered = self.filter_supertrait_dominated_property_entries(entries);
            if !filtered.is_empty() {
                out.insert(property, filtered);
            }
        }
        out
    }

    /// Check one concrete property implementation against one trait property requirement.
    fn check_impl_against_trait_property_requirement(
        &mut self,
        type_name: &str,
        via_trait: &str,
        property_name: &str,
        property_info: &PropertyInfo,
        properties: &HashMap<String, PropertyInfo>,
        adoption_span: Span,
    ) {
        match properties.get(property_name) {
            None => self
                .errors
                .push(errors::missing_trait_property(via_trait, property_name, adoption_span)),
            Some(found) => {
                if !self.types_compatible(&found.return_type, &property_info.return_type) {
                    self.errors.push(errors::trait_property_signature_mismatch(
                        via_trait,
                        type_name,
                        property_name,
                        &property_info.return_type.to_string(),
                        &found.return_type.to_string(),
                        adoption_span,
                    ));
                }
            }
        }
    }

    /// Enforce abstract properties from `trait_name` and its supertraits on a concrete type.
    fn enforce_trait_abstract_properties(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        properties: &HashMap<String, PropertyInfo>,
    ) {
        let grouped =
            self.grouped_trait_abstract_property_obligations(trait_name, trait_args.map(|args| (trait_info, args)));
        let mut property_names: Vec<String> = grouped.keys().cloned().collect();
        property_names.sort();
        for property_name in property_names {
            let Some(group) = grouped.get(&property_name) else {
                continue;
            };
            let exp0 = &group[0].1;
            if group.len() == 1 {
                self.check_impl_against_trait_property_requirement(
                    type_name,
                    &group[0].0,
                    property_name.as_str(),
                    exp0,
                    properties,
                    adoption_span,
                );
                continue;
            }
            let all_mutually_compat = group.iter().all(|(_, e)| {
                self.types_compatible(&exp0.return_type, &e.return_type)
                    && self.types_compatible(&e.return_type, &exp0.return_type)
            });
            if !all_mutually_compat {
                self.errors.push(errors::trait_property_conflict(
                    &group[0].0,
                    &group[1].0,
                    property_name.as_str(),
                    adoption_span,
                ));
                continue;
            }
            let all_identical = group.iter().all(|(_, e)| property_infos_identical(exp0, e));
            if all_identical {
                self.check_impl_against_trait_property_requirement(
                    type_name,
                    &group[0].0,
                    property_name.as_str(),
                    exp0,
                    properties,
                    adoption_span,
                );
                continue;
            }
            if properties.get(property_name.as_str()).is_some_and(|found| {
                group
                    .iter()
                    .all(|(_, e)| self.types_compatible(&found.return_type, &e.return_type))
            }) {
                continue;
            }
            self.errors.push(errors::supertrait_property_ambiguity(
                trait_name,
                property_name.as_str(),
                &group[0].0,
                &group[1].0,
                adoption_span,
            ));
        }
    }

    /// Check that `methods` on a concrete type satisfy one abstract requirement from the trait graph.
    ///
    /// `via_trait` is the trait that originated the obligation (for diagnostics). Skips requirements that already have
    /// a default body on the trait (`has_body`).
    fn check_impl_against_trait_method_requirement(
        &mut self,
        type_name: &str,
        via_trait: &str,
        method_name: &str,
        method_info: &MethodInfo,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        adoption_span: Span,
    ) {
        if method_info.has_body {
            return;
        }
        match method_overloads.get(method_name) {
            None => self
                .errors
                .push(errors::missing_trait_method(via_trait, method_name, adoption_span)),
            Some(found_group) => {
                if !found_group.iter().any(|found| {
                    Self::method_trait_target_matches(found, via_trait)
                        && self.method_sigs_compatible(method_info, found)
                }) {
                    let expected_sig = self.method_sig_string_named(method_name, method_info);
                    let found_sig = found_group
                        .first()
                        .map(|found| self.method_sig_string_named(method_name, found))
                        .unwrap_or_else(|| "<missing>".to_string());
                    self.errors.push(errors::trait_method_signature_mismatch(
                        via_trait,
                        type_name,
                        method_name,
                        &expected_sig,
                        &found_sig,
                        adoption_span,
                    ));
                }
            }
        }
    }

    /// Enforce abstract methods from `trait_name` and its supertraits on a concrete type's method map (RFC 042).
    fn enforce_trait_abstract_methods(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let grouped =
            self.grouped_trait_abstract_method_obligations(trait_name, trait_args.map(|args| (trait_info, args)));
        let mut method_names: Vec<String> = grouped.keys().cloned().collect();
        method_names.sort();
        for method_name in method_names {
            let Some(group) = grouped.get(&method_name) else {
                continue;
            };
            if group.is_empty() {
                continue;
            }
            let exp0 = &group[0].1;
            if group.len() == 1 {
                self.check_impl_against_trait_method_requirement(
                    type_name,
                    &group[0].0,
                    method_name.as_str(),
                    exp0,
                    method_overloads,
                    adoption_span,
                );
                continue;
            }
            let all_mutually_compat = group
                .iter()
                .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
            if !all_mutually_compat {
                self.errors.push(errors::trait_conflict(
                    &group[0].0,
                    &group[1].0,
                    method_name.as_str(),
                    adoption_span,
                ));
                continue;
            }
            let all_identical = group.iter().all(|(_, e)| method_infos_identical(exp0, e));
            if all_identical {
                self.check_impl_against_trait_method_requirement(
                    type_name,
                    &group[0].0,
                    method_name.as_str(),
                    exp0,
                    method_overloads,
                    adoption_span,
                );
                continue;
            }
            let satisfies_all = method_overloads.get(method_name.as_str()).is_some_and(|found_group| {
                found_group
                    .iter()
                    .any(|found| group.iter().all(|(_, e)| self.method_sigs_compatible(e, found)))
            });
            if satisfies_all {
                continue;
            }
            if let Some(tm) = trait_info.methods.get(method_name.as_str())
                && tm.has_body
            {
                continue;
            }
            self.errors.push(errors::supertrait_method_ambiguity(
                trait_name,
                method_name.as_str(),
                &group[0].0,
                &group[1].0,
                adoption_span,
            ));
        }
    }

    /// If a concrete type overrides a trait default method, the override must remain signature-compatible.
    fn enforce_trait_default_method_overrides(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let adoption = ResolvedTraitAdoption {
            name: trait_name.to_string(),
            info: trait_info.clone(),
            args: trait_args.map(|args| args.to_vec()).unwrap_or_default(),
            module_path: None,
            span: adoption_span,
        };
        for entry in self.trait_method_entries_for_adoption(&adoption) {
            if !entry.info.has_body {
                continue;
            }
            let Some(found_group) = method_overloads.get(&entry.method_name) else {
                continue;
            };
            if !found_group
                .iter()
                .any(|found| self.method_sigs_compatible(&entry.info, found))
            {
                let expected_sig = self.method_sig_string_named(&entry.method_name, &entry.info);
                let found_sig = found_group
                    .first()
                    .map(|found| self.method_sig_string_named(&entry.method_name, found))
                    .unwrap_or_else(|| "<missing>".to_string());
                self.errors.push(errors::trait_method_signature_mismatch(
                    &entry.origin_trait,
                    type_name,
                    &entry.method_name,
                    &expected_sig,
                    &found_sig,
                    adoption_span,
                ));
            }
        }
    }

    /// If a trait method partial explicitly overrides an inherited trait method, its projected signature must remain
    /// compatible with the inherited surface it shadows.
    fn validate_trait_partial_inherited_overrides(
        &mut self,
        trait_name: &str,
        partials: &[Spanned<MethodPartialDecl>],
    ) {
        if partials.is_empty() {
            return;
        }
        let Some(trait_info) = self.lookup_trait_info(trait_name).cloned() else {
            return;
        };
        for partial in partials {
            let Some(found) = trait_info.methods.get(&partial.node.name).cloned() else {
                continue;
            };
            for (origin_trait, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
                let Some(supertrait_info) = self.lookup_semantic_trait_info(origin_trait.as_str()) else {
                    continue;
                };
                let Some(expected) = supertrait_info.methods.get(&partial.node.name) else {
                    continue;
                };
                let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
                let expected = substitute_method_info(expected, &subst);
                if self.method_sigs_compatible(&expected, &found) {
                    continue;
                }
                let expected_sig = self.method_sig_string_named(&partial.node.name, &expected);
                let found_sig = self.method_sig_string_named(&partial.node.name, &found);
                self.errors.push(errors::trait_method_signature_mismatch(
                    &origin_trait,
                    trait_name,
                    &partial.node.name,
                    &expected_sig,
                    &found_sig,
                    partial.span,
                ));
            }
        }
    }

    /// Render trait type arguments into a stable diagnostic and duplicate-detection key.
    fn trait_args_key(args: &[ResolvedType]) -> String {
        args.iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Reject identical generic trait instantiations adopted more than once by one type.
    fn validate_duplicate_trait_instantiations(&mut self, adoptions: &[ResolvedTraitAdoption]) {
        let mut seen: HashMap<(String, String), Span> = HashMap::new();
        for adoption in adoptions {
            let args_key = Self::trait_args_key(&adoption.args);
            let key = (adoption.name.clone(), args_key.clone());
            if seen.insert(key, adoption.span).is_some() {
                self.errors.push(errors::duplicate_trait_instantiation(
                    &adoption.name,
                    &args_key,
                    adoption.span,
                ));
            }
        }
    }

    /// Reject same-name method obligations that come from unrelated adopted trait families.
    fn validate_cross_trait_method_collisions(
        &mut self,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let mut by_method: HashMap<String, Vec<(String, MethodInfo, Span)>> = HashMap::new();
        for adoption in adoptions {
            for entry in self.trait_method_entries_for_adoption(adoption) {
                by_method
                    .entry(entry.method_name)
                    .or_default()
                    .push((entry.origin_trait, entry.info, adoption.span));
            }
        }
        for (method, entries) in by_method {
            let filtered_entries = self.filter_supertrait_dominated_entries(
                entries
                    .iter()
                    .map(|(origin, info, _)| (origin.clone(), info.clone()))
                    .collect(),
            );
            let mut origins: Vec<(String, Span)> = Vec::new();
            for (origin, _) in filtered_entries {
                if origins.iter().any(|(existing, _)| existing == &origin) {
                    continue;
                }
                let span = entries
                    .iter()
                    .find(|(entry_origin, _, _)| entry_origin == &origin)
                    .map(|(_, _, span)| *span)
                    .unwrap_or_default();
                origins.push((origin, span));
            }
            if origins.len() > 1 {
                if let Some(overloads) = method_overloads.get(&method)
                    && origins.iter().all(|(origin, _)| {
                        overloads.iter().any(|method_info| {
                            method_info
                                .trait_target
                                .as_ref()
                                .is_some_and(|target| target.name == *origin)
                        })
                    })
                {
                    continue;
                }
                let (left, span) = &origins[0];
                let (right, _) = &origins[1];
                self.errors
                    .push(errors::cross_trait_method_collision(left, right, &method, *span));
            }
        }
    }

    /// Return whether a collected method may satisfy the named trait target.
    fn method_trait_target_matches(method_info: &MethodInfo, trait_name: &str) -> bool {
        method_info
            .trait_target
            .as_ref()
            .is_none_or(|target| target.name == trait_name)
    }

    /// Group source spans by method name so overload diagnostics can point at the matching declaration.
    fn method_decl_spans_by_name(methods: &[Spanned<MethodDecl>]) -> HashMap<String, Vec<Span>> {
        let mut spans: HashMap<String, Vec<Span>> = HashMap::new();
        for method in methods {
            spans.entry(method.node.name.clone()).or_default().push(method.span);
        }
        spans
    }

    /// Ensure overloads that differ only by return type map to same-family trait obligations.
    fn validate_overloaded_methods_are_trait_backed(
        &mut self,
        type_name: &str,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        method_spans: &HashMap<String, Vec<Span>>,
    ) {
        let mut obligations_by_method: HashMap<String, Vec<(String, MethodInfo)>> = HashMap::new();
        for adoption in adoptions {
            for entry in self.trait_method_entries_for_adoption(adoption) {
                obligations_by_method
                    .entry(entry.method_name)
                    .or_default()
                    .push((entry.origin_trait, entry.info));
            }
        }

        for (method_name, overloads) in method_overloads {
            if overloads.len() <= 1 {
                continue;
            }
            let obligations = obligations_by_method.get(method_name).map(Vec::as_slice).unwrap_or(&[]);
            let mut matched_obligations = vec![false; obligations.len()];
            let mut overload_matches: Vec<Option<usize>> = vec![None; overloads.len()];
            for (idx, overload) in overloads.iter().enumerate() {
                let matched_index = obligations
                    .iter()
                    .enumerate()
                    .find(|(obligation_idx, (origin_trait, expected))| {
                        !matched_obligations[*obligation_idx]
                            && Self::method_trait_target_matches(overload, origin_trait)
                            && self.method_sigs_compatible(expected, overload)
                    })
                    .map(|(obligation_idx, _)| obligation_idx);
                if let Some(obligation_idx) = matched_index {
                    matched_obligations[obligation_idx] = true;
                    overload_matches[idx] = Some(obligation_idx);
                }
            }
            let has_trait_backed_overload = overload_matches.iter().any(Option::is_some);
            for (idx, overload) in overloads.iter().enumerate() {
                if overload_matches[idx].is_some() {
                    continue;
                }
                let shares_call_shape = overloads
                    .iter()
                    .enumerate()
                    .any(|(other_idx, other)| other_idx != idx && self.method_call_shapes_same(overload, other));
                if has_trait_backed_overload && !shares_call_shape {
                    continue;
                }
                let span = method_spans
                    .get(method_name)
                    .and_then(|spans| spans.get(idx).copied())
                    .or_else(|| method_spans.get(method_name).and_then(|spans| spans.first().copied()))
                    .unwrap_or_default();
                self.errors
                    .push(errors::duplicate_method_not_trait_backed(type_name, method_name, span));
            }
        }
    }

    /// Run RFC 025 validation for generic trait adoptions and same-name concrete methods.
    fn validate_multi_instantiation_trait_surface(
        &mut self,
        type_name: &str,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        method_spans: &HashMap<String, Vec<Span>>,
    ) {
        self.validate_duplicate_trait_instantiations(adoptions);
        self.validate_cross_trait_method_collisions(adoptions, method_overloads);
        self.validate_overloaded_methods_are_trait_backed(type_name, adoptions, method_overloads, method_spans);
    }

    /// Validate that explicit `Awaitable[T]` adoptions have a compiler-known await realization.
    ///
    /// User-authored wrapper types may satisfy `Awaitable[T]` by containing a field whose type is itself awaitable and
    /// whose output type is compatible with `T`. Rust-backed future types and stdlib task handles are handled by the
    /// ordinary await-realization path outside this declaration check.
    fn validate_awaitable_adoptions(
        &mut self,
        type_name: &str,
        adoptions: &[ResolvedTraitAdoption],
        fields: &[(&str, ResolvedType)],
        type_param_bounds: HashMap<String, Vec<TypeBoundInfo>>,
    ) {
        let awaitable_name = builtin_traits::as_str(TraitId::Awaitable);
        if !adoptions.iter().any(|adoption| adoption.name == awaitable_name) {
            return;
        }

        self.current_type_param_bound_details.push(type_param_bounds);
        for adoption in adoptions.iter().filter(|adoption| adoption.name == awaitable_name) {
            let Some(expected_output) = adoption.args.first() else {
                continue;
            };
            let realization_field = fields.iter().find_map(|(field_name, field_ty)| {
                self.await_output_type_from_type(field_ty).and_then(|actual_output| {
                    (self.types_compatible(&actual_output, expected_output)
                        || self.types_compatible(expected_output, &actual_output))
                    .then(|| (*field_name).to_string())
                })
            });
            if let Some(field_name) = realization_field {
                self.type_info
                    .expressions
                    .awaitable_delegation_fields
                    .insert(type_name.to_string(), field_name);
            } else {
                self.errors.push(errors::invalid_awaitable_adoption(
                    type_name,
                    &expected_output.to_string(),
                    adoption.span,
                ));
            }
        }
        self.current_type_param_bound_details.pop();
    }

    // ========================================================================
    // Second pass: check declarations
    // ========================================================================

    /// Validate a declaration's body and semantics (second pass).
    ///
    /// Dispatches to `check_model`, `check_class`, etc. Expects symbols to
    /// already be registered via [`collect_declaration`](Self::collect_declaration).
    pub(crate) fn check_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(_) => {} // Already handled
            Declaration::Const(konst) => self.check_const(konst, decl.span),
            Declaration::Static(static_decl) => self.check_static(static_decl, decl.span),
            Declaration::Model(model) => self.check_model(model),
            Declaration::Class(class) => self.check_class(class),
            Declaration::Trait(tr) => self.check_trait(tr),
            Declaration::Alias(_) => {} // Alias targets are validated during collection.
            Declaration::Partial(partial) => self.check_partial_decl(partial),
            Declaration::TypeAlias(_) => {} // Type aliases are transparent; no body to check
            Declaration::Newtype(nt) => self.check_newtype(nt),
            Declaration::Enum(en) => self.check_enum(en),
            Declaration::Function(func) => self.check_function(func),
            Declaration::TestModule(test_module) => self.check_test_module(test_module),
            Declaration::Docstring(_) => {} // Docstrings don't need checking
        }
    }

    /// Validate preset expressions for a collected top-level partial declaration.
    fn check_partial_decl(&mut self, partial: &PartialDecl) {
        let Some(sym) = self.lookup_symbol(&partial.name).cloned() else {
            for arg in &partial.args {
                self.validate_top_level_partial_preset(arg);
                self.check_expr(&arg.value);
            }
            return;
        };
        let SymbolKind::Function(info) = sym.kind else {
            for arg in &partial.args {
                self.validate_top_level_partial_preset(arg);
                self.check_expr(&arg.value);
            }
            return;
        };
        for arg in &partial.args {
            self.validate_top_level_partial_preset(arg);
            let actual = self.check_expr(&arg.value);
            if let Some(param) = info.params.iter().find(|param| param.name() == Some(arg.name.as_str()))
                && !self.types_compatible(&actual, &param.ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param.ty.to_string(),
                    &actual.to_string(),
                    arg.value.span,
                ));
            }
        }
    }

    /// Emit the RFC 084 declaration-safety diagnostic for one module-level partial preset.
    fn validate_top_level_partial_preset(&mut self, arg: &PartialArg) {
        if !self.is_declaration_safe_partial_preset(&arg.value) {
            self.errors
                .push(errors::unsafe_top_level_partial_preset(&arg.name, arg.value.span));
        }
    }

    /// Return whether a module-level partial preset can be represented without executing user code.
    fn is_declaration_safe_partial_preset(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Literal(_) => true,
            Expr::Ident(_) | Expr::Field(_, _) => self.is_declaration_safe_const_or_variant_path(expr),
            Expr::Paren(inner) => self.is_declaration_safe_partial_preset(inner),
            Expr::List(entries) => entries.iter().all(|entry| match entry {
                ListEntry::Element(value) => self.is_declaration_safe_partial_preset(value),
                ListEntry::Spread(_) => false,
            }),
            Expr::Dict(entries) => entries.iter().all(|entry| match entry {
                DictEntry::Pair(key, value) => {
                    self.is_declaration_safe_partial_preset(key) && self.is_declaration_safe_partial_preset(value)
                }
                DictEntry::Spread(_) => false,
            }),
            Expr::Call(callee, type_args, args) => {
                type_args.is_empty()
                    && self.is_model_literal_callee(callee)
                    && args.iter().all(|arg| match arg {
                        CallArg::Named(_, value) => self.is_declaration_safe_partial_preset(value),
                        CallArg::Positional(_) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
                    })
            }
            Expr::Constructor(name, args) => {
                self.is_model_literal_name(name)
                    && args.iter().all(|arg| match arg {
                        CallArg::Named(_, value) => self.is_declaration_safe_partial_preset(value),
                        CallArg::Positional(_) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
                    })
            }
            _ => false,
        }
    }

    /// Return whether a call-like preset target names a model literal constructor.
    fn is_model_literal_callee(&self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => self.is_model_literal_name(name),
            _ => false,
        }
    }

    /// Return whether a name resolves to a model type that can be serialized as preset metadata.
    fn is_model_literal_name(&self, name: &str) -> bool {
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Model(_))))
    }

    /// Return whether a top-level partial preset path names a const or a zero-argument enum variant.
    fn is_declaration_safe_const_or_variant_path(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(name) => {
                self.const_decls.contains_key(name)
                    || self
                        .lookup_symbol(name)
                        .is_some_and(|sym| matches!(sym.kind, SymbolKind::Variant(_)))
            }
            Expr::Field(base, member) => {
                if let Expr::Ident(type_name) = &base.node
                    && let Some(symbol) = self.lookup_symbol(type_name)
                    && let SymbolKind::Type(TypeInfo::Enum(info)) = &symbol.kind
                {
                    return info.variants.iter().any(|variant| variant == member)
                        || info.variant_aliases.contains_key(member);
                }
                false
            }
            _ => false,
        }
    }

    /// Validate preset expressions for same-type method partial declarations.
    fn check_method_partials(&mut self, owner: &str, partials: &[Spanned<MethodPartialDecl>]) {
        for partial in partials {
            let Some(target) = self.method_info_for_owner(owner, &partial.node.target).cloned() else {
                for arg in &partial.node.args {
                    self.check_expr(&arg.value);
                }
                continue;
            };

            for arg in &partial.node.args {
                let expected = target
                    .params
                    .iter()
                    .find(|param| param.name() == Some(arg.name.as_str()))
                    .map(|param| param.ty.clone());
                let actual = match expected.as_ref() {
                    Some(expected_ty) => self.check_expr_with_expected(&arg.value, Some(expected_ty)),
                    None => self.check_expr(&arg.value),
                };
                if let Some(expected_ty) = expected
                    && !self.types_compatible(&actual, &expected_ty)
                {
                    self.errors.push(errors::type_mismatch(
                        &expected_ty.to_string(),
                        &actual.to_string(),
                        arg.value.span,
                    ));
                }
            }
        }
    }

    /// Return collected method metadata for an owner type or trait surface.
    fn method_info_for_owner(&self, owner: &str, method: &str) -> Option<&MethodInfo> {
        if let Some(info) = self.lookup_trait_info(owner) {
            return info.methods.get(method);
        }
        match self.lookup_type_info(owner)? {
            TypeInfo::Model(info) => info.methods.get(method),
            TypeInfo::Class(info) => info.methods.get(method),
            TypeInfo::Newtype(info) => info.methods.get(method),
            TypeInfo::Enum(info) => info.methods.get(method),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Typecheck an inline test module declaration.
    fn check_test_module(&mut self, test_module: &TestModuleDecl) {
        self.symbols.enter_scope(ScopeKind::Block);
        for decl in &test_module.body {
            self.collect_declaration(decl);
        }
        for decl in &test_module.body {
            self.check_declaration(decl);
        }
        self.symbols.exit_scope();
    }

    /// Check a const declaration, routing RFC 024 `__derives__` metadata through derive-specific validation.
    fn check_const(&mut self, konst: &ConstDecl, span: Span) {
        if konst.name == "__derives__" {
            self.check_derives_metadata(konst);
            return;
        }
        // RFC 008: const-eval (with cycle detection + category classification).
        self.check_and_resolve_const(konst, span);
    }

    /// Validate RFC 024 module-level `__derives__` metadata against the module's local trait declarations.
    fn check_derives_metadata(&mut self, konst: &ConstDecl) {
        let Expr::List(entries) = &konst.value.node else {
            self.errors.push(CompileError::type_error(
                "`__derives__` must be a list of trait names".to_string(),
                konst.value.span,
            ));
            return;
        };
        if entries.is_empty() {
            self.errors.push(CompileError::type_error(
                "`__derives__` must list at least one trait".to_string(),
                konst.value.span,
            ));
            return;
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            let crate::frontend::ast::ListEntry::Element(expr) = entry else {
                self.errors.push(CompileError::type_error(
                    "`__derives__` entries must be trait names, not spreads".to_string(),
                    konst.value.span,
                ));
                continue;
            };
            let Expr::Ident(name) = &expr.node else {
                self.errors.push(CompileError::type_error(
                    "`__derives__` entries must be trait names".to_string(),
                    expr.span,
                ));
                continue;
            };
            if !seen.insert(name.clone()) {
                self.errors.push(CompileError::type_error(
                    format!("Duplicate trait '{name}' in `__derives__`"),
                    expr.span,
                ));
            }
            if self.lookup_trait_info(name).is_none() {
                self.errors.push(CompileError::type_error(
                    format!("`__derives__` entry '{name}' is not a trait"),
                    expr.span,
                ));
            }
        }
    }

    /// Validate a module static declaration and record its final type for later lowering.
    fn check_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let expected_ty = self.resolve_type_checked(&static_decl.ty);
        let value_ty = self.check_expr_with_expected(&static_decl.value, Some(&expected_ty));
        if !self.types_compatible(&value_ty, &expected_ty)
            && !self.record_validated_newtype_coercion_if_possible(&value_ty, &expected_ty, static_decl.value.span)
        {
            self.errors.push(errors::type_mismatch(
                &expected_ty.to_string(),
                &value_ty.to_string(),
                static_decl.value.span,
            ));
        }

        if let Some(symbol_id) = self.symbols.lookup_local(&static_decl.name)
            && let Some(symbol) = self.symbols.get_mut(symbol_id)
            && let SymbolKind::Static(info) = &mut symbol.kind
        {
            info.ty = expected_ty.clone();
        }

        if let Some(binding) = self.type_info.declarations.static_bindings.get_mut(&static_decl.name) {
            binding.is_imported = false;
        } else {
            self.type_info.declarations.static_bindings.insert(
                static_decl.name.clone(),
                super::StaticBindingInfo { is_imported: false },
            );
        }

        let _ = span;
    }

    /// Validate a model declaration after collection, including decorators, trait conformance, fields, and methods.
    fn check_model(&mut self, model: &ModelDecl) {
        self.symbols.enter_scope(ScopeKind::Model);

        self.validate_decorators_rejecting_user_defined(&model.decorators, "model");
        // Validate @derive decorators
        self.validate_derives(&model.decorators);
        self.validate_rust_derives(&model.decorators, "model", false, &model.traits);
        let derives = self.extract_derive_names(&model.decorators);
        let has_validate = derives
            .iter()
            .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));

        // Define type parameters
        for param in &model.type_params {
            self.symbols.define(Symbol {
                name: param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin), // Type var placeholder
                span: Span::default(),
                scope: 0,
            });
        }

        // Check traits exist and are satisfied (models can adopt storage-free traits, RFC 000).
        // Note: do this after defining type params so `@requires(field: T)` can resolve `T`.
        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &model.traits {
            let trait_name = trait_ref.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let Some((trait_info, trait_args)) = self.resolve_adopted_trait_info(&trait_ref.node, trait_ref.span)
                else {
                    continue;
                };
                resolved_trait_adoptions.push(ResolvedTraitAdoption {
                    name: trait_name.to_string(),
                    info: trait_info.clone(),
                    args: trait_args.clone(),
                    module_path: None,
                    span: trait_ref.span,
                });
                self.check_trait_conformance_model(
                    model,
                    trait_info,
                    trait_name,
                    if trait_args.is_empty() {
                        None
                    } else {
                        Some(trait_args.as_slice())
                    },
                    trait_ref.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }
        let derive_adoption_span = Self::first_derive_span(&model.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(&derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        let model_fields: Vec<_> = model
            .fields
            .iter()
            .map(|field| (field.node.name.as_str(), self.resolve_type_checked(&field.node.ty)))
            .collect();
        let model_type_param_bounds = self.type_param_bound_details_from_type_params(&model.type_params);
        self.validate_awaitable_adoptions(
            &model.name,
            &resolved_trait_adoptions,
            &model_fields,
            model_type_param_bounds,
        );

        let mut method_names = HashSet::new();
        if let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) {
            method_names.extend(info.methods.keys().cloned());
        }
        method_names.extend(self.collect_trait_method_names(&model.traits));
        self.validate_field_metadata(&model.name, &model.fields, &method_names);
        if let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&model.methods);
            self.validate_multi_instantiation_trait_surface(
                &model.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }

        // Define fields in scope
        for field in &model.fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.validate_direct_recursive_model_field(&model.name, &ty, field.span);
            self.symbols.define(Symbol {
                name: field.node.name.clone(),
                kind: SymbolKind::Field(FieldInfo {
                    ty,
                    visibility: field.node.visibility,
                    owner: Some(model.name.clone()),
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                }),
                span: field.span,
                scope: 0,
            });

            // Check default expression type
            if let Some(default) = &field.node.default {
                let field_ty = self.resolve_type_checked(&field.node.ty);
                let default_ty = self.check_expr_with_expected(default, Some(&field_ty));
                if !self.types_compatible(&default_ty, &field_ty)
                    && !self.record_validated_newtype_coercion_if_possible(&default_ty, &field_ty, default.span)
                {
                    self.errors.push(errors::type_mismatch(
                        &field_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
            }
        }

        // Check methods
        for method in &model.methods {
            self.check_method_with_owner_type_params(&method.node, &model.name, &model.type_params);
        }
        self.check_method_partials(&model.name, &model.method_partials);
        for property in &model.properties {
            self.check_property(&property.node, &model.name);
        }

        if has_validate {
            self.check_validate_derive_model(model);
        }

        self.symbols.exit_scope();
    }

    /// Reject model fields whose resolved type contains the model itself without an indirection boundary.
    fn validate_direct_recursive_model_field(&mut self, model_name: &str, field_ty: &ResolvedType, span: Span) {
        let mut visiting = HashSet::new();
        if self.type_contains_direct_recursive_model(field_ty, model_name, &mut visiting) {
            self.errors.push(CompileError::type_error(
                format!(
                    "Model '{model_name}' has a direct recursive field type '{field_ty}'. Use an indirection such as List[...] for recursive payloads."
                ),
                span,
            ));
        }
    }

    /// Return whether a type contains the target model through only inline Rust-layout positions.
    fn type_contains_direct_recursive_model(
        &self,
        ty: &ResolvedType,
        model_name: &str,
        visiting: &mut HashSet<String>,
    ) -> bool {
        match ty {
            ResolvedType::Named(name) => {
                self.nominal_type_contains_direct_recursive_model(name, &[], model_name, visiting)
            }
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => args
                .iter()
                .any(|arg| self.type_contains_direct_recursive_model(arg, model_name, visiting)),
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(
                    CollectionTypeId::List
                    | CollectionTypeId::Dict
                    | CollectionTypeId::Set
                    | CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::Generator,
                ) => false,
                Some(CollectionTypeId::Tuple | CollectionTypeId::Option | CollectionTypeId::Result) => args
                    .iter()
                    .any(|arg| self.type_contains_direct_recursive_model(arg, model_name, visiting)),
                None => self.nominal_type_contains_direct_recursive_model(name, args, model_name, visiting),
            },
            ResolvedType::Tuple(items) => items
                .iter()
                .any(|item| self.type_contains_direct_recursive_model(item, model_name, visiting)),
            ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::Function(_, _) => false,
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => false,
        }
    }

    /// Follow known nominal field types to find direct recursive model layouts.
    fn nominal_type_contains_direct_recursive_model(
        &self,
        type_name: &str,
        type_args: &[ResolvedType],
        model_name: &str,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if type_name == model_name {
            return true;
        }

        let visit_key = if type_args.is_empty() {
            type_name.to_string()
        } else {
            format!(
                "{}[{}]",
                type_name,
                type_args.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            )
        };
        if !visiting.insert(visit_key.clone()) {
            return false;
        }

        let result = match self.lookup_semantic_type_info(type_name) {
            Some(TypeInfo::Model(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                info.fields.values().any(|field| {
                    let field_ty = substitute_resolved_type(&field.ty, &subst);
                    let field_ty = self.expand_type_aliases(field_ty);
                    self.type_contains_direct_recursive_model(&field_ty, model_name, visiting)
                })
            }
            Some(TypeInfo::Class(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                info.fields.values().any(|field| {
                    let field_ty = substitute_resolved_type(&field.ty, &subst);
                    let field_ty = self.expand_type_aliases(field_ty);
                    self.type_contains_direct_recursive_model(&field_ty, model_name, visiting)
                })
            }
            Some(TypeInfo::Newtype(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                let underlying = substitute_resolved_type(&info.underlying, &subst);
                let underlying = self.expand_type_aliases(underlying);
                self.type_contains_direct_recursive_model(&underlying, model_name, visiting)
            }
            Some(TypeInfo::Enum(_) | TypeInfo::Builtin | TypeInfo::TypeAlias) | None => false,
        };

        visiting.remove(&visit_key);
        result
    }

    /// Validate a model declaration that derives validation support.
    fn check_validate_derive_model(&mut self, model: &ModelDecl) {
        // Validate that validate() exists and has the expected signature.
        let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) else {
            return;
        };

        let Some(validate) = info.methods.get("validate") else {
            self.errors.push(errors::validate_derive_missing_validate_method(
                &model.name,
                Span::default(),
            ));
            return;
        };

        let expected = "def validate(self) -> Result[Self, E]";
        let found_sig = self.method_sig_string_named("validate", validate);

        // Receiver must exist and be immutable.
        if validate.receiver != Some(Receiver::Immutable) || validate.is_async || !validate.params.is_empty() {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
            return;
        }

        // Return type must be Result[Self, E] (allow Result[ModelName, E] too).
        let ok_matches_self = |ok: &ResolvedType| {
            matches!(ok, ResolvedType::SelfType)
                || matches!(ok, ResolvedType::Named(n) if n == &model.name)
                || matches!(ok, ResolvedType::TypeVar(n) if n == &model.name)
        };

        if validate.return_type.is_result() {
            let ok_ty = validate
                .return_type
                .result_ok_type()
                .cloned()
                .unwrap_or(ResolvedType::Unknown);
            if !ok_matches_self(&ok_ty) {
                self.errors.push(errors::validate_derive_invalid_validate_signature(
                    &model.name,
                    expected,
                    &found_sig,
                    Span::default(),
                ));
            }
        } else {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
        }
    }
    /// Validate that a model satisfies required fields and methods for one instantiated trait adoption.
    fn check_trait_conformance_model(
        &mut self,
        model: &ModelDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        // Check required fields (including types)
        for (field_name, field_ty) in &trait_info.requires {
            let found = model.fields.iter().find(|f| &f.node.name == field_name);
            match found {
                None => {
                    self.errors
                        .push(errors::missing_field(&model.name, field_name, adoption_span));
                }
                Some(f) => {
                    let actual_ty = self.resolve_type_checked(&f.node.ty);
                    if !self.types_compatible(&actual_ty, field_ty) {
                        self.errors.push(errors::type_mismatch(
                            &field_ty.to_string(),
                            &actual_ty.to_string(),
                            f.node.ty.span,
                        ));
                    }
                }
            }
        }

        // Required methods: direct trait + transitive supertraits (RFC 042).
        let model_info = self
            .symbols
            .lookup(&model.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Model(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(mi) = model_info {
            self.enforce_trait_abstract_methods(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.properties,
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    let found = model.methods.iter().any(|m| &m.node.name == method_name);
                    if !found {
                        self.errors
                            .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                    }
                }
            }
        }
    }

    /// Validate a class declaration after collection, including inheritance, field metadata, traits, and methods.
    fn check_class(&mut self, class: &ClassDecl) {
        self.symbols.enter_scope(ScopeKind::Class);

        self.validate_decorators_rejecting_user_defined(&class.decorators, "class");
        // Validate @derive decorators
        self.validate_derives(&class.decorators);
        self.validate_rust_derives(&class.decorators, "class", false, &class.traits);
        let derives = self.extract_derive_names(&class.decorators);

        // Check base class exists
        if let Some(base) = &class.extends
            && self.symbols.lookup(base).is_none()
        {
            self.errors.push(errors::unknown_symbol(base, Span::default()));
        }

        // Check traits exist and are satisfied
        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &class.traits {
            let trait_name = trait_ref.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let Some((trait_info, trait_args)) = self.resolve_adopted_trait_info(&trait_ref.node, trait_ref.span)
                else {
                    continue;
                };
                resolved_trait_adoptions.push(ResolvedTraitAdoption {
                    name: trait_name.to_string(),
                    info: trait_info.clone(),
                    args: trait_args.clone(),
                    module_path: None,
                    span: trait_ref.span,
                });
                self.check_trait_conformance(
                    class,
                    trait_info,
                    trait_name,
                    if trait_args.is_empty() {
                        None
                    } else {
                        Some(trait_args.as_slice())
                    },
                    trait_ref.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }
        let derive_adoption_span = Self::first_derive_span(&class.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(&derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        let class_fields: Vec<_> = class
            .fields
            .iter()
            .map(|field| (field.node.name.as_str(), self.resolve_type_checked(&field.node.ty)))
            .collect();
        let class_type_param_bounds = self.type_param_bound_details_from_type_params(&class.type_params);
        self.validate_awaitable_adoptions(
            &class.name,
            &resolved_trait_adoptions,
            &class_fields,
            class_type_param_bounds,
        );

        // RFC 021: Field aliases are NOT supported on class declarations.
        // Reject any field metadata on class fields.
        for field in &class.fields {
            if field.node.metadata.alias.is_some() {
                self.errors.push(errors::alias_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
            if field.node.metadata.description.is_some() {
                self.errors.push(errors::description_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
        }

        // Define fields
        for field in &class.fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.symbols.define(Symbol {
                name: field.node.name.clone(),
                kind: SymbolKind::Field(FieldInfo {
                    ty,
                    visibility: field.node.visibility,
                    owner: Some(class.name.clone()),
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                }),
                span: field.span,
                scope: 0,
            });

            if let Some(default) = &field.node.default {
                let field_ty = self.resolve_type_checked(&field.node.ty);
                let default_ty = self.check_expr_with_expected(default, Some(&field_ty));
                if !self.types_compatible(&default_ty, &field_ty)
                    && !self.record_validated_newtype_coercion_if_possible(&default_ty, &field_ty, default.span)
                {
                    self.errors.push(errors::type_mismatch(
                        &field_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
            }
        }

        // Check methods
        for method in &class.methods {
            self.check_method_with_owner_type_params(&method.node, &class.name, &class.type_params);
        }
        self.check_method_partials(&class.name, &class.method_partials);
        for property in &class.properties {
            self.check_property(&property.node, &class.name);
        }
        if let Some(TypeInfo::Class(info)) = self.lookup_type_info(&class.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&class.methods);
            self.validate_multi_instantiation_trait_surface(
                &class.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }

        self.symbols.exit_scope();
    }

    /// Validate that a class satisfies required fields and methods for one instantiated trait adoption.
    fn check_trait_conformance(
        &mut self,
        class: &ClassDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        // Use the effective members view (own + inherited) from the symbol table.
        let class_info = self
            .symbols
            .lookup(&class.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Class(info)) => Some(info.clone()),
                _ => None,
            });

        // Check required fields (presence + type compatibility).
        for (field_name, field_ty) in &trait_info.requires {
            match class_info.as_ref().and_then(|ci| ci.fields.get(field_name)) {
                None => {
                    self.errors
                        .push(errors::missing_field(&class.name, field_name, adoption_span));
                }
                Some(found) => {
                    if !self.types_compatible(&found.ty, field_ty) {
                        self.errors.push(errors::trait_required_field_type_mismatch(
                            trait_name,
                            &class.name,
                            field_name,
                            &field_ty.to_string(),
                            &found.ty.to_string(),
                            adoption_span,
                        ));
                    }
                }
            }
        }

        if let Some(ci) = class_info.as_ref() {
            self.enforce_trait_abstract_methods(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.properties,
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    self.errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                }
            }
        }
    }

    /// Validate a trait declaration, including required fields, abstract properties, defaults, and self-typed method
    /// bodies.
    fn check_trait(&mut self, tr: &TraitDecl) {
        self.symbols.enter_scope(ScopeKind::Trait);

        self.validate_decorators_rejecting_user_defined(&tr.decorators, "trait");
        self.reject_rust_allow_on_unsupported_declaration(&tr.decorators, "trait");
        let requires_map: HashMap<String, ResolvedType> = self
            .symbols
            .lookup(&tr.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Trait(info) => Some(info.requires.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .fold(HashMap::new(), |mut acc, (name, ty)| {
                acc.entry(name).or_insert(ty);
                acc
            });
        let prev_trait_requires = self.current_trait_requires.take();
        let prev_trait_properties = self.current_trait_properties.take();
        let prev_trait_name = self.current_trait_name.take();
        let prev_missing_emitted = self.current_trait_missing_requires_emitted.take();
        self.current_trait_requires = Some(requires_map);
        self.current_trait_properties = self.lookup_trait_info(&tr.name).map(|info| info.properties.clone());
        self.current_trait_name = Some(tr.name.clone());
        self.validate_trait_partial_inherited_overrides(&tr.name, &tr.method_partials);

        for method in &tr.methods {
            self.validate_decorators_allowing_user_defined(&method.node.decorators);
            if method.node.body.is_some() {
                let prev_method_seen = self.current_trait_missing_requires_emitted.take();
                self.current_trait_missing_requires_emitted = Some(std::collections::HashSet::new());
                // Trait default methods are checked against `Self` (the eventual adopter type), not against the trait
                // name itself. This allows default bodies to reference adopter fields (validated at adoption sites via
                // `@requires`).
                let trait_type_params: Vec<String> = tr.type_params.iter().map(|tp| tp.name.clone()).collect();
                self.check_method_with_self_ty(
                    &method.node,
                    ResolvedType::SelfType,
                    &trait_type_params,
                    &tr.type_params,
                );
                self.current_trait_missing_requires_emitted = prev_method_seen;
            }
            self.apply_user_defined_method_decorators(&method.node, &tr.name);
        }
        self.check_method_partials(&tr.name, &tr.method_partials);
        for property in &tr.properties {
            if property.node.body.is_some() {
                self.errors.push(errors::trait_property_body_not_supported(
                    &tr.name,
                    &property.node.name,
                    property.span,
                ));
            }
        }

        self.current_trait_requires = prev_trait_requires;
        self.current_trait_properties = prev_trait_properties;
        self.current_trait_name = prev_trait_name;
        self.current_trait_missing_requires_emitted = prev_missing_emitted;
        self.symbols.exit_scope();
    }

    /// Return Rust trait metadata for a direct `rust::...` import used in a `with` clause.
    fn imported_rust_trait_candidate(&self, trait_name: &str) -> Option<(String, Option<RustItemMetadata>)> {
        let sym = self.lookup_symbol(trait_name)?;
        let SymbolKind::RustItem(info) = &sym.kind else {
            return None;
        };
        let metadata = match &info.metadata {
            Some(metadata) if matches!(metadata.kind, RustItemKind::Trait(_)) => Some(metadata.clone()),
            Some(_) => None,
            None => None,
        };
        Some((info.path.clone(), metadata))
    }

    /// Return whether an adopted trait name denotes the RFC 039 `Awaitable` protocol.
    fn is_awaitable_trait_name(trait_name: &str) -> bool {
        trait_name == "Awaitable" || trait_name.ends_with(".Awaitable")
    }

    /// Compare an associated item target with an adopted trait bound by name and arity.
    fn trait_target_matches_bound(target: &TraitBound, bound: &TraitBound) -> bool {
        target.name == bound.name && target.type_args.len() == bound.type_args.len()
    }

    /// Return whether a rusttype declaration authors members for the imported Rust trait instead of requesting
    /// body-less forwarding.
    fn newtype_has_explicit_rust_trait_members(
        nt: &NewtypeDecl,
        trait_bound: &TraitBound,
        trait_metadata: Option<&RustItemMetadata>,
    ) -> bool {
        let method_names: HashSet<String> = trait_metadata
            .and_then(|metadata| match &metadata.kind {
                RustItemKind::Trait(info) => Some(
                    info.items
                        .iter()
                        .filter_map(|item| match item {
                            RustTraitAssoc::Function { name, .. } => Some(name.clone()),
                            RustTraitAssoc::TypeAlias { .. } | RustTraitAssoc::Constant { .. } => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();

        nt.associated_types.iter().any(|associated_type| {
            Self::trait_target_matches_bound(&associated_type.node.trait_target.node, trait_bound)
        }) || nt.methods.iter().any(|method| {
            method
                .node
                .trait_target
                .as_ref()
                .is_some_and(|target| Self::trait_target_matches_bound(&target.node, trait_bound))
                || method_names.contains(&method.node.name)
        })
    }

    /// Return the path suffix used when comparing re-exported Rust trait paths.
    pub(in crate::frontend::typechecker) fn rust_trait_path_suffix(path: &str) -> &str {
        path.split_once("::").map(|(_, suffix)| suffix).unwrap_or(path)
    }

    /// Return whether Rust type metadata proves the backing type implements the requested trait path.
    fn rust_type_metadata_implements_trait(
        type_metadata: &RustItemMetadata,
        trait_path: &str,
        trait_definition_path: Option<&str>,
    ) -> bool {
        let RustItemKind::Type(type_info) = &type_metadata.kind else {
            return false;
        };
        let trait_suffix = Self::rust_trait_path_suffix(trait_path);
        type_info.implemented_traits.iter().any(|implemented| {
            implemented.path == trait_path
                || Some(implemented.path.as_str()) == trait_definition_path
                || Self::rust_trait_path_suffix(&implemented.path) == trait_suffix
        })
    }

    /// Validate imported Rust trait adoption on newtype/rusttype declarations.
    ///
    /// A `rusttype` lowers to a Rust type alias. That means custom `impl ForeignTrait for Alias` output would still be
    /// an impl for the foreign backing type and can violate Rust coherence. Body-less adoption is therefore accepted
    /// only when rust-inspect proves the backing type already implements the trait; lowering then skips the impl.
    fn validate_imported_rust_trait_newtype_adoption(
        &mut self,
        nt: &NewtypeDecl,
        trait_bound: &TraitBound,
        trait_span: Span,
        trait_path: &str,
        trait_metadata: Option<&RustItemMetadata>,
        rusttype_path: Option<&str>,
    ) {
        let has_explicit_members = Self::newtype_has_explicit_rust_trait_members(nt, trait_bound, trait_metadata);
        if nt.is_rusttype {
            if has_explicit_members {
                self.errors.push(errors::rusttype_foreign_trait_impl_orphan(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            }
            if !trait_bound.type_args.is_empty() {
                self.errors.push(errors::rusttype_forwarding_generic_trait_blocked(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            }
            let Some(backing_path) = rusttype_path else {
                return;
            };
            let Some(backing_metadata) = self.rust_item_metadata_for_path(backing_path) else {
                self.errors.push(errors::rusttype_forwarding_requires_metadata(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            };
            let trait_definition_path = trait_metadata.and_then(|metadata| metadata.definition_path.as_deref());
            if Self::rust_type_metadata_implements_trait(&backing_metadata, trait_path, trait_definition_path) {
                self.type_info
                    .rust
                    .rusttype_forwarded_trait_adoptions
                    .insert((nt.name.clone(), trait_bound.name.clone()));
            } else {
                self.errors.push(errors::rusttype_forwarding_trait_not_implemented(
                    &nt.name,
                    backing_path,
                    trait_path,
                    trait_span,
                ));
            }
            return;
        }

        let Some(metadata) = trait_metadata else {
            return;
        };
        let RustItemKind::Trait(info) = &metadata.kind else {
            return;
        };
        let declared_associated_types: HashSet<&str> = nt
            .associated_types
            .iter()
            .filter(|associated_type| {
                Self::trait_target_matches_bound(&associated_type.node.trait_target.node, trait_bound)
            })
            .map(|associated_type| associated_type.node.name.as_str())
            .collect();
        for item in &info.items {
            let RustTraitAssoc::TypeAlias { name } = item else {
                continue;
            };
            if !declared_associated_types.contains(name.as_str()) {
                self.errors
                    .push(errors::missing_rust_associated_type(trait_path, name, trait_span));
            }
        }
    }

    /// Validate one newtype or rusttype declaration after collection has registered its symbol.
    fn check_newtype(&mut self, nt: &NewtypeDecl) {
        self.symbols.enter_scope(ScopeKind::Block);

        for param in &nt.type_params {
            self.symbols.define(Symbol {
                name: param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }

        self.validate_decorators_rejecting_user_defined(&nt.decorators, "newtype");
        self.validate_rust_derives(&nt.decorators, "newtype", nt.is_rusttype, &nt.traits);

        // Check underlying type exists
        let underlying = self.resolve_type_checked(&nt.underlying);
        if matches!(underlying, ResolvedType::Unknown) {
            self.errors.push(errors::unknown_symbol(
                &format!("{:?}", nt.underlying.node),
                nt.underlying.span,
            ));
        }

        let rusttype_path = if nt.is_rusttype {
            self.rust_path_for_rusttype_underlying(&underlying)
        } else {
            None
        };
        if nt.is_rusttype && rusttype_path.is_none() {
            self.errors
                .push(errors::rusttype_requires_rust_backing(&nt.name, nt.underlying.span));
        }
        if nt.is_rusttype
            && let Some(path) = &rusttype_path
        {
            self.type_info
                .rust
                .rusttype_canonical_paths
                .insert(nt.name.clone(), path.clone());
        }
        if !nt.is_rusttype && !nt.interop_edges.is_empty() {
            self.errors
                .push(errors::interop_block_requires_rusttype(&nt.name, nt.underlying.span));
        }
        self.validate_newtype_from_underlying_hook(nt, &underlying);

        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &nt.traits {
            let trait_name = trait_ref.node.name.as_str();
            if nt.is_rusttype && Self::is_awaitable_trait_name(trait_name) {
                self.errors
                    .push(errors::awaitable_future_bridge_blocked(&nt.name, trait_ref.span));
                continue;
            }
            if let Some((trait_path, trait_metadata)) = self.imported_rust_trait_candidate(trait_name) {
                self.validate_imported_rust_trait_newtype_adoption(
                    nt,
                    &trait_ref.node,
                    trait_ref.span,
                    trait_path.as_str(),
                    trait_metadata.as_ref(),
                    rusttype_path.as_deref(),
                );
            } else if self.lookup_trait_info(trait_name).is_some() {
                let Some((trait_info, trait_args)) = self.resolve_adopted_trait_info(&trait_ref.node, trait_ref.span)
                else {
                    continue;
                };
                resolved_trait_adoptions.push(ResolvedTraitAdoption {
                    name: trait_name.to_string(),
                    info: trait_info.clone(),
                    args: trait_args.clone(),
                    module_path: None,
                    span: trait_ref.span,
                });
                self.check_trait_conformance_newtype(
                    nt,
                    trait_info,
                    trait_name,
                    if trait_args.is_empty() {
                        None
                    } else {
                        Some(trait_args.as_slice())
                    },
                    trait_ref.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }

        for associated_type in &nt.associated_types {
            let trait_name = associated_type.node.trait_target.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let _ = self.resolve_adopted_trait_info(
                    &associated_type.node.trait_target.node,
                    associated_type.node.trait_target.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(
                    trait_name,
                    associated_type.node.trait_target.span,
                ));
            }
            let _ = self.resolve_type_checked(&associated_type.node.ty);
        }

        for rebinding in &nt.rebindings {
            // Short-form rebinding targets (`alias = method`) should be interpreted as method names,
            // not as local variable reads.
            if !matches!(rebinding.node.target.node, Expr::Ident(_)) {
                let _ = self.check_expr(&rebinding.node.target);
            }
        }

        let rusttype_ty = self.rusttype_decl_resolved_type(nt);
        let mut seen_edges: HashMap<(bool, String), (InteropAdapterKind, String, Span)> = HashMap::new();
        for edge in &nt.interop_edges {
            let boundary_ty = self.resolve_type_checked(&edge.node.ty);
            let key_ty = boundary_ty.to_string();
            if !matches!(boundary_ty, ResolvedType::Unknown) {
                let key = (matches!(edge.node.direction, InteropDirection::Into), key_ty.clone());
                let adapter_ref = Self::interop_adapter_ref_display(&edge.node.adapter);
                if let Some((prev_kind, prev_adapter, prev_span)) = seen_edges.get(&key) {
                    if *prev_kind == edge.node.adapter_kind && prev_adapter == &adapter_ref {
                        self.errors.push(errors::duplicate_interop_edge(
                            &nt.name,
                            if key.0 { "into" } else { "from" },
                            key_ty.as_str(),
                            *prev_span,
                            edge.span,
                        ));
                    } else {
                        self.errors.push(errors::conflicting_interop_edge(
                            &nt.name,
                            if key.0 { "into" } else { "from" },
                            key_ty.as_str(),
                            prev_adapter.as_str(),
                            adapter_ref.as_str(),
                            edge.span,
                        ));
                    }
                } else {
                    seen_edges.insert(key, (edge.node.adapter_kind, adapter_ref, edge.span));
                }
            }

            if let Some(adapter_sig) = self.resolve_interop_adapter_signature(nt, edge) {
                self.validate_interop_adapter_signature(nt, edge, &boundary_ty, &rusttype_ty, &adapter_sig);
            }
        }

        // Check methods (reuse the standard method-checking logic so parameters are in scope).
        for method in &nt.methods {
            if method.node.body.is_some() {
                self.check_method_with_owner_type_params(&method.node, &nt.name, &nt.type_params);
            }
        }
        self.check_method_partials(&nt.name, &nt.method_partials);

        if let Some(TypeInfo::Newtype(info)) = self.lookup_type_info(&nt.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&nt.methods);
            self.validate_multi_instantiation_trait_surface(
                &nt.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }

        self.symbols.exit_scope();
    }

    /// Validate explicit newtype/rusttype trait adoption using the same method contract as other nominal types.
    ///
    /// Newtypes do not expose storage fields or properties in the trait surface, so field/property obligations are
    /// rejected at the adoption site. Method obligations are checked against the collected newtype method overload map.
    fn check_trait_conformance_newtype(
        &mut self,
        nt: &NewtypeDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        for (field_name, _) in &trait_info.requires {
            self.errors
                .push(errors::missing_field(&nt.name, field_name, adoption_span));
        }

        let newtype_info = self
            .symbols
            .lookup(&nt.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Newtype(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(info) = newtype_info {
            self.enforce_trait_abstract_methods(
                &nt.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &nt.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &HashMap::new(),
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    self.errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                }
            }
        }
    }

    /// Validate the canonical RFC 017 `from_underlying` hook when a newtype declares one.
    fn validate_newtype_from_underlying_hook(&mut self, nt: &NewtypeDecl, underlying: &ResolvedType) {
        let Some(method_decl) = nt
            .methods
            .iter()
            .find(|method| method.node.name == incan_core::lang::conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
        else {
            return;
        };
        let method_span = method_decl.span;
        let method_info = self.lookup_type_info(&nt.name).and_then(|info| match info {
            TypeInfo::Newtype(newtype) => newtype
                .methods
                .get(incan_core::lang::conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
                .cloned(),
            _ => None,
        });
        let Some(method_info) = method_info else {
            return;
        };

        if method_info.receiver.is_some() {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must be a static method with no self receiver",
                method_span,
            ));
        }
        if method_info.is_async {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must be deterministic and cannot be async",
                method_span,
            ));
        }
        if method_info.params.len() != 1
            || method_info
                .params
                .first()
                .is_some_and(|param| param.kind != ParamKind::Normal)
        {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must accept exactly one ordinary parameter",
                method_span,
            ));
        } else if let Some(param) = method_info.params.first()
            && !self.types_compatible(&param.ty, underlying)
        {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                &format!(
                    "parameter type must be the newtype underlying type '{underlying}', found '{}'",
                    param.ty
                ),
                method_span,
            ));
        }

        let expected_err = ResolvedType::Named("ValidationError".to_string());
        let valid_return = matches!(
            &method_info.return_type,
            ResolvedType::Generic(name, args)
                if crate::frontend::typechecker::helpers::collection_type_id(name.as_str())
                    == Some(incan_core::lang::types::collections::CollectionTypeId::Result)
                    && args.len() == 2
                    && (
                        matches!(&args[0], ResolvedType::Named(name) if name == &nt.name)
                            || matches!(&args[0], ResolvedType::SelfType)
                    )
                    && args[1] == expected_err
        );
        if !valid_return {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                &format!(
                    "return type must be Result[{}, ValidationError], found '{}'",
                    nt.name, method_info.return_type
                ),
                method_span,
            ));
        }
    }

    /// Validate enum decorators, value-enum rules, and variant payload field types.
    fn check_enum(&mut self, en: &EnumDecl) {
        self.symbols.enter_scope(ScopeKind::Block);

        self.validate_decorators_rejecting_user_defined(&en.decorators, "enum");
        self.validate_derives(&en.decorators);
        self.validate_rust_derives(&en.decorators, "enum", false, &en.traits);
        let derives = self.extract_derive_names(&en.decorators);

        for param in &en.type_params {
            self.symbols.define(Symbol {
                name: param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }

        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &en.traits {
            let trait_name = trait_ref.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let Some((trait_info, trait_args)) = self.resolve_adopted_trait_info(&trait_ref.node, trait_ref.span)
                else {
                    continue;
                };
                resolved_trait_adoptions.push(ResolvedTraitAdoption {
                    name: trait_name.to_string(),
                    info: trait_info.clone(),
                    args: trait_args.clone(),
                    module_path: None,
                    span: trait_ref.span,
                });
                self.check_trait_conformance_enum(
                    en,
                    trait_info,
                    trait_name,
                    if trait_args.is_empty() {
                        None
                    } else {
                        Some(trait_args.as_slice())
                    },
                    trait_ref.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }
        let derive_adoption_span = Self::first_derive_span(&en.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(&derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        let enum_type_param_bounds = self.type_param_bound_details_from_type_params(&en.type_params);
        self.validate_awaitable_adoptions(&en.name, &resolved_trait_adoptions, &[], enum_type_param_bounds);

        self.check_value_enum_decl(en);
        self.check_enum_variant_aliases(en);
        // Check variant field types exist
        for variant in &en.variants {
            for field_ty in &variant.node.fields {
                let resolved = self.resolve_type_checked(field_ty);
                if matches!(resolved, ResolvedType::Unknown) {
                    self.errors
                        .push(errors::unknown_symbol(&format!("{:?}", field_ty.node), field_ty.span));
                }
            }
        }

        for method in &en.methods {
            self.check_method_with_owner_type_params(&method.node, &en.name, &en.type_params);
        }
        if let Some(TypeInfo::Enum(info)) = self.lookup_type_info(&en.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&en.methods);
            self.validate_multi_instantiation_trait_surface(
                &en.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }

        self.symbols.exit_scope();
    }

    /// Validate enum variant aliases against the variant namespace.
    fn check_enum_variant_aliases(&mut self, en: &EnumDecl) {
        let variants: HashSet<&str> = en.variants.iter().map(|variant| variant.node.name.as_str()).collect();
        let mut aliases: HashSet<&str> = HashSet::new();
        for alias in &en.variant_aliases {
            if variants.contains(alias.node.name.as_str()) || !aliases.insert(alias.node.name.as_str()) {
                self.errors
                    .push(errors::duplicate_definition(&alias.node.name, alias.span));
            }
            if !variants.contains(alias.node.target.as_str()) {
                self.errors.push(errors::unknown_symbol(&alias.node.target, alias.span));
            }
        }
    }

    /// Validate explicit enum trait adoption using the same abstract-method contract as models/classes.
    ///
    /// Enums currently do not expose fields, so `@requires(...)` obligations are reported as missing fields at the
    /// adoption site. Method obligations are checked against the collected enum method map.
    fn check_trait_conformance_enum(
        &mut self,
        en: &EnumDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        for (field_name, _) in &trait_info.requires {
            self.errors
                .push(errors::missing_field(&en.name, field_name, adoption_span));
        }

        let enum_info = self
            .symbols
            .lookup(&en.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Enum(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(info) = enum_info {
            self.enforce_trait_abstract_methods(
                &en.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &en.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &en.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &HashMap::new(),
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    self.errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                }
            }
        }
    }

    /// Validate the declaration-time invariants that make value enum helpers type safe.
    fn check_value_enum_decl(&mut self, en: &EnumDecl) {
        let Some(value_type) = &en.value_type else {
            for variant in &en.variants {
                if variant.node.value.is_some() {
                    self.errors.push(errors::regular_enum_variant_value_not_allowed(
                        &en.name,
                        &variant.node.name,
                        variant.span,
                    ));
                }
            }
            return;
        };

        if !en.type_params.is_empty() {
            self.errors
                .push(errors::value_enum_type_params_not_supported(&en.name, value_type.span));
        }

        let backing = match value_type.node {
            ValueEnumType::Str => ValueEnumBacking::Str,
            ValueEnumType::Int => ValueEnumBacking::Int,
        };
        let mut seen_values: HashMap<ValueEnumValue, (&str, Span)> = HashMap::new();

        for variant in &en.variants {
            if matches!(variant.node.name.as_str(), "value" | "from_value") {
                self.errors.push(errors::value_enum_reserved_generated_name(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
            }

            if !variant.node.fields.is_empty() {
                self.errors.push(errors::value_enum_variant_payload_not_allowed(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
            }

            let Some(raw_value) = &variant.node.value else {
                self.errors.push(errors::value_enum_variant_missing_value(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
                continue;
            };

            let value = match (backing, &raw_value.node) {
                (ValueEnumBacking::Str, ValueEnumLiteral::Str(value)) => ValueEnumValue::Str(value.clone()),
                (ValueEnumBacking::Int, ValueEnumLiteral::Int(value)) => ValueEnumValue::Int(value.value),
                (ValueEnumBacking::Str, ValueEnumLiteral::Int(_)) => {
                    self.errors.push(errors::value_enum_literal_type_mismatch(
                        &en.name,
                        backing.display_name(),
                        "int",
                        raw_value.span,
                    ));
                    continue;
                }
                (ValueEnumBacking::Int, ValueEnumLiteral::Str(_)) => {
                    self.errors.push(errors::value_enum_literal_type_mismatch(
                        &en.name,
                        backing.display_name(),
                        "str",
                        raw_value.span,
                    ));
                    continue;
                }
            };

            if let Some((first_variant, _first_span)) = seen_values.get(&value) {
                self.errors.push(errors::value_enum_duplicate_value(
                    &en.name,
                    &value.display_value(),
                    first_variant,
                    &variant.node.name,
                    raw_value.span,
                ));
            } else {
                seen_values.insert(value, (variant.node.name.as_str(), raw_value.span));
            }
        }
    }

    /// Apply RFC 036 user-defined decorators to a top-level function binding after its body has been checked.
    ///
    /// The undecorated function is already present in the module symbol table from collection. Each user-defined
    /// decorator is checked as `decorator(current_binding)`, bottom-up, and the module-visible binding is then replaced
    /// with the resulting type as an ordinary immutable value.
    fn apply_user_defined_function_decorators(&mut self, func: &FunctionDecl, span: Span) {
        if !func
            .decorators
            .iter()
            .any(|decorator| self.is_user_defined_decorator_candidate(&decorator.node))
        {
            return;
        }

        let Some((original_ty, original_function_info)) =
            self.lookup_symbol(&func.name).and_then(|symbol| match &symbol.kind {
                SymbolKind::Function(info) => Some((function_info_callable_type(info), Some(info.clone()))),
                SymbolKind::Variable(info) => Some((info.ty.clone(), None)),
                _ => None,
            })
        else {
            return;
        };

        let mut binding_ty = original_ty.clone();
        for decorator in func.decorators.iter().rev() {
            if self.is_user_defined_decorator_candidate(&decorator.node) {
                binding_ty = self.apply_user_defined_decorator(decorator, binding_ty, &func.name);
            }
        }

        if let Some(symbol_id) = self.symbols.lookup(&func.name)
            && let Some(symbol) = self.symbols.get_mut(symbol_id)
        {
            self.type_info.declarations.decorated_function_bindings.insert(
                func.name.clone(),
                DecoratedFunctionBindingInfo {
                    ty: binding_ty.clone(),
                    original_ty,
                    type_params: original_function_info
                        .as_ref()
                        .map_or_else(Vec::new, |info| info.type_params.clone()),
                    type_param_bounds: original_function_info
                        .as_ref()
                        .map_or_else(HashMap::new, |info| info.type_param_bounds.clone()),
                    type_param_bound_details: original_function_info
                        .as_ref()
                        .map_or_else(HashMap::new, |info| info.type_param_bound_details.clone()),
                    is_async: original_function_info.as_ref().is_some_and(|info| info.is_async),
                },
            );
            symbol.kind = SymbolKind::Variable(VariableInfo {
                ty: binding_ty,
                is_mutable: false,
                is_used: false,
            });
            symbol.span = span;
        }
    }

    /// Apply RFC 036 user-defined decorators to a method table entry when the current method representation can store
    /// the resulting callable signature.
    fn apply_user_defined_method_decorators(&mut self, method: &MethodDecl, owner: &str) {
        if !method
            .decorators
            .iter()
            .any(|decorator| self.is_user_defined_decorator_candidate(&decorator.node))
        {
            return;
        }

        let Some(mut method_info) = self.lookup_method_info_for_update(owner, &method.name) else {
            return;
        };
        let receiver_ty = self.decorated_method_receiver_type(owner, method.receiver);
        let original_binding_ty = method_info_callable_type(&method_info, receiver_ty);
        let mut binding_ty = original_binding_ty.clone();
        for decorator in method.decorators.iter().rev() {
            if self.is_user_defined_decorator_candidate(&decorator.node) {
                binding_ty = self.apply_user_defined_decorator(decorator, binding_ty, &method.name);
            }
        }

        let ResolvedType::Function(params, ret) = binding_ty.clone() else {
            return;
        };
        let Some((_receiver, surface_params)) = params.split_first() else {
            return;
        };
        self.type_info.declarations.decorated_method_bindings.insert(
            (owner.to_string(), method.name.clone()),
            DecoratedMethodBindingInfo {
                unbound_ty: binding_ty,
                original_unbound_ty: original_binding_ty,
            },
        );
        method_info.params = surface_params.to_vec();
        method_info.return_type = *ret;
        self.replace_method_info(owner, &method.name, method_info);
    }

    /// Return the source-level receiver type passed as the first argument to a method decorator.
    fn decorated_method_receiver_type(&self, owner: &str, receiver: Option<Receiver>) -> ResolvedType {
        let owner_ty = ResolvedType::Named(owner.to_string());
        if receiver == Some(Receiver::Mutable) {
            ResolvedType::RefMut(Box::new(owner_ty))
        } else {
            ResolvedType::Ref(Box::new(owner_ty))
        }
    }

    /// Return the method metadata currently visible for an owner and method name.
    fn lookup_method_info_for_update(&self, owner: &str, method_name: &str) -> Option<MethodInfo> {
        let symbol_id = self.symbols.lookup(owner)?;
        let symbol = self.symbols.get(symbol_id)?;
        match &symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Class(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Newtype(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Type(TypeInfo::Enum(info)) => info.methods.get(method_name).cloned(),
            SymbolKind::Trait(info) => info.methods.get(method_name).cloned(),
            _ => None,
        }
    }

    /// Replace an owner method entry after decorator application has produced a representable callable type.
    fn replace_method_info(&mut self, owner: &str, method_name: &str, method_info: MethodInfo) {
        let Some(symbol_id) = self.symbols.lookup(owner) else {
            return;
        };
        let Some(symbol) = self.symbols.get_mut(symbol_id) else {
            return;
        };
        match &mut symbol.kind {
            SymbolKind::Type(TypeInfo::Model(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Type(TypeInfo::Class(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Type(TypeInfo::Newtype(info)) => {
                info.methods.insert(method_name.to_string(), method_info);
            }
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                info.methods.insert(method_name.to_string(), method_info.clone());
                if let Some(overloads) = info.method_overloads.get_mut(method_name) {
                    *overloads = vec![method_info];
                }
            }
            SymbolKind::Trait(info) => {
                info.methods.insert(method_name.to_string(), method_info);
            }
            _ => {}
        }
    }

    /// Type-check one decorator application against the current decorated binding type.
    fn apply_user_defined_decorator(
        &mut self,
        decorator: &Spanned<Decorator>,
        binding_ty: ResolvedType,
        binding_name: &str,
    ) -> ResolvedType {
        let display = Self::decorator_display(&decorator.node);
        let callable_ty = if decorator.node.is_call {
            self.check_decorator_factory_expr(decorator, &display)
        } else {
            self.check_expr(&Self::decorator_path_expr(&decorator.node, decorator.span))
        };

        self.apply_decorator_callable(&display, callable_ty, binding_ty, binding_name, decorator.span)
    }

    /// Type-check a decorator factory expression such as `@logged(label="x")` or `@app.get("/")`.
    fn check_decorator_factory_expr(&mut self, decorator: &Spanned<Decorator>, display: &str) -> ResolvedType {
        let Some(args) = self.decorator_call_args(decorator, display) else {
            return ResolvedType::Unknown;
        };
        let path = &decorator.node.path.segments;
        let factory_expr = if path.len() >= 2 {
            let base_path = ImportPath {
                parent_levels: decorator.node.path.parent_levels,
                is_absolute: decorator.node.path.is_absolute,
                segments: path[..path.len() - 1].to_vec(),
            };
            let base = Self::decorator_path_expr_from_import_path(&base_path, decorator.span);
            let method = path.last().cloned().unwrap_or_default();
            Spanned::new(
                Expr::MethodCall(Box::new(base), method, decorator.node.type_args.clone(), args),
                decorator.span,
            )
        } else {
            let callee = Self::decorator_path_expr(&decorator.node, decorator.span);
            Spanned::new(
                Expr::Call(Box::new(callee), decorator.node.type_args.clone(), args),
                decorator.span,
            )
        };
        self.check_expr(&factory_expr)
    }

    /// Apply a callable decorator value to the decorated binding type and return the post-decoration callable type.
    fn apply_decorator_callable(
        &mut self,
        display: &str,
        callable_ty: ResolvedType,
        binding_ty: ResolvedType,
        binding_name: &str,
        span: Span,
    ) -> ResolvedType {
        let ResolvedType::Function(params, ret) = callable_ty else {
            if !matches!(callable_ty, ResolvedType::Unknown) {
                self.errors.push(if display.contains('(') {
                    errors::decorator_factory_not_callable(display, span)
                } else {
                    errors::decorator_not_callable(display, span)
                });
            }
            return ResolvedType::Unknown;
        };

        let arg_expr = Spanned::new(Expr::Ident(binding_name.to_string()), span);
        let args = vec![CallArg::Positional(arg_expr)];
        let arg_types = vec![binding_ty];
        let mut type_bindings = HashMap::new();
        let error_count = self.errors.len();
        self.validate_callable_arg_bindings(display, &params, &args, &arg_types, &mut type_bindings, span);
        if self.errors.len() != error_count {
            return ResolvedType::Unknown;
        }
        let result_ty = substitute_resolved_type(&ret, &type_bindings);
        if !matches!(result_ty, ResolvedType::Function(_, _) | ResolvedType::Unknown) {
            self.errors.push(errors::decorator_result_not_callable(display, span));
            return ResolvedType::Unknown;
        }
        result_ty
    }

    /// Convert decorator arguments into ordinary call arguments for user-defined decorator factory checking.
    fn decorator_call_args(&mut self, decorator: &Spanned<Decorator>, display: &str) -> Option<Vec<CallArg>> {
        let mut args = Vec::new();
        let mut valid = true;
        for arg in &decorator.node.args {
            match arg {
                DecoratorArg::Positional(expr) => args.push(CallArg::Positional(expr.clone())),
                DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)) => {
                    args.push(CallArg::Named(name.clone(), expr.clone()));
                }
                DecoratorArg::Named(_, DecoratorArgValue::Type(ty)) => {
                    self.errors
                        .push(errors::decorator_type_argument_not_supported(display, ty.span));
                    valid = false;
                }
            }
        }
        valid.then_some(args)
    }

    /// Render a decorator path with a coarse call marker for diagnostics.
    fn decorator_display(decorator: &Decorator) -> String {
        let path = decorator.path.segments.join(".");
        if decorator.is_call {
            if decorator.type_args.is_empty() {
                format!("{path}(...)")
            } else {
                format!("{path}[...](...)")
            }
        } else {
            path
        }
    }

    /// Build an expression from a decorator's path.
    fn decorator_path_expr(decorator: &Decorator, span: Span) -> Spanned<Expr> {
        Self::decorator_path_expr_from_import_path(&decorator.path, span)
    }

    /// Build an identifier/field expression from an import-style decorator path.
    fn decorator_path_expr_from_import_path(path: &ImportPath, span: Span) -> Spanned<Expr> {
        let mut segments = path.segments.iter();
        let Some(first) = segments.next() else {
            return Spanned::new(Expr::Ident(String::new()), span);
        };
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in segments {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        expr
    }

    /// Typecheck one function body with its parameters, return type, decorators, and generic bounds in scope.
    fn check_function(&mut self, func: &FunctionDecl) {
        self.symbols.enter_scope(ScopeKind::Function);

        self.validate_decorators_allowing_user_defined(&func.decorators);
        self.validate_callable_rest_params(&func.params);
        let fixture_span = fixture_function_span(func);
        let fixture_args = self.testing_fixture_marker_args(&func.decorators, fixture_span);
        if let Some(args) = &fixture_args {
            if let Some(span) = args.unsupported_timeout_span {
                self.errors
                    .push(errors::fixture_timeout_config_not_supported(&func.name, span));
            }

            let has_teardown = if func.is_async() {
                match validate_async_fixture_yield_shape(&func.body) {
                    AsyncFixtureYieldShape::Valid { has_teardown } => has_teardown,
                    AsyncFixtureYieldShape::Missing => {
                        self.errors
                            .push(errors::async_fixture_requires_yield(&func.name, fixture_span));
                        false
                    }
                    AsyncFixtureYieldShape::Invalid(span) => {
                        self.errors
                            .push(errors::async_fixture_invalid_yield_shape(&func.name, span));
                        false
                    }
                    AsyncFixtureYieldShape::MissingValue(span) => {
                        self.errors
                            .push(errors::async_fixture_yield_requires_value(&func.name, span));
                        false
                    }
                }
            } else {
                fixture_body_has_yield(&func.body)
            };

            let dependencies = func
                .params
                .iter()
                .filter(|param| self.testing_fixture_names.contains(&param.node.name))
                .map(|param| param.node.name.clone())
                .collect();
            self.type_info.testing.fixtures.insert(
                func.name.clone(),
                TestingFixtureInfo {
                    scope: args.scope,
                    autouse: args.autouse,
                    is_async: func.is_async(),
                    has_teardown,
                    dependencies,
                },
            );
        }
        self.validate_surface_modifier_typecheck_actions(&func.surface_modifiers, &func.return_type);

        // Define type parameters so explicit generic bounds are visible in function-level type resolution.
        for param in &func.type_params {
            self.symbols.define(Symbol {
                name: param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin), // Type-var placeholder
                span: Span::default(),
                scope: 0,
            });
        }
        let active_bounds = self.type_param_bound_details_from_type_params(&func.type_params);
        self.current_type_param_bound_details.push(active_bounds);

        // Define parameters
        for param in &func.params {
            let ty = local_type_for_param(param.node.kind, self.resolve_type_checked(&param.node.ty));
            self.symbols.define(Symbol {
                name: param.node.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span: param.span,
                scope: 0,
            });
        }

        let return_type = self.resolve_type_checked(&func.return_type);
        let has_yield = any_expr_in_body(&func.body, |expr| matches!(expr, Expr::Yield(_)));
        if return_type.generator_element_type().is_some() && !has_yield && !body_has_return_value(&func.body) {
            self.errors
                .push(errors::generator_requires_yield(&func.name, func.return_type.span));
        }
        let yield_context = if fixture_args.is_some() {
            YieldContext::Fixture
        } else if has_yield {
            match return_type.generator_element_type() {
                Some(element_ty) => YieldContext::Generator {
                    element_ty: element_ty.clone(),
                },
                None => YieldContext::Disallowed,
            }
        } else {
            YieldContext::Disallowed
        };
        self.symbols.set_return_type(return_type.clone());

        // Set error type for ? checking
        self.current_return_error_type = return_type.result_err_type().cloned();

        let prev_in_async_body = self.in_async_body;
        let prev_yield_context = std::mem::replace(&mut self.current_yield_context, yield_context);
        self.in_async_body = func.is_async();
        let previous_consumed_iterator_bindings = std::mem::take(&mut self.consumed_iterator_bindings);

        // Check body
        for stmt in &func.body {
            self.check_statement(stmt);
        }

        self.consumed_iterator_bindings = previous_consumed_iterator_bindings;
        self.in_async_body = prev_in_async_body;
        self.current_yield_context = prev_yield_context;
        self.current_return_error_type = None;
        self.current_type_param_bound_details.pop();
        self.symbols.exit_scope();
        self.apply_user_defined_function_decorators(func, fixture_span);
    }

    /// Resolve generic type-parameter bounds while preserving trait type arguments for call-site checks.
    fn type_param_bound_details_from_type_params(
        &mut self,
        type_params: &[TypeParam],
    ) -> HashMap<String, Vec<TypeBoundInfo>> {
        type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: bound.name.clone(),
                            source_name: None,
                            type_args: bound
                                .type_args
                                .iter()
                                .map(|type_arg| self.resolve_type_checked(type_arg))
                                .collect(),
                            module_path: None,
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Validate a model, class, enum, or newtype method body using the concrete nominal owner as `self`.
    fn check_method_with_owner_type_params(&mut self, method: &MethodDecl, owner: &str, owner_params: &[TypeParam]) {
        self.validate_decorators_allowing_user_defined(&method.decorators);
        if method.body.is_none() {
            self.errors.push(errors::concrete_method_requires_body(
                &method.name,
                method.return_type.span,
            ));
            self.apply_user_defined_method_decorators(method, owner);
            return;
        }
        let owner_type_params = self
            .lookup_type_info(owner)
            .map(|info| match info {
                TypeInfo::Model(model) => model.type_params.clone(),
                TypeInfo::Class(class) => class.type_params.clone(),
                TypeInfo::Newtype(newtype) => newtype.type_params.clone(),
                TypeInfo::Enum(enum_info) => enum_info.type_params.clone(),
                TypeInfo::Builtin | TypeInfo::TypeAlias => Vec::new(),
            })
            .unwrap_or_default();
        let previous_owner = self.current_method_owner.replace(owner.to_string());
        let owner_self_ty = if owner_type_params.is_empty() {
            ResolvedType::Named(owner.to_string())
        } else {
            ResolvedType::Generic(
                owner.to_string(),
                owner_type_params
                    .iter()
                    .map(|type_param| ResolvedType::TypeVar(type_param.clone()))
                    .collect(),
            )
        };
        self.check_method_with_self_ty(method, owner_self_ty, &owner_type_params, owner_params);
        self.current_method_owner = previous_owner;
        self.apply_user_defined_method_decorators(method, owner);
    }

    /// Validate a model or class computed property body using the concrete nominal owner as immutable `self`.
    pub(crate) fn check_property(&mut self, property: &PropertyDecl, owner: &str) {
        let owner_type_params = self
            .lookup_type_info(owner)
            .map(|info| match info {
                TypeInfo::Model(model) => model.type_params.clone(),
                TypeInfo::Class(class) => class.type_params.clone(),
                TypeInfo::Builtin | TypeInfo::TypeAlias | TypeInfo::Newtype(_) | TypeInfo::Enum(_) => Vec::new(),
            })
            .unwrap_or_default();
        let previous_owner = self.current_method_owner.replace(owner.to_string());
        let owner_self_ty = if owner_type_params.is_empty() {
            ResolvedType::Named(owner.to_string())
        } else {
            ResolvedType::Generic(
                owner.to_string(),
                owner_type_params
                    .iter()
                    .map(|type_param| ResolvedType::TypeVar(type_param.clone()))
                    .collect(),
            )
        };
        self.check_property_with_self_ty(property, owner_self_ty, &owner_type_params);
        self.current_method_owner = previous_owner;
    }

    /// Check a computed property body with the supplied `self` type.
    fn check_property_with_self_ty(
        &mut self,
        property: &PropertyDecl,
        self_ty: ResolvedType,
        owner_type_params: &[String],
    ) {
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: Some(Receiver::Immutable),
        });

        for type_param in owner_type_params {
            self.symbols.define(Symbol {
                name: type_param.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }

        self.symbols.define(Symbol {
            name: "self".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: self_ty.clone(),
                is_mutable: false,
                is_used: true,
            }),
            span: Span::default(),
            scope: 0,
        });

        let return_type = self.resolve_type_checked(&property.return_type);
        let effective_return_type = Self::concretize_self_type_in_annotation(&return_type, &self_ty);
        self.symbols.set_return_type(effective_return_type);
        self.current_return_error_type = return_type.result_err_type().cloned();

        if let Some(body) = &property.body {
            let prev_in_async_body = self.in_async_body;
            self.in_async_body = false;
            for stmt in body {
                self.check_statement(stmt);
            }
            self.in_async_body = prev_in_async_body;
        }

        self.current_return_error_type = None;
        self.symbols.exit_scope();
    }

    /// Check a method body with the concrete owner type used for `Self` in annotations and classmethod constructors.
    ///
    /// Generic owners pass `Owner[T, ...]` here so return checking and `cls(...)` constructor calls see the same
    /// generic surface that call sites use. Trait default methods may still pass bare `Self` because their eventual
    /// adopter is resolved later during trait conformance and method-call substitution.
    fn check_method_with_self_ty(
        &mut self,
        method: &MethodDecl,
        self_ty: ResolvedType,
        owner_type_params: &[String],
        owner_params: &[TypeParam],
    ) {
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: method.receiver,
        });
        self.validate_callable_rest_params(&method.params);
        self.validate_surface_modifier_typecheck_actions(&method.surface_modifiers, &method.return_type);

        // Define owner type parameters so generic wrappers can use them in bodies and annotations.
        for type_param in owner_type_params {
            self.symbols.define(Symbol {
                name: type_param.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }

        // Define method type parameters so generic methods can use them in signatures and bodies.
        for type_param in &method.type_params {
            self.symbols.define(Symbol {
                name: type_param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
        }
        if let Some(target) = &method.trait_target {
            let trait_name = target.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let _ = self.resolve_adopted_trait_info(&target.node, target.span);
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, target.span));
            }
        }
        let mut active_bounds = self.type_param_bound_details_from_type_params(owner_params);
        active_bounds.extend(self.type_param_bound_details_from_type_params(&method.type_params));
        self.current_type_param_bound_details.push(active_bounds);

        // Define self if present
        if let Some(receiver) = method.receiver {
            let is_mutable = matches!(receiver, Receiver::Mutable);
            if is_mutable {
                self.mutable_bindings.insert("self".to_string());
            }
            self.symbols.define(Symbol {
                name: "self".to_string(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: self_ty.clone(),
                    is_mutable,
                    is_used: true,
                }),
                span: Span::default(),
                scope: 0,
            });
        }
        let is_classmethod = Self::method_has_decorator(method, DecoratorId::ClassMethod);
        let previous_classmethod_self_ty = self.current_classmethod_self_ty.take();
        if is_classmethod {
            self.current_classmethod_self_ty = Some(self_ty.clone());
        }

        // Define parameters
        for param in &method.params {
            let ty = local_type_for_param(param.node.kind, self.resolve_type_checked(&param.node.ty));
            self.symbols.define(Symbol {
                name: param.node.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span: param.span,
                scope: 0,
            });
        }

        let return_type = self.resolve_type_checked(&method.return_type);
        let effective_return_type = Self::concretize_self_type_in_annotation(&return_type, &self_ty);
        let body_has_yield = method
            .body
            .as_ref()
            .is_some_and(|body| any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_))));
        if effective_return_type.generator_element_type().is_some()
            && !body_has_yield
            && method.body.as_deref().is_some_and(|body| !body_has_return_value(body))
        {
            self.errors
                .push(errors::generator_requires_yield(&method.name, method.return_type.span));
        }
        let yield_context = if body_has_yield {
            match effective_return_type.generator_element_type() {
                Some(element_ty) => YieldContext::Generator {
                    element_ty: element_ty.clone(),
                },
                None => YieldContext::Disallowed,
            }
        } else {
            YieldContext::Disallowed
        };
        self.symbols.set_return_type(effective_return_type);

        // Set error type for ? checking
        self.current_return_error_type = return_type.result_err_type().cloned();

        // Check body
        if let Some(body) = &method.body {
            let prev_in_async_body = self.in_async_body;
            let prev_yield_context = std::mem::replace(&mut self.current_yield_context, yield_context);
            self.in_async_body = method.is_async();
            let previous_consumed_iterator_bindings = std::mem::take(&mut self.consumed_iterator_bindings);
            for stmt in body {
                self.check_statement(stmt);
            }
            self.consumed_iterator_bindings = previous_consumed_iterator_bindings;
            self.in_async_body = prev_in_async_body;
            self.current_yield_context = prev_yield_context;
        }

        self.current_return_error_type = None;
        self.current_classmethod_self_ty = previous_classmethod_self_ty;
        self.current_type_param_bound_details.pop();
        self.mutable_bindings.remove("self");
        self.symbols.exit_scope();
    }
}
