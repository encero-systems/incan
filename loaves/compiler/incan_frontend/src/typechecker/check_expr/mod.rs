//! Check expressions and resolve their types.
//!
//! This module owns the expression-checking entrypoint (`check_expr`) and delegates to themed submodules for
//! maintainability. Expression checking is error-accumulating: on invalid input it returns [`ResolvedType::Unknown`] so
//! later checks can continue.
//!
//! ## See also
//! - [`super::TypeChecker`]: the main type checker entrypoint.

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::symbols::{
    CallableParam, FieldInfo, FunctionInfo, ResolvedType, SymbolKind, TypeBoundInfo, TypeInfo, VariableInfo,
};
use crate::typechecker::helpers::{decimal_shape, is_frozen_bytes, is_frozen_str};
use incan_lang::lang::keywords;
use incan_lang::lang::types::numerics::{self, NumericFamily, NumericTypeId};
use incan_lang::numeric_values::{IntegerBounds, integer_bounds};
use incan_semantics_core::SurfaceExprTypeCheck;
use std::collections::HashMap;

use super::mut_arguments::MutArgumentCallee;
use super::{IdentKind, TypeChecker};

mod access;
mod basics;
mod builtin_method_args;
mod calls;
mod collections;
mod comps;
mod control_flow;
mod dict_lookups;
mod error_display;
mod list_methods;
mod literal_carriers;
mod match_;
mod match_coverage;
mod ops;
mod printed_form;
mod task_types;

pub(in crate::typechecker) use collections::fill_open_result_parts;

/// The generic callable a local partial names as its target, with what instantiating it needs (RFC 084).
///
/// A local partial is a value, and a value is not generic, so the partial instantiates the target: its presets and its
/// written type arguments give each of the target's type parameters its type argument.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct GenericPartialTarget {
    /// The target as the partial spells it, for diagnostics.
    callee: String,
    /// The target's type parameters, in declaration order.
    type_params: Vec<String>,
    /// The target's declared bounds, by type parameter, as a call checks them.
    bounds: HashMap<String, Vec<String>>,
    /// The target's resolved bounds, by type parameter, as a call checks them.
    bound_details: HashMap<String, Vec<TypeBoundInfo>>,
}

impl GenericPartialTarget {
    /// Describe a generic function named `callee` as a partial target.
    pub(in crate::typechecker) fn of_function(callee: &str, info: &FunctionInfo) -> Self {
        Self {
            callee: callee.to_string(),
            type_params: info.type_params.clone(),
            bounds: info.type_param_bounds.clone(),
            bound_details: info.type_param_bound_details.clone(),
        }
    }
}

impl TypeChecker {
    /// Type-check a local partial expression and return its projected callable type.
    ///
    /// A generic target (a generic function, or the constructor of a generic model, class or newtype) is instantiated
    /// first: the partial's written type arguments seed its type parameters, each preset value binds the ones its
    /// parameter's type names, and a type parameter left unbound is refused, since the partial is a value and a value
    /// is not generic. The instantiated target type is recorded at the target for lowering, and a generic function's
    /// type arguments at the partial for the call the partial makes.
    fn check_partial_expr(&mut self, partial: &PartialExpr, span: Span) -> ResolvedType {
        let method_target = match self.partial_method_target_type(&partial.target) {
            Ok(method_target) => method_target,
            Err(()) => {
                for arg in &partial.args {
                    self.check_expr(&arg.value);
                }
                return ResolvedType::Unknown;
            }
        };
        let (target_ty, generic_target) = match method_target
            .map(|method_ty| (method_ty, None))
            .or_else(|| self.partial_constructor_target_type(&partial.target))
        {
            Some(target) => target,
            None => {
                let previous_span = self
                    .generic_partial_target_span
                    .replace((partial.target.span.start, partial.target.span.end));
                let previous_target = self.generic_partial_target.take();
                let target_ty = self.check_expr(&partial.target);
                let generic_target = std::mem::replace(&mut self.generic_partial_target, previous_target);
                self.generic_partial_target_span = previous_span;
                (target_ty, generic_target)
            }
        };
        if generic_target.is_none() && !partial.type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let ResolvedType::Function(params, ret) = target_ty else {
            self.errors.push(CompileError::type_error(
                "Partial expression target must be a callable value".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        // Calling a stored callable is supported, but constructing another partial from one is not yet representable:
        // the existing callable signature does not retain enough provenance to distinguish a preset captured by the
        // target from one captured by this new partial. Reject this at the source boundary instead of accepting a
        // program that legacy lowering or Body IR must later refuse.
        if let Expr::Ident(name) = &partial.target.node
            && self
                .lookup_symbol(name)
                .is_some_and(|symbol| matches!(&symbol.kind, SymbolKind::Variable(_)))
        {
            self.errors.push(CompileError::type_error(
                "Partial application of a locally stored callable is not supported".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        }

        let Some(projected) = self.project_partial_params("<local partial>", "<callable>", params, &partial.args, span)
        else {
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        // ---- Seed a generic target's type parameters from the written type arguments ----
        let type_params = generic_target
            .as_ref()
            .map(|target| target.type_params.clone())
            .unwrap_or_default();
        let mut bindings = HashMap::new();
        if let Some(target) = &generic_target
            && !partial.type_args.is_empty()
        {
            if partial.type_args.len() != target.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    &target.callee,
                    &target.type_params,
                    &Self::written_type_args(&partial.type_args),
                    span,
                ));
                for arg in &partial.args {
                    self.check_expr(&arg.value);
                }
                return ResolvedType::Unknown;
            }
            let written = partial
                .type_args
                .iter()
                .map(|type_arg| self.resolve_type_checked(type_arg))
                .collect::<Vec<_>>();
            bindings = type_param_subst_map_call_site(&target.type_params, &written);
        }

        // ---- Check each preset against its parameter, binding the type parameters it names ----
        // A written type argument is not revised by a preset: a preset of another type is a mismatch against it.
        let param_named = |name: &str| projected.iter().find(|param| param.name() == Some(name));
        let mut preset_types = Vec::with_capacity(partial.args.len());
        let mut inferred = HashMap::new();
        for arg in &partial.args {
            let expected = param_named(arg.name.as_str())
                .map(|param| substitute_resolved_type(&param.ty, &bindings))
                .filter(|expected| calls::first_open_type_param(expected, &type_params).is_none());
            let actual = self.check_expr_with_expected(&arg.value, expected.as_ref());
            if let Some(param) = param_named(arg.name.as_str()) {
                self.infer_type_param_bindings(&param.ty, &actual, &mut inferred);
            }
            preset_types.push(actual);
        }
        for (type_param, inferred_ty) in inferred {
            bindings.entry(type_param).or_insert(inferred_ty);
        }

        // ---- Instantiate a generic target ----
        let (projected, ret) = match &generic_target {
            Some(target) => {
                let unfixed = target
                    .type_params
                    .iter()
                    .filter(|type_param| {
                        bindings.get(*type_param).is_none_or(|bound| {
                            matches!(bound, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
                                || calls::first_open_type_param(bound, &type_params).is_some()
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !unfixed.is_empty() {
                    for type_param in &unfixed {
                        self.errors.push(errors::local_partial_type_param_unfixed(
                            &target.callee,
                            type_param,
                            span,
                        ));
                    }
                    return ResolvedType::Unknown;
                }
                self.emit_explicit_bound_errors(
                    &target.callee,
                    &target.bounds,
                    &target.bound_details,
                    &bindings,
                    Self::type_argument_origin(&partial.type_args),
                    span,
                );
                let instantiated_params = projected
                    .iter()
                    .map(|param| CallableParam {
                        ty: substitute_resolved_type(&param.ty, &bindings),
                        ..param.clone()
                    })
                    .collect::<Vec<_>>();
                let instantiated_ret = substitute_resolved_type(&ret, &bindings);
                // Lowering reads the target's parameters from its recorded type, and a generic function's type
                // arguments for the call the partial makes.
                if let Some(target_ty) = self.type_info.expr_type(partial.target.span).cloned() {
                    self.record_expr_type(partial.target.span, substitute_resolved_type(&target_ty, &bindings));
                }
                if self.generic_partial_target_names_a_function(&partial.target) {
                    let type_args = target
                        .type_params
                        .iter()
                        .filter_map(|type_param| bindings.get(type_param).cloned())
                        .collect();
                    self.type_info
                        .calls
                        .call_site_monomorph_type_args
                        .insert((span.start, span.end), type_args);
                }
                (instantiated_params, instantiated_ret)
            }
            None => (projected, *ret),
        };

        for (arg, actual) in partial.args.iter().zip(&preset_types) {
            if let Some(param) = projected.iter().find(|param| param.name() == Some(arg.name.as_str()))
                && !self.types_compatible(actual, &param.ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param.ty.to_string(),
                    &actual.to_string(),
                    arg.value.span,
                ));
            }
        }

        // A local partial retains its complete callable signature. Presets become defaulted, name-overrideable
        // slots; `is_partial_preset` preserves the separate positional rule that starts at the residual arguments.
        ResolvedType::Function(Self::local_partial_params(projected, &partial.args), Box::new(ret))
    }

    /// Return the callable type of a local partial's target that is a method of a value (`partial user.label(...)`),
    /// with the receiver's type substituted for `Self` and for the type's own type parameters (RFC 084).
    ///
    /// The receiver is a place: a local, `self`, or a field of one. The partial evaluates it once, when it is built,
    /// and holds its own copy, as a closure holds a local it captures, so a method that changes its receiver (`mut
    /// self`) is refused: its change would reach only the copy. An overloaded method, which RFC 084 does not admit as a
    /// target, and a generic method, whose type arguments nothing could fix, are refused too. Each refusal is reported
    /// here and answers `Err`. Any other target answers `Ok(None)`, for the ordinary target rules. The method's
    /// identity and its resolved name are recorded at the target for lowering, which calls it on the held receiver.
    fn partial_method_target_type(&mut self, target: &Spanned<Expr>) -> Result<Option<ResolvedType>, ()> {
        let Expr::Field(base, member) = &target.node else {
            return Ok(None);
        };
        if !self.partial_receiver_is_a_value_place(base) {
            return Ok(None);
        }
        let receiver_ty = self.check_expr(base);
        let (type_name, receiver_args) = match &receiver_ty {
            ResolvedType::Named(name) => (name.clone(), Vec::new()),
            ResolvedType::Generic(name, args) => (name.clone(), args.clone()),
            _ => return Ok(None),
        };
        let Some((type_params, method, info, overloaded)) =
            self.lookup_semantic_type_info(&type_name).and_then(|type_info| {
                let (type_params, aliases, methods, overloads) = match type_info {
                    TypeInfo::Model(model) => (
                        &model.type_params,
                        Some(&model.method_aliases),
                        &model.methods,
                        &model.method_overloads,
                    ),
                    TypeInfo::Class(class) => (
                        &class.type_params,
                        Some(&class.method_aliases),
                        &class.methods,
                        &class.method_overloads,
                    ),
                    TypeInfo::Newtype(newtype) => (
                        &newtype.type_params,
                        Some(&newtype.method_aliases),
                        &newtype.methods,
                        &newtype.method_overloads,
                    ),
                    TypeInfo::Enum(enum_info) => (
                        &enum_info.type_params,
                        None,
                        &enum_info.methods,
                        &enum_info.method_overloads,
                    ),
                    TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
                };
                let method = aliases
                    .and_then(|aliases| aliases.get(member))
                    .cloned()
                    .unwrap_or_else(|| member.clone());
                let info = methods.get(&method)?.clone();
                let overloaded = overloads.get(&method).is_some_and(|overloads| overloads.len() > 1);
                Some((type_params.clone(), method, info, overloaded))
            })
        else {
            return Ok(None);
        };
        let refusal = if overloaded {
            Some("it is overloaded, and a partial presets one callable")
        } else if !info.type_params.is_empty() {
            Some("it is generic, and nothing the partial writes fixes its type parameters")
        } else if info.receiver == Some(Receiver::Mutable) {
            Some(
                "it takes 'mut self', and the partial holds its own copy of the receiver, so the change would not reach it",
            )
        } else {
            None
        };
        if let Some(reason) = refusal {
            self.errors
                .push(errors::local_partial_method_target_refused(member, reason, target.span));
            return Err(());
        }
        if info.receiver.is_none() {
            return Ok(None);
        }
        let bindings = type_param_subst_map_call_site(&type_params, &receiver_args);
        let params = info
            .params
            .iter()
            .map(|param| CallableParam {
                ty: substitute_resolved_type(
                    &self.substitute_self_in_resolved_type(param.ty.clone(), &receiver_ty),
                    &bindings,
                ),
                ..param.clone()
            })
            .collect::<Vec<_>>();
        let ret = substitute_resolved_type(
            &self.substitute_self_in_resolved_type(info.return_type.clone(), &receiver_ty),
            &bindings,
        );
        let method_ty = ResolvedType::Function(params, Box::new(ret));
        if let Some(identity) = info.identity {
            self.type_info.record_resolved_identity(target.span, identity);
        }
        self.type_info
            .expressions
            .local_partial_method_targets
            .insert((target.span.start, target.span.end), method);
        self.record_expr_type(target.span, method_ty.clone());
        Ok(Some(method_ty))
    }

    /// Return whether a local partial's target base names a value the partial can hold: a local, `self`, or a field
    /// of one, through parentheses. A module, a type name and any other expression are not.
    fn partial_receiver_is_a_value_place(&self, base: &Spanned<Expr>) -> bool {
        match &base.node {
            Expr::SelfExpr => true,
            Expr::Ident(name) => self
                .lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Variable(_))),
            Expr::Field(inner, _) | Expr::Paren(inner) => self.partial_receiver_is_a_value_place(inner),
            _ => false,
        }
    }

    /// Return whether a span is the target of the local partial being checked, where a generic function may be named
    /// as a value for the partial to instantiate.
    pub(in crate::typechecker::check_expr) fn is_generic_partial_target_span(&self, span: Span) -> bool {
        self.generic_partial_target_span == Some((span.start, span.end))
    }

    /// Whether a local partial's target is a generic function, whose call takes the partial's type arguments, rather
    /// than a generic type's constructor, which is built from its fields.
    fn generic_partial_target_names_a_function(&self, target: &Spanned<Expr>) -> bool {
        !matches!(
            self.type_info
                .expressions
                .ident_kinds
                .get(&(target.span.start, target.span.end)),
            Some(IdentKind::TypeName)
        )
    }

    /// Return the constructor of the model, class or newtype a local partial names as its target, as a callable type,
    /// with the type's own type parameters when it is generic.
    ///
    /// RFC 084 makes constructor presets first-class, so `partial Reader(layer=layer)` inside a function presets the
    /// constructor's parameters as the top-level form does. The callable type is recorded at the target for lowering,
    /// with the target marked as a type name. A generic type's constructor returns the type over its own type
    /// parameters, which the partial instantiates.
    fn partial_constructor_target_type(
        &mut self,
        target: &Spanned<Expr>,
    ) -> Option<(ResolvedType, Option<GenericPartialTarget>)> {
        let Expr::Ident(name) = &target.node else {
            return None;
        };
        let symbol_id = self.symbols.lookup(name)?;
        let kind = self.symbols.get(symbol_id)?.kind.clone();
        if !matches!(
            kind,
            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        let (params, return_type, _, type_params, bounds, bound_details) =
            Self::partial_callable_signature_from_kind(std::slice::from_ref(name), kind)?;
        if let Some(identity) = self.symbols.identity_of(symbol_id).cloned() {
            self.type_info.record_resolved_identity(target.span, identity);
        }
        self.type_info
            .expressions
            .ident_kinds
            .insert((target.span.start, target.span.end), IdentKind::TypeName);
        let (return_type, generic_target) = if type_params.is_empty() {
            (return_type, None)
        } else {
            let own_type = ResolvedType::Generic(
                name.clone(),
                type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
            );
            let generic_target = GenericPartialTarget {
                callee: name.clone(),
                type_params,
                bounds,
                bound_details,
            };
            (own_type, Some(generic_target))
        };
        let constructor_ty = ResolvedType::Function(params, Box::new(return_type));
        self.record_expr_type(target.span, constructor_ty.clone());
        Some((constructor_ty, generic_target))
    }

    /// Type-check every interpolated expression of an f-string and refuse a `{value}` part with no printed form
    /// (#1748) or an unsupported format specifier.
    ///
    /// A `{value}` part displays under the rule `print`/`println` arguments and `str(...)` share (see
    /// [`Self::check_display_operand`]); an `Error` adopter with no `__str__` is recorded to render its `message()`
    /// (#1778). A `{value:?}` part asks for the value's structure through `Debug`, and a value with none is refused the
    /// same way (see [`Self::check_debug_operand`]). A part with any other format spec (`{value:x}`) names a specifier
    /// the language does not support and is refused outright.
    fn check_fstring_parts(&mut self, parts: &[FStringPart]) {
        for part in parts {
            let FStringPart::Expr { expr, format } = part else {
                continue;
            };
            let ty = self.check_expr(expr);
            match format {
                FStringFormat::Display => {
                    self.record_error_message_display(expr.span, &ty);
                    self.check_display_operand(errors::DisplayPosition::Interpolation, expr, &ty);
                }
                FStringFormat::Debug => self.check_debug_operand(expr, &ty),
                FStringFormat::Unsupported(spec) => self
                    .errors
                    .push(errors::unsupported_fstring_format_specifier(spec, expr.span)),
            }
        }
    }

    /// Resolve a field by canonical name or alias, returning the canonical name and FieldInfo.
    ///
    /// - `allow_alias`: whether alias lookup is allowed (models only).
    /// - `allow_numeric_alias`: whether numeric spellings like `"1"` may match aliases.
    fn resolve_field_info<'a>(
        &self,
        fields: &'a HashMap<String, FieldInfo>,
        field_name: &str,
        allow_alias: bool,
        allow_numeric_alias: bool,
    ) -> Option<(String, &'a FieldInfo)> {
        if let Some(info) = fields.get(field_name) {
            return Some((field_name.to_string(), info));
        }
        if !allow_alias {
            return None;
        }
        if !allow_numeric_alias && field_name.parse::<usize>().is_ok() {
            return None;
        }
        fields
            .iter()
            .find(|(_, info)| info.alias.as_deref() == Some(field_name))
            .map(|(name, info)| (name.clone(), info))
    }

    /// Return whether one checked field is private outside its declaring nominal type.
    fn private_field_is_inaccessible(&self, type_name: &str, field: &FieldInfo) -> bool {
        let owner = field.owner.as_deref().unwrap_or(type_name);
        field.is_type_private && self.current_method_owner.as_deref() != Some(owner)
    }

    /// Resolve an expression receiver like `math` to an imported module binding path.
    ///
    /// This is intentionally module-kind driven (`SymbolKind::Module`) instead of name-driven so member access does not
    /// require per-module hardcoded registries.
    pub(in crate::typechecker) fn imported_module_for_expr(
        &self,
        expr: &Spanned<Expr>,
    ) -> Option<(String, Vec<String>)> {
        let Expr::Ident(name) = &expr.node else {
            return None;
        };
        let sym = self.lookup_symbol(name)?;
        let SymbolKind::Module(info) = &sym.kind else {
            return None;
        };
        // Rust imports keep their dedicated metadata path and Python modules remain dynamic.
        if info.path.first().is_some_and(|seg| seg == "rust") || info.is_python {
            return None;
        }
        Some((name.clone(), info.path.clone()))
    }

    /// The declaring identity of a callable reached through a module binding (`math.sqrt`), from the module graph or,
    /// for a standard-library module, from the stdlib cache that resolved its signature.
    ///
    /// A stdlib module binding resolves the callable's symbol through the stdlib cache, which the dependency-member
    /// walk does not consult, so the walk alone leaves `import std.math` + `math.sqrt(..)` without the identity that
    /// `from std.math import sqrt` + `sqrt(..)` records. Both spellings select the same declaration and record it.
    fn imported_module_callable_identity(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        self.dependency_member_identity(&ImportPath::simple(module_path.to_vec()), member)
            .or_else(|| self.stdlib_cache.lookup_identity(module_path, member))
    }

    /// Resolve a function member from a stdlib or public-package module binding.
    fn resolve_imported_module_function_member(&mut self, module_path: &[String], member: &str) -> Option<SymbolKind> {
        self.resolve_imported_module_function_member_with_source(module_path, member)
            .map(|(kind, _)| kind)
    }

    /// Resolve an imported callable together with the authored module path used by lowering and codegraph facts.
    fn resolve_imported_module_function_member_with_source(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        if let Some(kind) = self.stdlib_cache.lookup_function_symbol(module_path, member) {
            return Some((kind, module_path.to_vec()));
        }
        if module_path.len() >= 2 && module_path.first().is_some_and(|seg| seg == "pub") {
            let (kind, source_module_path) =
                self.lookup_pub_library_module_symbol_member(&module_path[1], &module_path[2..], member)?;
            return matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
                .then_some((kind, source_module_path));
        }
        let import_path = ImportPath::simple(module_path.to_vec());
        let kind = self.dependency_member_symbol_for_path(&import_path, member)?;
        if !matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)) {
            return None;
        }
        let identity = self.dependency_member_identity(&import_path, member)?;
        let incan_semantics_core::SymbolOrigin::Module(source_module_path) = identity.origin else {
            return None;
        };
        Some((kind, source_module_path))
    }

    /// Resolve a constant reached through an imported standard-library or checked public-package module.
    pub(in crate::typechecker) fn resolve_imported_module_constant_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(VariableInfo, Option<incan_semantics_core::CanonicalSymbolId>)> {
        if let Some(info) = self.stdlib_cache.lookup_constant(module_path, member) {
            let identity = self.stdlib_cache.lookup_identity(module_path, member);
            return Some((info, identity));
        }
        if module_path.len() >= 2 && module_path.first().is_some_and(|seg| seg == "pub") {
            let resolved = self
                .resolve_pub_library_module_symbol_member(&module_path[1], &module_path[2..], member)
                .ok()
                .flatten()?;
            return match resolved.kind {
                SymbolKind::Variable(info) => Some((info, resolved.canonical)),
                SymbolKind::Static(info) => Some((
                    VariableInfo {
                        ty: info.ty,
                        is_mutable: false,
                        is_used: info.is_used,
                    },
                    resolved.canonical,
                )),
                _ => None,
            };
        }
        let kind = self.dependency_member_symbol_for_path(&ImportPath::simple(module_path.to_vec()), member)?;
        let identity = self.dependency_member_identity(&ImportPath::simple(module_path.to_vec()), member);
        match kind {
            SymbolKind::Variable(info) => Some((info, identity)),
            SymbolKind::Static(info) => Some((
                VariableInfo {
                    ty: info.ty,
                    is_mutable: false,
                    is_used: info.is_used,
                },
                identity,
            )),
            _ => None,
        }
    }

    /// Convert function symbol information into a resolved function type.
    fn function_info_to_resolved_function_type(info: &FunctionInfo) -> ResolvedType {
        ResolvedType::Function(info.params.clone(), Box::new(info.return_type.clone()))
    }
    // ========================================================================
    // Expressions
    // ========================================================================

    /// Validate an expression and return its resolved type.
    ///
    /// Dispatches to specialized helpers (`check_call`, `check_binary`, `check_match`, etc.) and accumulates errors.
    /// Returns [`ResolvedType::Unknown`] when the expression is invalid so checking can continue. Once a call has
    /// resolved its callee, its arguments for `mut` parameters whose changes reach the caller are recorded for the
    /// module-level `INCAN-T0117` decision.
    pub fn check_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        self.refuse_mut_params_held_in(expr);
        let ty = match &expr.node {
            Expr::Ident(name) => self.check_ident(name, expr.span),
            Expr::Literal(lit) => self.check_literal(lit, expr.span),
            Expr::SelfExpr => self.check_self(expr.span),
            Expr::Binary(left, op, right) => self.check_binary(left, *op, right, expr.span),
            Expr::Unary(op, operand) => self.check_unary(*op, operand, expr.span),
            Expr::Call(callee, type_args, args) => {
                let ty = self.check_call(callee, type_args, args, expr.span);
                self.record_mut_arguments(MutArgumentCallee::Function(callee), expr.span, args);
                ty
            }
            Expr::Index(base, index) => self.check_index(base, index, expr.span),
            Expr::Slice(base, slice) => self.check_slice(base, slice, expr.span),
            Expr::Field(base, field) => self.check_field(base, field, expr.span),
            Expr::MethodCall(base, method, type_args, args) => {
                let ty = self.check_method_call(base, method, type_args, args, expr.span);
                self.record_mut_arguments(MutArgumentCallee::Method { receiver: base, method }, expr.span, args);
                ty
            }
            Expr::Partial(partial) => self.check_partial_expr(partial, expr.span),
            Expr::Surface(surface_expr) => self.check_surface_expr(surface_expr, expr.span),
            Expr::Try(inner) => self.check_try(inner, expr.span),
            Expr::Match(subject, arms) => self.check_match(subject, arms, expr.span, None),
            Expr::If(if_expr) => self.check_if_expr(if_expr, expr.span),
            Expr::Loop(loop_expr) => self.check_loop_expr(loop_expr, None, expr.span),
            Expr::Generator(generator) => self.check_generator_expr(generator, expr.span),
            Expr::ListComp(comp) => self.check_list_comp(comp, expr.span),
            Expr::DictComp(comp) => self.check_dict_comp(comp, expr.span),
            Expr::Closure(params, body) => self.check_closure(params, body, expr.span),
            Expr::Tuple(elems) => self.check_tuple(elems),
            Expr::List(elems) => self.check_list(elems),
            Expr::Dict(entries) => self.check_dict(entries),
            Expr::Set(elems) => self.check_set(elems),
            Expr::Paren(inner) => self.check_expr(inner),
            Expr::Constructor(name, args) => self.check_constructor(name, args, expr.span),
            Expr::FString(parts) => {
                self.check_fstring_parts(parts);
                ResolvedType::Str
            }
            Expr::Yield(inner) => {
                let context = self.current_yield_context.clone();
                match context {
                    super::YieldContext::Disallowed => {
                        let yield_ty = inner
                            .as_ref()
                            .map(|inner| self.check_expr(inner))
                            .unwrap_or(ResolvedType::Unit);
                        self.errors.push(errors::yield_outside_generator(expr.span));
                        yield_ty
                    }
                    super::YieldContext::Fixture => inner
                        .as_ref()
                        .map(|inner| self.check_expr(inner))
                        .unwrap_or(ResolvedType::Unit),
                    super::YieldContext::Generator { element_ty } => {
                        if let Some(inner) = inner {
                            let yield_ty = self.check_expr_with_expected(inner, Some(&element_ty));
                            if self.types_compatible(&yield_ty, &element_ty) {
                                // A yielded item is written to the generator's element type as a returned value is
                                // to the return type: a narrower numeric is widened to it (RFC 009).
                                self.record_value_destination_if_compatible(inner.span, &yield_ty, &element_ty);
                            } else {
                                self.errors.push(errors::type_mismatch(
                                    &element_ty.to_string(),
                                    &yield_ty.to_string(),
                                    inner.span,
                                ));
                            }
                            yield_ty
                        } else {
                            self.errors.push(errors::generator_yield_requires_value(expr.span));
                            ResolvedType::Unknown
                        }
                    }
                }
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => self.check_range_expr(start, end),
            Expr::VocabBlock(block) => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Vocab expression declaration `{}` reached typechecking before desugaring",
                        block.keyword
                    ),
                    expr.span,
                ));
                ResolvedType::Unknown
            }
            Expr::Embedded(fragment) => self.check_embedded_fragment_expr(fragment),
        };

        // Record for downstream stages (lowering/codegen).
        self.record_expr_type(expr.span, ty.clone());
        self.record_sdk_task_expression_type(expr);
        self.refuse_capturing_callables_held_in(expr);
        ty
    }

    /// Type-check an expression used as a type-owned receiver, such as `Type.method()` or `Enum.Variant`.
    pub fn check_type_receiver_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        self.type_receiver_spans.push((expr.span.start, expr.span.end));
        let ty = self.check_expr(expr);
        self.type_receiver_spans.pop();
        ty
    }

    /// Type-check a type-owned receiver with the destination type inherited from its enclosing method call.
    ///
    /// The caller must establish that the enclosing method returns `Self`; only that contract makes the enclosing
    /// destination a sound expectation for the receiver expression. Keeping the receiver-span marker active preserves
    /// static type and enum-member resolution while contextual generic inference walks an instance method chain.
    pub fn check_type_receiver_expr_with_expected(
        &mut self,
        expr: &Spanned<Expr>,
        expected: &ResolvedType,
    ) -> ResolvedType {
        self.type_receiver_spans.push((expr.span.start, expr.span.end));
        let ty = self.check_expr_with_expected(expr, Some(expected));
        self.type_receiver_spans.pop();
        ty
    }

    /// Return whether a span identifies a type receiver.
    pub fn is_type_receiver_span(&self, span: Span) -> bool {
        self.type_receiver_spans
            .iter()
            .rev()
            .any(|&(start, end)| start == span.start && end == span.end)
    }

    /// Return whether a span identifies a type name being checked against an explicit `Type[T]` destination.
    pub fn is_type_token_value_span(&self, span: Span) -> bool {
        self.type_token_value_spans
            .iter()
            .rev()
            .any(|&(start, end)| start == span.start && end == span.end)
    }

    /// Type-check an expression with an expected destination type when one is already known.
    ///
    /// This is intentionally narrow: only expression forms that benefit from contextual typing without broad inference
    /// changes should use the hint. Calls record their `mut` arguments as [`Self::check_expr`] does.
    pub fn check_expr_with_expected(&mut self, expr: &Spanned<Expr>, expected: Option<&ResolvedType>) -> ResolvedType {
        let errors_before = self.errors.len();
        self.refuse_mut_params_held_in(expr);
        // An integer literal at an `Option` or union destination takes the one numeric type that destination holds.
        let numeric_destination = match (&expr.node, expected) {
            (Expr::Literal(Literal::Int(_)), Some(expected_ty)) => {
                self.integer_literal_numeric_destination(expected_ty)
            }
            (Expr::Unary(UnaryOp::Neg, inner), Some(expected_ty))
                if matches!(inner.node, Expr::Literal(Literal::Int(_))) =>
            {
                self.integer_literal_numeric_destination(expected_ty)
            }
            _ => None,
        };
        let frozen_string_destination = match (&expr.node, expected) {
            (Expr::Literal(Literal::String(_)), Some(expected_ty)) => {
                self.string_literal_frozen_destination(expected_ty)
            }
            _ => None,
        };
        let expected = numeric_destination
            .as_ref()
            .or(frozen_string_destination.as_ref())
            .or(expected);
        let ty = match (&expr.node, expected) {
            (_, Some(ResolvedType::TypeVar(_))) => return self.check_expr(expr),
            (Expr::Paren(inner), Some(expected_ty)) => self.check_expr_with_expected(inner, Some(expected_ty)),
            (Expr::Ident(_), Some(ResolvedType::TypeToken(_))) => {
                self.type_token_value_spans.push((expr.span.start, expr.span.end));
                let ty = self.check_expr(expr);
                self.type_token_value_spans.pop();
                ty
            }
            // None constructs the Option its destination already proves. Retaining that type here also fixes
            // an enclosing intrinsic constructor's payload type without downstream inference.
            (Expr::Literal(Literal::None), Some(expected_ty)) if expected_ty.option_inner_type().is_some() => {
                expected_ty.clone()
            }
            (Expr::Literal(literal @ Literal::Int(value)), _) if value.suffix.is_some() => {
                self.check_literal(literal, expr.span)
            }
            (Expr::Literal(literal @ Literal::Float(value)), _) if value.suffix.is_some() => {
                self.check_literal(literal, expr.span)
            }
            (Expr::Unary(UnaryOp::Neg, inner), _) if matches!(&inner.node, Expr::Literal(Literal::Int(value)) if value.suffix.is_some()) => {
                self.check_unary_with_expected(UnaryOp::Neg, inner, expr.span, expected)
            }
            (Expr::Literal(Literal::Int(_)), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some() =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Unary(UnaryOp::Neg, inner), Some(expected_ty))
                if super::numeric_type_id_for_compat(expected_ty).is_some()
                    && matches!(inner.node, Expr::Literal(Literal::Int(_))) =>
            {
                self.check_int_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Float(_)), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32)
                ) =>
            {
                self.check_float_literal_with_expected(expr, expected_ty)
            }
            (Expr::Literal(Literal::Decimal(_)), Some(expected_ty)) if is_decimal_type(expected_ty) => {
                self.validate_decimal_literal_with_expected(expr, expected_ty);
                expected_ty.clone()
            }
            (Expr::Literal(Literal::String(_)), Some(expected_ty)) if is_frozen_str(expected_ty) => expected_ty.clone(),
            (Expr::Literal(Literal::Bytes(_)), Some(expected_ty)) if is_frozen_bytes(expected_ty) => {
                expected_ty.clone()
            }
            (Expr::Binary(left, op, right), Some(expected_ty)) => {
                self.check_binary_with_expected(left, *op, right, expr.span, Some(expected_ty))
            }
            (Expr::Unary(UnaryOp::Neg, operand), Some(expected_ty))
                if matches!(
                    expected_ty,
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32)
                ) && matches!(operand.node, Expr::Literal(Literal::Float(_))) =>
            {
                self.check_expr_with_expected(operand, Some(expected_ty));
                expected_ty.clone()
            }
            (Expr::Unary(op, operand), Some(expected_ty)) => {
                self.check_unary_with_expected(*op, operand, expr.span, Some(expected_ty))
            }
            (Expr::Try(inner), Some(expected_ty)) => self.check_try_with_expected(inner, expr.span, Some(expected_ty)),
            (Expr::Call(callee, type_args, args), Some(expected_ty)) => {
                let ty = self.check_call_with_expected(callee, type_args, args, expr.span, Some(expected_ty));
                self.record_mut_arguments(MutArgumentCallee::Function(callee), expr.span, args);
                ty
            }
            (Expr::MethodCall(base, method, type_args, args), Some(expected_ty)) => {
                let ty =
                    self.check_method_call_with_expected(base, method, type_args, args, expr.span, Some(expected_ty));
                self.record_mut_arguments(MutArgumentCallee::Method { receiver: base, method }, expr.span, args);
                ty
            }
            (Expr::Tuple(items), Some(ResolvedType::Unit)) if items.is_empty() => ResolvedType::Unit,
            (Expr::Tuple(items), Some(expected_ty)) => self.check_tuple_with_expected(items, expected_ty),
            (Expr::Closure(params, body), Some(ResolvedType::Function(expected_params, expected_ret))) => {
                self.check_closure_with_expected(params, body, expected_params, expected_ret, expr.span)
            }
            (Expr::List(elems), expected_ty) => self.check_list_with_expected(elems, expected_ty),
            (Expr::Dict(entries), expected_ty) => self.check_dict_with_expected(entries, expected_ty),
            (Expr::Set(elems), expected_ty) => self.check_set_with_expected(elems, expected_ty),
            (Expr::Loop(loop_expr), expected_ty) => self.check_loop_expr(loop_expr, expected_ty, expr.span),
            (Expr::Match(subject, arms), Some(expected_ty)) => {
                self.check_match(subject, arms, expr.span, Some(expected_ty))
            }
            _ => return self.check_expr(expr),
        };

        // A literal whose elements already failed to check is not refused a second time for its open type.
        if matches!(expr.node, Expr::List(_) | Expr::Dict(_) | Expr::Tuple(_)) && self.errors.len() == errors_before {
            self.refuse_collection_literal_without_one_member(&ty, expected, expr.span);
        }
        self.record_expr_type(expr.span, ty.clone());
        self.record_sdk_task_expression_type(expr);
        self.refuse_capturing_callables_held_in(expr);
        ty
    }

    /// Typecheck an explicitly suffixed integer token, preserving its named type and validating its signed value.
    pub(in crate::typechecker::check_expr) fn check_suffixed_int_literal(
        &mut self,
        value: &IntLiteral,
        negative: bool,
        span: Span,
    ) -> ResolvedType {
        let Some(target) = value.suffix else {
            return ResolvedType::Int;
        };
        let target_ty = ResolvedType::from_numeric_id(target);
        let fits = match integer_bounds(target) {
            Some(IntegerBounds::Signed { minimum, .. }) if negative => value.magnitude <= minimum.unsigned_abs(),
            Some(IntegerBounds::Signed { maximum, .. }) => value.magnitude <= maximum as u128,
            Some(IntegerBounds::Unsigned { .. }) if negative => false,
            Some(IntegerBounds::Unsigned { maximum }) => value.magnitude <= maximum,
            None if numerics::info_for(target).family == NumericFamily::BinaryFloat => match target {
                NumericTypeId::F32 => (value.magnitude as f32).is_finite(),
                NumericTypeId::F64 => (value.magnitude as f64).is_finite(),
                _ => false,
            },
            None => false,
        };
        if !fits {
            let spelling = if negative {
                format!("-{}", value.repr)
            } else {
                value.repr.clone()
            };
            self.errors.push(CompileError::type_error(
                format!("Numeric literal {spelling} does not fit in {target_ty}"),
                span,
            ));
        }
        target_ty
    }

    /// Typecheck an explicitly suffixed float token in the suffix's binary-float type.
    ///
    /// An `f32` literal must be finite in `f32`. An `f64` literal is a `float`, which holds IEEE infinity, so it takes
    /// every value an unsuffixed float literal takes.
    fn check_suffixed_float_literal(&mut self, value: &FloatLiteral, span: Span) -> ResolvedType {
        let Some(target) = value.suffix else {
            return ResolvedType::Float;
        };
        let target_ty = ResolvedType::from_numeric_id(target);
        let fits = match target {
            NumericTypeId::F32 => value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX),
            NumericTypeId::F64 => true,
            _ => false,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Float literal {} does not fit in {target_ty}", value.repr),
                span,
            ));
        }
        target_ty
    }

    /// Typecheck an integer literal in a known numeric target context.
    fn check_int_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) -> ResolvedType {
        let Some(target) = super::numeric_type_id_for_compat(expected_ty) else {
            return self.check_expr(expr);
        };
        if matches!(target, incan_lang::lang::types::numerics::NumericTypeId::U128) {
            if unsigned_int_literal_magnitude(expr).is_some() {
                return expected_ty.clone();
            }
            self.errors.push(CompileError::type_error(
                format!(
                    "Integer literal does not fit in {expected_ty}; valid range is 0..={}",
                    u128::MAX
                ),
                expr.span,
            ));
            return expected_ty.clone();
        }
        if matches!(
            target,
            incan_lang::lang::types::numerics::NumericTypeId::F32
                | incan_lang::lang::types::numerics::NumericTypeId::F64
        ) {
            let finite = match signed_int_literal_value(expr) {
                Some(value) => match target {
                    incan_lang::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_lang::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                },
                None => unsigned_int_literal_magnitude(expr).is_some_and(|value| match target {
                    incan_lang::lang::types::numerics::NumericTypeId::F32 => (value as f32).is_finite(),
                    incan_lang::lang::types::numerics::NumericTypeId::F64 => (value as f64).is_finite(),
                    _ => false,
                }),
            };
            if !finite {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal does not fit in {expected_ty}"),
                    expr.span,
                ));
            }
            return expected_ty.clone();
        }
        let Some(value) = signed_int_literal_value(expr) else {
            return self.check_expr(expr);
        };
        match integer_bounds(target) {
            Some(IntegerBounds::Signed { minimum, maximum }) if value < minimum || value > maximum => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Integer literal {value} does not fit in {expected_ty}; valid range is {minimum}..={maximum}"
                    ),
                    expr.span,
                ));
            }
            Some(IntegerBounds::Unsigned { maximum }) if value < 0 || (value as u128) > maximum => {
                self.errors.push(CompileError::type_error(
                    format!("Integer literal {value} does not fit in {expected_ty}; valid range is 0..={maximum}"),
                    expr.span,
                ));
            }
            _ => {}
        }
        expected_ty.clone()
    }

    /// Return the numeric type an integer literal takes at an `Option` or union destination: the one numeric type the
    /// destination holds, as `float` for `Option[float]`, `Option[Option[float]]` or `float | str` (#1859).
    ///
    /// A destination that is itself numeric is checked against directly, so it gives nothing here. Neither does one
    /// holding `int`, where the literal is an `int` already, nor one holding two other numeric types (`f32 | f64`),
    /// which does not say which of them the literal is.
    fn integer_literal_numeric_destination(&self, expected: &ResolvedType) -> Option<ResolvedType> {
        if super::numeric_type_id_for_compat(expected).is_some() {
            return None;
        }
        let mut held = Vec::new();
        Self::collect_held_numeric_types(&self.expand_type_aliases(expected.clone()), &mut held);
        match held.as_slice() {
            [only] if !matches!(only, ResolvedType::Int) => Some(only.clone()),
            _ => None,
        }
    }

    /// Return `FrozenStr` when it is the only string storage a union or `Option` destination offers a string literal.
    fn string_literal_frozen_destination(&self, expected: &ResolvedType) -> Option<ResolvedType> {
        let mut held = Vec::new();
        Self::collect_held_string_types(&self.expand_type_aliases(expected.clone()), &mut held);
        matches!(held.as_slice(), [ResolvedType::FrozenStr]).then_some(ResolvedType::FrozenStr)
    }

    /// Collect the distinct owned and frozen string types held inside `Option` layers and union members.
    fn collect_held_string_types(ty: &ResolvedType, held: &mut Vec<ResolvedType>) {
        if matches!(ty, ResolvedType::Str | ResolvedType::FrozenStr) {
            if !held.contains(ty) {
                held.push(ty.clone());
            }
        } else if let Some(inner) = ty.option_inner_type() {
            Self::collect_held_string_types(inner, held);
        } else if let Some(members) = ty.union_members() {
            for member in members {
                Self::collect_held_string_types(member, held);
            }
        }
    }

    /// Collect, without duplicates, the numeric types `ty` is or holds inside `Option` layers and union members.
    fn collect_held_numeric_types(ty: &ResolvedType, held: &mut Vec<ResolvedType>) {
        if super::numeric_type_id_for_compat(ty).is_some() {
            if !held.contains(ty) {
                held.push(ty.clone());
            }
        } else if let Some(inner) = ty.option_inner_type() {
            Self::collect_held_numeric_types(inner, held);
        } else if let Some(members) = ty.union_members() {
            for member in members {
                Self::collect_held_numeric_types(member, held);
            }
        }
    }

    /// Typecheck a binary-float literal in a known `f32` target context, where the literal must be finite in `f32`.
    fn check_float_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) -> ResolvedType {
        let Expr::Literal(Literal::Float(value)) = &expr.node else {
            return self.check_expr(expr);
        };
        let fits = match expected_ty {
            ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32) => {
                value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
            }
            _ => true,
        };
        if !fits {
            self.errors.push(CompileError::type_error(
                format!("Float literal {} does not fit in {expected_ty}", value.repr),
                expr.span,
            ));
        }
        expected_ty.clone()
    }

    /// Reject an out-of-domain float literal nested beneath an `f32` arithmetic destination.
    ///
    /// Binary operands are otherwise checked independently, so the enclosing destination does not reach a literal
    /// such as `1e9999` in `1e9999 + 0.0`. This validation deliberately records no inferred type and does not make
    /// `float` (`f64`) finite-only; it only preserves the `f32` destination's literal-domain check and literal span.
    pub(in crate::typechecker::check_expr) fn validate_exact_float_literals_in_arithmetic(
        &mut self,
        expr: &Spanned<Expr>,
        expected_ty: &ResolvedType,
    ) {
        match &expr.node {
            Expr::Literal(Literal::Float(value)) => {
                let fits = match expected_ty {
                    ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32) => {
                        value.value.is_finite() && value.value.abs() <= f64::from(f32::MAX)
                    }
                    _ => return,
                };
                if !fits {
                    self.errors.push(CompileError::type_error(
                        format!("Float literal {} does not fit in {expected_ty}", value.repr),
                        expr.span,
                    ));
                }
            }
            Expr::Unary(UnaryOp::Neg, inner) | Expr::Paren(inner) => {
                self.validate_exact_float_literals_in_arithmetic(inner, expected_ty);
            }
            Expr::Binary(
                left,
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Pow,
                right,
            ) => {
                self.validate_exact_float_literals_in_arithmetic(left, expected_ty);
                self.validate_exact_float_literals_in_arithmetic(right, expected_ty);
            }
            _ => {}
        }
    }

    /// Validate a decimal literal against a known decimal precision and scale.
    fn validate_decimal_literal_with_expected(&mut self, expr: &Spanned<Expr>, expected_ty: &ResolvedType) {
        let Expr::Literal(Literal::Decimal(value)) = &expr.node else {
            return;
        };
        let Some((precision, scale)) = decimal_precision_scale(expected_ty) else {
            return;
        };
        let Some((integer_digits, fractional_digits, total_digits)) = decimal_literal_digit_counts(value.body.as_str())
        else {
            self.errors.push(CompileError::type_error(
                format!("Decimal literal {} is not a plain decimal literal", value.repr),
                expr.span,
            ));
            return;
        };
        let max_integer_digits = precision - scale;
        if integer_digits > max_integer_digits {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {integer_digits} integer digit(s), but {expected_ty} allows at most {max_integer_digits}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if fractional_digits > scale {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {fractional_digits} fractional digit(s), but {expected_ty} allows at most {scale}"
                    , value.repr
                ),
                expr.span,
            ));
        }
        if total_digits > precision {
            self.errors.push(CompileError::type_error(
                format!(
                    "Decimal literal {} has {total_digits} total digit(s), but {expected_ty} allows at most {precision}",
                    value.repr
                ),
                expr.span,
            ));
        }
    }

    /// Typecheck a surface expression via the semantics registry.
    fn check_surface_expr(&mut self, expr: &SurfaceExpr, span: Span) -> ResolvedType {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_expr_action(&expr.key) else {
            // No pack claimed this surface expression — report as unknown.
            let label = match &expr.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
                incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                } => {
                    format!("{dependency_key}:{descriptor_key}")
                }
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return ResolvedType::Unknown;
        };

        match (action, &expr.payload) {
            (SurfaceExprTypeCheck::AwaitCheck, SurfaceExprPayload::PrefixUnary(inner)) => self.check_await(inner, span),
            (SurfaceExprTypeCheck::RaceForCheck, SurfaceExprPayload::RaceFor(race)) => self.check_race_for(race, span),
            _ => ResolvedType::Unknown,
        }
    }

    /// Typecheck a descriptor-gated embedded-fragment expression (RFC 081, `#1023`).
    ///
    /// Unlike [`TypeChecker::check_surface_expr`], this node is never eliminated by the pre-typecheck vocab
    /// desugar pass (`loaves/compiler/incan_frontend/src/vocab_desugar_pass/rewrite.rs`) — it must reach `check_expr`
    /// as itself, because its [`EmbeddedNode::Hole`] sub-expressions are genuine Incan expressions that need real
    /// types, exactly as if they appeared in ordinary expression position. The surrounding structural content
    /// (tags, selectors, declarations, regex/type shapes, ...) is DSL-owned syntax with no ordinary Incan type of
    /// its own — its runtime meaning is supplied by the owning DSL's desugarer or lowering hook (RFC 081
    /// §Semantics), which is downstream of `#1023`. The fragment as a whole therefore resolves to
    /// `ResolvedType::Unknown`, deliberately reusing the existing "no further ordinary-Incan meaning to check here"
    /// sentinel rather than introducing a new `ResolvedType` variant that every exhaustive match over
    /// `ResolvedType` across the compiler would need to handle for a type with no ordinary-Incan operations anyway.
    fn check_embedded_fragment_expr(&mut self, fragment: &EmbeddedFragmentExpr) -> ResolvedType {
        for hole in fragment.holes() {
            self.check_expr(hole);
        }
        ResolvedType::Unknown
    }
}

/// Return whether a resolved type is one of the parameterized decimal families.
fn is_decimal_type(ty: &ResolvedType) -> bool {
    decimal_shape(ty).is_some()
}

/// Extract precision and scale from a checked resolved decimal type.
fn decimal_precision_scale(ty: &ResolvedType) -> Option<(usize, usize)> {
    decimal_shape(ty).map(|shape| (usize::from(shape.precision), usize::from(shape.scale)))
}

/// Count significant integer digits, fractional digits, and their total in a plain decimal literal body.
fn decimal_literal_digit_counts(body: &str) -> Option<(usize, usize, usize)> {
    if body.contains('e') || body.contains('E') {
        return None;
    }
    let (integer, fractional) = body.split_once('.').unwrap_or((body, ""));
    let integer_digits = integer
        .chars()
        .skip_while(|ch| *ch == '0')
        .filter(|ch| ch.is_ascii_digit())
        .count();
    let fractional_digits = fractional.chars().filter(|ch| ch.is_ascii_digit()).count();
    Some((integer_digits, fractional_digits, integer_digits + fractional_digits))
}

/// Return the signed value represented by an integer literal or unary-negative integer literal.
fn signed_int_literal_value(expr: &Spanned<Expr>) -> Option<i128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => i128::try_from(value.magnitude).ok(),
        Expr::Unary(UnaryOp::Neg, inner) => match &inner.node {
            Expr::Literal(Literal::Int(value)) if value.magnitude == (1_u128 << 127) => Some(i128::MIN),
            Expr::Literal(Literal::Int(value)) => i128::try_from(value.magnitude).ok().map(|value| -value),
            _ => None,
        },
        _ => None,
    }
}

/// Return the unsigned magnitude for a non-negative integer literal.
fn unsigned_int_literal_magnitude(expr: &Spanned<Expr>) -> Option<u128> {
    match &expr.node {
        Expr::Literal(Literal::Int(value)) => Some(value.magnitude),
        _ => None,
    }
}
