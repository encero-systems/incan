//! Second-pass declaration checking: validate models, classes, traits, enums, functions, methods.

use crate::frontend::ast::*;
use crate::frontend::ast_walk::any_expr_in_body;
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_method_info, substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::*;
use crate::frontend::testing_markers::{
    TestingFixtureMarkerArgs, TestingMarkerSemantics, load_testing_marker_semantics,
    resolve_testing_fixture_marker_args,
};
use crate::frontend::typechecker::helpers::{dict_ty, list_ty};

use super::{TestingFixtureInfo, TypeChecker, YieldContext};
use incan_core::interop::RustItemKind;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;
use incan_core::lang::stdlib;
use std::collections::{HashMap, HashSet};

/// Structural equality for trait method signatures (RFC 042 diamond / obligation merging).
fn method_infos_identical(a: &MethodInfo, b: &MethodInfo) -> bool {
    a.receiver == b.receiver
        && a.is_async == b.is_async
        && a.has_body == b.has_body
        && a.params == b.params
        && a.return_type == b.return_type
}

fn local_type_for_param(kind: ParamKind, ty: ResolvedType) -> ResolvedType {
    match kind {
        ParamKind::Normal => ty,
        ParamKind::RestPositional => list_ty(ty),
        ParamKind::RestKeyword => dict_ty(ResolvedType::Str, ty),
    }
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
    span: Span,
}

#[derive(Debug, Clone)]
pub(in crate::frontend::typechecker) struct TraitMethodEntry {
    pub method_name: String,
    pub origin_trait: String,
    pub origin_type_args: Vec<ResolvedType>,
    pub info: MethodInfo,
}

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
    resolved.len() == 3 && resolved[0] == "std" && resolved[1] == "testing" && resolved[2] == "fixture"
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
                            .map(|p| self.resolved_param_type_from_rust_display(p.type_display.as_str()))
                            .collect();
                        candidates.push(InteropAdapterSig {
                            name: format!("rust::{}.{name}", path),
                            receiver,
                            params,
                            return_type: self.resolved_type_from_rust_display(method.signature.return_type.as_str()),
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

    /// Collect all methods from an instantiated trait and its supertraits with type arguments applied.
    fn collect_instantiated_trait_method_entries(
        &self,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: &[ResolvedType],
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
                info: method_info.clone(),
            });
        }

        for (supertrait_name, supertrait_args) in &trait_info.supertraits {
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let instantiated = self.instantiate_trait_info(supertrait_info, supertrait_args);
            self.collect_instantiated_trait_method_entries(supertrait_name, &instantiated, supertrait_args, seen, out);
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

    pub(in crate::frontend::typechecker) fn trait_method_entry_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<TraitMethodEntry> {
        if adoption.type_args.is_empty() {
            return self
                .trait_method_info_resolved(&adoption.name, method, ambiguity_span)
                .map(|info| TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: adoption.name.clone(),
                    origin_type_args: Vec::new(),
                    info,
                });
        }
        let root = self.lookup_semantic_trait_info(&adoption.name)?.clone();
        let instantiated = self.instantiate_trait_info(&root, &adoption.type_args);
        let resolved = ResolvedTraitAdoption {
            name: adoption.name.clone(),
            info: instantiated,
            args: adoption.type_args.clone(),
            span: ambiguity_span,
        };
        let entries = self.trait_method_entries_for_adoption(&resolved);
        let matching: Vec<(String, Vec<ResolvedType>, MethodInfo)> = entries
            .into_iter()
            .filter(|entry| entry.method_name == method)
            .map(|entry| (entry.origin_trait, entry.origin_type_args, entry.info))
            .collect();
        let filtered = self.filter_supertrait_dominated_entries(
            matching
                .iter()
                .map(|(origin, _, info)| (origin.clone(), info.clone()))
                .collect(),
        );
        match filtered.as_slice() {
            [] => None,
            [(origin_trait, info)] => {
                let origin_type_args = matching
                    .iter()
                    .find(|(origin, _, candidate)| {
                        origin == origin_trait && self.method_sigs_compatible(candidate, info)
                    })
                    .map(|(_, args, _)| args.clone())
                    .unwrap_or_default();
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: origin_trait.clone(),
                    origin_type_args,
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
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: rest[0].0.clone(),
                    origin_type_args: matching
                        .iter()
                        .find(|(origin, _, candidate)| {
                            origin == &rest[0].0 && self.method_sigs_compatible(candidate, exp0)
                        })
                        .map(|(_, args, _)| args.clone())
                        .unwrap_or_default(),
                    info: exp0.clone(),
                })
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
                if !found_group
                    .iter()
                    .any(|found| self.method_sigs_compatible(method_info, found))
                {
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
    fn validate_cross_trait_method_collisions(&mut self, adoptions: &[ResolvedTraitAdoption]) {
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
                let (left, span) = &origins[0];
                let (right, _) = &origins[1];
                self.errors
                    .push(errors::cross_trait_method_collision(left, right, &method, *span));
            }
        }
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
                    .find(|(obligation_idx, (_, expected))| {
                        !matched_obligations[*obligation_idx] && self.method_sigs_compatible(expected, overload)
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
        self.validate_cross_trait_method_collisions(adoptions);
        self.validate_overloaded_methods_are_trait_backed(type_name, adoptions, method_overloads, method_spans);
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
            Declaration::Alias(_) => {}     // Alias targets are validated during collection.
            Declaration::TypeAlias(_) => {} // Type aliases are transparent; no body to check
            Declaration::Newtype(nt) => self.check_newtype(nt),
            Declaration::Enum(en) => self.check_enum(en),
            Declaration::Function(func) => self.check_function(func),
            Declaration::TestModule(test_module) => self.check_test_module(test_module),
            Declaration::Docstring(_) => {} // Docstrings don't need checking
        }
    }

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

    fn check_const(&mut self, konst: &ConstDecl, span: Span) {
        // RFC 008: const-eval (with cycle detection + category classification).
        self.check_and_resolve_const(konst, span);
    }

    fn check_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let expected_ty = self.resolve_type_checked(&static_decl.ty);
        let value_ty = self.check_expr_with_expected(&static_decl.value, Some(&expected_ty));
        if !self.types_compatible(&value_ty, &expected_ty) {
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

        if let Some(binding) = self.type_info.static_bindings.get_mut(&static_decl.name) {
            binding.is_imported = false;
        } else {
            self.type_info.static_bindings.insert(
                static_decl.name.clone(),
                super::StaticBindingInfo { is_imported: false },
            );
        }

        let _ = span;
    }

    /// Validate a model declaration after collection, including decorators, trait conformance, fields, and methods.
    fn check_model(&mut self, model: &ModelDecl) {
        self.symbols.enter_scope(ScopeKind::Model);

        self.validate_decorators(&model.decorators);
        // Validate @derive decorators
        self.validate_derives(&model.decorators);
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
                let default_ty = self.check_expr(default);
                let field_ty = self.resolve_type_checked(&field.node.ty);
                if !self.types_compatible(&default_ty, &field_ty) {
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
            self.check_method(&method.node, &model.name);
        }

        if has_validate {
            self.check_validate_derive_model(model);
        }

        self.symbols.exit_scope();
    }

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

        self.validate_decorators(&class.decorators);
        // Validate @derive decorators
        self.validate_derives(&class.decorators);

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
                let default_ty = self.check_expr(default);
                let field_ty = self.resolve_type_checked(&field.node.ty);
                if !self.types_compatible(&default_ty, &field_ty) {
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
            self.check_method(&method.node, &class.name);
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
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    self.errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                }
            }
        }
    }

    fn check_trait(&mut self, tr: &TraitDecl) {
        self.symbols.enter_scope(ScopeKind::Trait);

        self.validate_decorators(&tr.decorators);
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
        let prev_trait_name = self.current_trait_name.take();
        let prev_missing_emitted = self.current_trait_missing_requires_emitted.take();
        self.current_trait_requires = Some(requires_map);
        self.current_trait_name = Some(tr.name.clone());

        for method in &tr.methods {
            if method.node.body.is_some() {
                let prev_method_seen = self.current_trait_missing_requires_emitted.take();
                self.current_trait_missing_requires_emitted = Some(std::collections::HashSet::new());
                // Trait default methods are checked against `Self` (the eventual adopter type), not against the trait
                // name itself. This allows default bodies to reference adopter fields (validated at adoption sites via
                // `@requires`).
                let trait_type_params: Vec<String> = tr.type_params.iter().map(|tp| tp.name.clone()).collect();
                self.check_method_with_self_ty(&method.node, ResolvedType::SelfType, &trait_type_params);
                self.current_trait_missing_requires_emitted = prev_method_seen;
            }
        }

        self.current_trait_requires = prev_trait_requires;
        self.current_trait_name = prev_trait_name;
        self.current_trait_missing_requires_emitted = prev_missing_emitted;
        self.symbols.exit_scope();
    }

    /// Validate one newtype or rusttype declaration after collection has registered its symbol.
    fn check_newtype(&mut self, nt: &NewtypeDecl) {
        self.validate_decorators(&nt.decorators);

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
            && let Some(path) = rusttype_path
        {
            self.type_info
                .rusttype_canonical_rust_paths
                .insert(nt.name.clone(), path);
        }
        if !nt.is_rusttype && !nt.interop_edges.is_empty() {
            self.errors
                .push(errors::interop_block_requires_rusttype(&nt.name, nt.underlying.span));
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
                self.check_method(&method.node, &nt.name);
            }
        }
    }

    /// Validate enum decorators, value-enum rules, and variant payload field types.
    fn check_enum(&mut self, en: &EnumDecl) {
        self.symbols.enter_scope(ScopeKind::Block);

        self.validate_decorators(&en.decorators);
        self.validate_derives(&en.decorators);

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

        self.check_value_enum_decl(en);
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
            self.check_method(&method.node, &en.name);
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

    /// Typecheck one function body with its parameters, return type, decorators, and generic bounds in scope.
    fn check_function(&mut self, func: &FunctionDecl) {
        self.symbols.enter_scope(ScopeKind::Function);

        self.validate_decorators(&func.decorators);
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
            self.type_info.testing_fixtures.insert(
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
        // TODO(#146): add async return-type and related validation here via the surface semantics registry — not
        // hardcoded KeywordId checks.

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

        // Check body
        for stmt in &func.body {
            self.check_statement(stmt);
        }

        self.in_async_body = prev_in_async_body;
        self.current_yield_context = prev_yield_context;
        self.current_return_error_type = None;
        self.current_type_param_bound_details.pop();
        self.symbols.exit_scope();
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
                            type_args: bound
                                .type_args
                                .iter()
                                .map(|type_arg| self.resolve_type_checked(type_arg))
                                .collect(),
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Validate a model, class, enum, or newtype method body using the concrete nominal owner as `self`.
    pub(crate) fn check_method(&mut self, method: &MethodDecl, owner: &str) {
        self.validate_decorators(&method.decorators);
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
        self.check_method_with_self_ty(method, owner_self_ty, &owner_type_params);
        self.current_method_owner = previous_owner;
    }

    /// Check a method body with the concrete owner type used for `Self` in annotations and classmethod constructors.
    ///
    /// Generic owners pass `Owner[T, ...]` here so return checking and `cls(...)` constructor calls see the same
    /// generic surface that call sites use. Trait default methods may still pass bare `Self` because their eventual
    /// adopter is resolved later during trait conformance and method-call substitution.
    fn check_method_with_self_ty(&mut self, method: &MethodDecl, self_ty: ResolvedType, owner_type_params: &[String]) {
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: method.receiver,
        });
        self.validate_callable_rest_params(&method.params);
        // TODO(#146): add async return-type and related validation for methods via the surface semantics registry.

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
        let active_bounds = self.type_param_bound_details_from_type_params(&method.type_params);
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
            for stmt in body {
                self.check_statement(stmt);
            }
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
