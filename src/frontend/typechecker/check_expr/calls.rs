//! Check calls, constructors, and builtins.
//!
//! This module keeps the call-expression coordinator (`foo(...)`) thin and delegates argument binding, constructor
//! handling, generic inference, builtin dispatch, and Rust boundary validation to focused child modules.

use crate::frontend::ast::{CallArg, Expr, Span, Spanned, Type};
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::resolved_type_subst::substitute_resolved_type;
use crate::frontend::symbols::{FieldInfo, FunctionInfo, FunctionOverloadInfo, ResolvedType, SymbolKind, TypeInfo};
use crate::frontend::typechecker::IdentKind;
use incan_core::interop::{RustFieldInfo, RustItemKind, RustTypeInfo};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::stdlib;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use std::collections::HashSet;

use super::TypeChecker;

mod args;
mod builtins;
mod constructors;
mod generic_bounds;
mod rust_boundary;

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

impl TypeChecker {
    /// Type-check a call expression after parsing has identified the callee, explicit type arguments, and value
    /// arguments.
    ///
    /// This is the central call coordinator: it preserves constructor and builtin special cases first, then resolves
    /// function values, callable objects, and ordinary value calls through the same argument-binding machinery.
    /// Callable values record their accepted parameter list at the full call span so IR lowering can preserve Rust
    /// borrow boundaries for calls reached through associated-function member access.
    pub(in crate::frontend::typechecker::check_expr) fn check_call(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.check_call_with_expected(callee, type_args, args, span, None)
    }

    /// Type-check a call expression with an optional expected result type.
    ///
    /// Contextual return hints are part of the generic call plan, not a desugaring special case. They let direct
    /// source, vocab-produced AST, and nested call arguments all use the same inference path when a destination
    /// type is known.
    pub(in crate::frontend::typechecker::check_expr) fn check_call_with_expected(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if let Some(name) = Self::explicit_builtin_member_name(callee) {
            let result = self.check_explicit_builtin_call(name, args, span);
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                return ResolvedType::Unknown;
            }
            return result;
        }

        // Special-case: Enum variant constructor syntax `Enum.Variant(...)`.
        // If callee is a field access where the base resolves to a known enum type
        // and the field name matches a variant, treat this as a constructor and
        // return the enum type.
        if let Expr::Field(base, member_name) = &callee.node {
            let base_ty = self.check_type_receiver_expr(base);
            let base_is_enum_type_name = self.is_enum_type_name_expr_for_call(base);
            if let ResolvedType::Named(enum_name) = &base_ty
                && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
                && let Some(value_enum) = enum_info.value_enum.clone()
            {
                if member_name == "from_value" && base_is_enum_type_name {
                    return self.check_value_enum_from_value_call(enum_name, &value_enum, type_args, args, span);
                }
                if member_name == "value" && !base_is_enum_type_name {
                    return self.check_value_enum_value_call(enum_name, &value_enum, type_args, args, span);
                }
            }
            if let ResolvedType::Named(enum_name) = &base_ty
                && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
                && (enum_info.variants.iter().any(|v| v == member_name)
                    || enum_info.variant_aliases.contains_key(member_name))
            {
                if !type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(span));
                }
                self.check_call_args(args);
                return ResolvedType::Named(enum_name.clone());
            }
            if self.receiver_has_computed_property(&base_ty, member_name, span) {
                self.check_call_args(args);
                self.errors.push(errors::property_called_as_method(member_name, span));
                return ResolvedType::Unknown;
            }
        }

        // Imported module function calls whose signatures are known via the stdlib AST cache
        // (for example `math.sqrt(...)`).
        if let Expr::Field(base, method) = &callee.node
            && let Some((module_name, module_path)) = self.imported_module_for_expr(base)
        {
            // Ensure lowering marks the receiver identifier as a module-path binding.
            let _ = self.check_ident(module_name.as_str(), base.span);
            if let Some(func_info) = self.resolve_imported_module_function_member(&module_path, method.as_str()) {
                let callable = format!("{module_name}.{method}");
                self.record_source_target(span, module_path.clone(), method.clone(), "function");
                self.record_source_target(callee.span, module_path, method.clone(), "function");
                return self.validate_stdlib_module_function_call(
                    callable.as_str(),
                    &func_info,
                    type_args,
                    args,
                    span,
                    expected_return_ty,
                );
            }
        }

        if let Expr::Ident(name) = &callee.node {
            if keywords::from_str(name.as_str()) == Some(KeywordId::Cls)
                && self.symbols.lookup(name).is_none()
                && let (Some(owner_name), Some(self_ty)) = (
                    self.current_method_owner.clone(),
                    self.current_classmethod_self_ty.clone(),
                )
            {
                let ctor_fields: Option<std::collections::HashMap<String, FieldInfo>> =
                    self.lookup_type_info(&owner_name).and_then(|info| match info {
                        TypeInfo::Model(m) => Some(m.fields.clone()),
                        TypeInfo::Class(c) => Some(c.fields.clone()),
                        _ => None,
                    });
                if let Some(fields) = ctor_fields {
                    self.record_expr_type(callee.span, self_ty.clone());
                    self.type_info
                        .expressions
                        .ident_kinds
                        .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                    self.check_model_or_class_constructor_call(&owner_name, &fields, args, span);
                    return self_ty;
                }
            }

            let marker_binding_in_scope = self
                .symbols
                .lookup(name)
                .and_then(|id| self.symbols.get(id))
                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Function(_)) && sym.scope == 0);
            if self.testing_marker_import_bindings.contains(name) && marker_binding_in_scope {
                self.check_call_args(args);
                self.errors
                    .push(errors::testing_marker_runtime_call_not_supported(name, span));
                return ResolvedType::Unknown;
            }

            if let Some(result) = self.check_builtin_call(name, args, span) {
                if !type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(span));
                    return ResolvedType::Unknown;
                }
                return result;
            }

            if let Some(sym) = self.lookup_symbol(name).cloned() {
                match sym.kind {
                    SymbolKind::Type(type_info) => {
                        if let Some(target) = self.source_target_for_symbol(name, &SymbolKind::Type(type_info.clone()))
                        {
                            self.record_source_target(
                                callee.span,
                                target.module_path.clone(),
                                target.name.clone(),
                                target.kind.clone(),
                            );
                            self.record_source_target(span, target.module_path, target.name, target.kind);
                        }
                        if let Some(ret) =
                            self.check_type_constructor_hook_call(name, &type_info, type_args, args, span)
                        {
                            self.record_expr_type(callee.span, ResolvedType::Named(name.clone()));
                            self.type_info
                                .expressions
                                .ident_kinds
                                .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                            return ret;
                        }
                        if stdlib::is_graph_constructor_type(name) && args.is_empty() {
                            return self.check_graph_constructor_call(name, &type_info, type_args, args, span);
                        }
                        if let Some(tid) = surface_types::from_str(name) {
                            if !type_args.is_empty() {
                                self.errors
                                    .push(errors::explicit_call_site_type_args_not_supported(span));
                                self.check_call_args(args);
                                return ResolvedType::Unknown;
                            }
                            self.record_expr_type(callee.span, ResolvedType::Named(name.clone()));
                            self.type_info
                                .expressions
                                .ident_kinds
                                .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                                return self.check_json_query_constructor_call(tid, args, span);
                            }
                            if matches!(tid, SurfaceTypeId::Html) {
                                return ResolvedType::Named(surface_types::as_str(tid).to_string());
                            }
                            if matches!(tid, SurfaceTypeId::ValidationError) {
                                return self.check_constructor(name, args, span);
                            }
                        }
                        let explicit_constructor_context =
                            self.explicit_constructor_type_context(name, &type_info, type_args, span);
                        let explicit_constructor_ty = explicit_constructor_context.as_ref().map(|(ty, _)| ty.clone());
                        if let TypeInfo::Model(model) = &type_info
                            && model
                                .derives
                                .iter()
                                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate))
                        {
                            self.check_call_args(args);
                            self.errors
                                .push(errors::validate_derive_disallows_raw_construction(name, span));
                            return ResolvedType::Unknown;
                        }
                        if matches!(type_info, TypeInfo::Newtype(_)) {
                            self.record_expr_type(callee.span, ResolvedType::Named(name.clone()));
                            self.type_info
                                .expressions
                                .ident_kinds
                                .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                            let constructor_ty = self.check_constructor(name, args, span);
                            return explicit_constructor_ty.unwrap_or(constructor_ty);
                        }
                        let ctor_fields = match &type_info {
                            TypeInfo::Model(info) => Some(info.fields.clone()),
                            TypeInfo::Class(info) => Some(info.fields.clone()),
                            _ => None,
                        };
                        let Some(mut fields) = ctor_fields else {
                            return ResolvedType::Unknown;
                        };
                        if let Some((_, type_bindings)) = &explicit_constructor_context {
                            for field in fields.values_mut() {
                                field.ty = substitute_resolved_type(&field.ty, type_bindings);
                            }
                        }
                        let constructor_ty = self.check_model_or_class_constructor_call(name, &fields, args, span);
                        self.record_expr_type(callee.span, ResolvedType::Named(name.clone()));
                        self.type_info
                            .expressions
                            .ident_kinds
                            .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                        return explicit_constructor_ty.unwrap_or(constructor_ty);
                    }
                    SymbolKind::Function(func_info) => {
                        if let Some(target) =
                            self.source_target_for_symbol(name, &SymbolKind::Function(func_info.clone()))
                        {
                            self.record_source_target(
                                callee.span,
                                target.module_path.clone(),
                                target.name.clone(),
                                target.kind.clone(),
                            );
                            self.record_source_target(span, target.module_path, target.name, target.kind);
                        }
                        return self.validate_function_call(
                            name,
                            &func_info,
                            type_args,
                            args,
                            span,
                            expected_return_ty,
                        );
                    }
                    SymbolKind::FunctionOverloads(overloads) => {
                        if let Some(target) =
                            self.source_target_for_symbol(name, &SymbolKind::FunctionOverloads(overloads.clone()))
                        {
                            self.record_source_target(
                                callee.span,
                                target.module_path.clone(),
                                target.name.clone(),
                                target.kind.clone(),
                            );
                            self.record_source_target(span, target.module_path, target.name, target.kind);
                        }
                        return self.validate_function_overload_call(
                            name,
                            &overloads,
                            type_args,
                            args,
                            span,
                            expected_return_ty,
                        );
                    }
                    SymbolKind::RustItem(info) => {
                        if !type_args.is_empty() {
                            self.errors
                                .push(errors::explicit_call_site_type_args_not_supported(span));
                            self.check_call_args(args);
                            return ResolvedType::Unknown;
                        }
                        if let Some(meta) = &info.metadata {
                            match &meta.kind {
                                RustItemKind::Function(sig) => {
                                    let error_count_before = self.errors.len();
                                    let result = self.validate_rust_function_call(info.path.as_str(), sig, args, span);
                                    if self.errors.len() == error_count_before {
                                        self.record_expr_type(
                                            callee.span,
                                            self.resolved_function_type_from_rust_sig_for_owner_path(
                                                sig,
                                                false,
                                                info.path.as_str(),
                                            ),
                                        );
                                        self.type_info
                                            .expressions
                                            .ident_kinds
                                            .insert((callee.span.start, callee.span.end), IdentKind::RustImport);
                                    }
                                    return result;
                                }
                                RustItemKind::Type(type_info) if !type_info.fields.is_empty() => {
                                    let error_count_before = self.errors.len();
                                    let result = self.check_rust_named_field_constructor_call(
                                        info.path.as_str(),
                                        type_info,
                                        args,
                                        span,
                                    );
                                    if self.errors.len() == error_count_before {
                                        self.record_expr_type(callee.span, ResolvedType::RustPath(info.path.clone()));
                                        self.type_info
                                            .expressions
                                            .ident_kinds
                                            .insert((callee.span.start, callee.span.end), IdentKind::RustImport);
                                    }
                                    return result;
                                }
                                RustItemKind::Type(_) => {
                                    self.check_call_args(args);
                                    self.errors
                                        .push(errors::rust_constructor_metadata_unavailable(info.path.as_str(), span));
                                    return ResolvedType::Unknown;
                                }
                                RustItemKind::Unsupported { description } => {
                                    self.check_call_args(args);
                                    self.errors.push(errors::rust_item_shape_not_supported(
                                        info.path.as_str(),
                                        description.as_str(),
                                        span,
                                    ));
                                    return ResolvedType::Unknown;
                                }
                                _ => {
                                    self.check_call_args(args);
                                    self.errors
                                        .push(errors::rust_constructor_metadata_unavailable(info.path.as_str(), span));
                                    return ResolvedType::Unknown;
                                }
                            }
                        } else if rust_path_last_segment_looks_like_type(info.path.as_str()) {
                            return self.check_metadata_free_rust_named_field_constructor_call(
                                info.path.as_str(),
                                args,
                                span,
                            );
                        }
                    }
                    // RFC 042: traits are abstract — reject `TraitName(...)` constructor syntax.
                    SymbolKind::Trait(_) => {
                        self.check_call_args(args);
                        self.errors.push(errors::cannot_instantiate_trait(name, span));
                        return ResolvedType::Unknown;
                    }
                    _ => {}
                }
            }

            let in_scope = self.symbols.lookup(name).is_some();
            if in_scope && let Some(tid) = surface_types::from_str(name) {
                if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                    return self.check_json_query_constructor_call(tid, args, span);
                }
                if matches!(tid, SurfaceTypeId::Html) {
                    return ResolvedType::Named(surface_types::as_str(tid).to_string());
                }
            }

            // Strict validated construction: `@derive(Validate)` models must be constructed via `TypeName.new(...)`.
            if let Some(TypeInfo::Model(m)) = self.lookup_type_info(name)
                && m.derives
                    .iter()
                    .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate))
            {
                // Still typecheck argument expressions for better downstream errors.
                self.check_call_args(args);
                self.errors
                    .push(errors::validate_derive_disallows_raw_construction(name, span));
                return ResolvedType::Unknown;
            }

            // Model/class constructor calls: validate field arguments at the Incan level.
            // NOTE: `lookup_type_info` returns a reference into `self`, so we clone the needed field map to avoid
            // borrow conflicts (we need `&mut self` for validation).
            let ctor_fields: Option<std::collections::HashMap<String, FieldInfo>> =
                self.lookup_type_info(name).and_then(|info| match info {
                    TypeInfo::Model(m) => Some(m.fields.clone()),
                    TypeInfo::Class(c) => Some(c.fields.clone()),
                    _ => None,
                });
            if let Some(fields) = ctor_fields {
                let constructor_ty = self.check_model_or_class_constructor_call(name, &fields, args, span);
                self.record_expr_type(callee.span, ResolvedType::Named(name.clone()));
                self.type_info
                    .expressions
                    .ident_kinds
                    .insert((callee.span.start, callee.span.end), IdentKind::TypeName);
                if in_scope && let Some(tid) = surface_types::from_str(name) {
                    if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                        return self.check_json_query_constructor_call(tid, args, span);
                    }
                    if matches!(tid, SurfaceTypeId::Html) {
                        return ResolvedType::Named(surface_types::as_str(tid).to_string());
                    }
                }
                return constructor_ty;
            }
        }

        if let Expr::Ident(name) = &callee.node
            && !type_args.is_empty()
            && let Some(binding) = self
                .type_info
                .declarations
                .decorated_function_bindings
                .get(name)
                .cloned()
            && !binding.type_params.is_empty()
            && let ResolvedType::Function(params, ret) = binding.ty
        {
            let info = FunctionInfo {
                params,
                return_type: *ret,
                is_async: binding.is_async,
                type_params: binding.type_params,
                type_param_bounds: binding.type_param_bounds,
                type_param_bound_details: binding.type_param_bound_details,
                emitted_name: None,
            };
            return self.validate_function_call(name, &info, type_args, args, span, expected_return_ty);
        }

        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let callee_ty = self.check_expr(callee);

        match callee_ty {
            ResolvedType::Function(params, ret) => {
                let mut type_bindings = std::collections::HashMap::new();
                if let Some(expected) = expected_return_ty {
                    self.infer_type_param_bindings(&ret, expected, &mut type_bindings);
                }
                let resolved_params = Self::substitute_callable_params(&params, &type_bindings);
                let arg_types = self.check_call_arg_types_for_params(args, &resolved_params);
                self.validate_callable_arg_bindings(
                    "<callable>",
                    &resolved_params,
                    args,
                    &arg_types,
                    &mut type_bindings,
                    span,
                );
                let final_params = Self::substitute_callable_params(&resolved_params, &type_bindings);
                self.type_info.record_call_site_callable_params(span, &final_params);
                substitute_resolved_type(&ret, &type_bindings)
            }
            ty if self.is_user_operator_receiver(&ty)
                && !matches!(
                    self.type_info.ident_kind(callee.span),
                    Some(IdentKind::TypeName | IdentKind::Variant | IdentKind::Trait)
                ) =>
            {
                let arg_types = self.check_call_arg_types(args);
                self.resolve_call_dunder(&ty, args, &arg_types, span)
                    .unwrap_or(ResolvedType::Unknown)
            }
            ResolvedType::Named(name) => {
                self.check_call_args(args);
                match self.lookup_symbol(&name).map(|s| &s.kind) {
                    Some(SymbolKind::Type(_)) => self.constructor_result_type(&name),
                    Some(SymbolKind::Variant(info)) => ResolvedType::Named(info.enum_name.clone()),
                    _ => ResolvedType::Unknown,
                }
            }
            _ => {
                self.check_call_args(args);
                ResolvedType::Unknown
            }
        }
    }

    /// Resolve a direct call to a top-level same-name overload set.
    ///
    /// Candidate checking runs through the ordinary function-call validator and rolls back failed candidates.
    /// That keeps overload dispatch on the ordinary checker while preserving argument-shape metadata for lowering.
    fn validate_function_overload_call(
        &mut self,
        func_name: &str,
        overloads: &[FunctionOverloadInfo],
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let baseline_errors = self.errors.clone();
        let baseline_warnings = self.warnings.clone();
        let baseline_type_info = self.type_info.clone();
        let baseline_consumed_iterator_bindings = self.consumed_iterator_bindings.clone();

        let mut matches = Vec::new();
        for overload in overloads {
            self.errors = baseline_errors.clone();
            self.warnings = baseline_warnings.clone();
            self.type_info = baseline_type_info.clone();
            self.consumed_iterator_bindings = baseline_consumed_iterator_bindings.clone();

            let result =
                self.validate_function_call(func_name, &overload.info, type_args, args, span, expected_return_ty);
            if self.errors.len() == baseline_errors.len() {
                matches.push((
                    overload.info.emitted_name.clone(),
                    result,
                    self.errors.clone(),
                    self.warnings.clone(),
                    self.type_info.clone(),
                    self.consumed_iterator_bindings.clone(),
                ));
            }
        }

        match matches.len() {
            1 => {
                let (emitted_name, result, errors, warnings, type_info, consumed_iterator_bindings) = matches.remove(0);
                self.errors = errors;
                self.warnings = warnings;
                self.type_info = type_info;
                self.consumed_iterator_bindings = consumed_iterator_bindings;
                if let Some(emitted_name) = emitted_name {
                    self.type_info.record_selected_function_emitted_name(span, emitted_name);
                }
                result
            }
            0 => {
                self.errors = baseline_errors;
                self.warnings = baseline_warnings;
                self.type_info = baseline_type_info;
                self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
                if let Some(first) = overloads.first() {
                    self.validate_function_call(func_name, &first.info, type_args, args, span, expected_return_ty)
                } else {
                    ResolvedType::Unknown
                }
            }
            _ => {
                self.errors = baseline_errors;
                self.warnings = baseline_warnings;
                self.type_info = baseline_type_info;
                self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
                self.errors.push(CompileError::type_error(
                    format!("Call to overloaded function '{func_name}' is ambiguous"),
                    span,
                ));
                ResolvedType::Unknown
            }
        }
    }

    /// Type-check a call to an imported Rust named-field struct using rust-inspect field metadata.
    fn check_rust_named_field_constructor_call(
        &mut self,
        path: &str,
        type_info: &RustTypeInfo,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let mut selected_fields = Vec::with_capacity(args.len());
        let mut provided = HashSet::new();
        let mut positional_index = 0usize;
        let mut has_shape_error = false;
        let mut emitted_arity_error = false;

        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    let Some(field) = type_info.fields.get(positional_index) else {
                        self.check_expr(expr);
                        if !emitted_arity_error {
                            self.errors
                                .push(errors::builtin_arity(path, type_info.fields.len(), args.len(), span));
                            emitted_arity_error = true;
                        }
                        has_shape_error = true;
                        continue;
                    };
                    positional_index += 1;
                    self.check_rust_struct_field_expr(field, expr);
                    if !provided.insert(field.name.clone()) {
                        self.errors.push(errors::duplicate_constructor_field(
                            path,
                            field.name.as_str(),
                            expr.span,
                        ));
                        has_shape_error = true;
                        continue;
                    }
                    selected_fields.push(field.name.clone());
                }
                CallArg::Named(field_name, expr) => {
                    let Some(field) = Self::rust_field_for_source_name(&type_info.fields, field_name.as_str()) else {
                        self.check_expr(expr);
                        self.errors.push(errors::missing_field(path, field_name, expr.span));
                        has_shape_error = true;
                        continue;
                    };
                    self.check_rust_struct_field_expr(field, expr);
                    if !provided.insert(field.name.clone()) {
                        self.errors.push(errors::duplicate_constructor_field(
                            path,
                            field.name.as_str(),
                            expr.span,
                        ));
                        has_shape_error = true;
                        continue;
                    }
                    selected_fields.push(field.name.clone());
                }
                CallArg::PositionalUnpack(expr) | CallArg::KeywordUnpack(expr) => {
                    self.check_expr(expr);
                    if !emitted_arity_error {
                        self.errors
                            .push(errors::builtin_arity(path, type_info.fields.len(), args.len(), span));
                        emitted_arity_error = true;
                    }
                    has_shape_error = true;
                }
            }
        }

        for field in &type_info.fields {
            if !provided.contains(&field.name) {
                self.errors.push(errors::missing_required_constructor_field(
                    path,
                    field.name.as_str(),
                    span,
                ));
                has_shape_error = true;
            }
        }

        if !has_shape_error {
            self.type_info
                .record_rust_named_field_constructor_fields(span, selected_fields);
        }
        ResolvedType::RustPath(path.to_string())
    }

    /// Type-check a Rust type-shaped constructor when metadata is unavailable but source names every field.
    ///
    /// Metadata is still required for positional construction because field order must come from Rust. Named source
    /// arguments already carry the field names lowering needs, so this path can preserve InQL/Substrait-style protobuf
    /// constructors without emitting invalid tuple calls.
    fn check_metadata_free_rust_named_field_constructor_call(
        &mut self,
        path: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let mut selected_fields = Vec::with_capacity(args.len());
        let mut provided = HashSet::new();
        let mut has_shape_error = false;
        let mut emitted_metadata_error = false;

        for arg in args {
            match arg {
                CallArg::Named(field_name, expr) => {
                    self.check_expr(expr);
                    if !provided.insert(field_name.clone()) {
                        self.errors.push(errors::duplicate_constructor_field(
                            path,
                            field_name.as_str(),
                            expr.span,
                        ));
                        has_shape_error = true;
                        continue;
                    }
                    selected_fields.push(field_name.clone());
                }
                CallArg::Positional(expr) => {
                    self.check_expr(expr);
                    if !emitted_metadata_error {
                        self.errors
                            .push(errors::rust_constructor_metadata_unavailable(path, span));
                        emitted_metadata_error = true;
                    }
                    has_shape_error = true;
                }
                CallArg::PositionalUnpack(expr) | CallArg::KeywordUnpack(expr) => {
                    self.check_expr(expr);
                    if !emitted_metadata_error {
                        self.errors
                            .push(errors::rust_constructor_metadata_unavailable(path, span));
                        emitted_metadata_error = true;
                    }
                    has_shape_error = true;
                }
            }
        }

        if !has_shape_error {
            self.type_info
                .record_rust_named_field_constructor_fields(span, selected_fields);
            return ResolvedType::RustPath(path.to_string());
        }
        ResolvedType::Unknown
    }

    /// Type-check a Rust struct field argument with the metadata-provided target type as context.
    fn check_rust_struct_field_expr(&mut self, field: &RustFieldInfo, expr: &Spanned<Expr>) -> ResolvedType {
        let expected = self.resolved_type_from_rust_shape(&field.type_shape);
        self.check_expr_with_expected(expr, Some(&expected))
    }

    /// Type-check RFC 047 graph direct constructors (`DiGraph[T]()`, `Dag[T]()`, `MultiDiGraph[T]()`).
    fn check_graph_constructor_call(
        &mut self,
        name: &str,
        type_info: &TypeInfo,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        if !args.is_empty() {
            self.errors.push(errors::builtin_arity(name, 0, args.len(), span));
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }

        let type_params = match type_info {
            TypeInfo::Newtype(info) => info.type_params.as_slice(),
            TypeInfo::Class(info) => info.type_params.as_slice(),
            TypeInfo::Model(info) => info.type_params.as_slice(),
            TypeInfo::Enum(info) => info.type_params.as_slice(),
            TypeInfo::TypeAlias | TypeInfo::Builtin => &[],
        };
        if type_args.len() != type_params.len() {
            self.errors.push(errors::explicit_type_arg_arity(
                name,
                type_params.len(),
                type_args.len(),
                span,
            ));
            return ResolvedType::Unknown;
        }

        let resolved_args = type_args
            .iter()
            .map(|ty| self.resolve_type_checked(ty))
            .collect::<Vec<_>>();
        ResolvedType::Generic(name.to_string(), resolved_args)
    }
}
