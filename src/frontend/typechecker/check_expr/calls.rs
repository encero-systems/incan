//! Check calls, constructors, and builtins.
//!
//! This module keeps the call-expression coordinator (`foo(...)`) thin and delegates argument binding, constructor
//! handling, generic inference, builtin dispatch, and Rust boundary validation to focused child modules.

use crate::frontend::ast::{CallArg, Expr, ParamKind, Span, Spanned, Type};
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::resolved_type_subst::substitute_resolved_type;
use crate::frontend::symbols::{
    CallableParam, FieldInfo, FunctionInfo, FunctionOverloadInfo, ResolvedType, SymbolKind, TypeInfo,
};
use crate::frontend::typechecker::type_info::{
    CAbiOutputSlot, CAbiSpan, CAbiSpanAccess, CAbiSpanAccessKind, CAbiSpanKind, CBindingRawCall, CBindingSymbol,
    CBindingType, COutputMode, CResourceAccess,
};
use crate::frontend::typechecker::{
    CAbiRawCallResult, IdentKind, PendingCAbiOutputSlot, canonical_public_library_type_name,
};
use incan_core::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustTypeInfo, compiler_owned_function_signature,
    metadata_free_function_signature,
};
use incan_core::lang::c_abi;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::keywords::{self, KeywordId};
use incan_core::lang::stdlib;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_core::lang::traits::{self, TraitId};
use std::collections::HashSet;

use super::TypeChecker;

mod args;
mod builtins;
mod constructors;
mod generic_bounds;
mod rust_boundary;

/// Source-facing and canonical identity for one constructor reached through a public package namespace.
pub(super) struct PublicModuleConstructorContext<'a> {
    pub display_name: &'a str,
    pub canonical_name: &'a str,
    pub type_info: &'a TypeInfo,
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
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_c_string_constructor(base, member, type_args, args, span)
        {
            return result;
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_span_constructor(base, member, type_args, args, span)
        {
            return result;
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_output_slot_constructor(base, member, type_args, args, span)
        {
            return result;
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_binding_symbol_member_call(base, member, type_args, args, span)
        {
            return result;
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some((_, module_path)) = self.imported_module_for_expr(base)
            && module_path.len() >= 2
            && module_path.first().is_some_and(|segment| segment == "pub")
            && let Err(source_modules) =
                self.resolve_pub_library_module_symbol_member(&module_path[1], &module_path[2..], member)
        {
            self.errors.push(errors::pub_library_module_member_ambiguous(
                &module_path[1],
                &module_path[2..],
                member,
                &source_modules,
                span,
            ));
            return ResolvedType::Unknown;
        }
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
            let is_public_library_module =
                module_path.len() >= 2 && module_path.first().is_some_and(|segment| segment == "pub");
            let resolved = if is_public_library_module {
                match self.resolve_pub_library_module_symbol_member(&module_path[1], &module_path[2..], method) {
                    Ok(resolved) => resolved.map(|resolved| {
                        (
                            resolved.kind,
                            resolved.source_module_path,
                            resolved.source_name,
                            Some(module_path[1].clone()),
                        )
                    }),
                    Err(source_modules) => {
                        self.errors.push(errors::pub_library_module_member_ambiguous(
                            &module_path[1],
                            &module_path[2..],
                            method,
                            &source_modules,
                            span,
                        ));
                        return ResolvedType::Unknown;
                    }
                }
            } else {
                self.resolve_imported_module_function_member_with_source(&module_path, method.as_str())
                    .map(|(kind, source_module_path)| (kind, source_module_path, method.clone(), None))
            };
            if let Some((kind, source_module_path, source_name, public_library)) = resolved {
                let callable = format!("{module_name}.{method}");
                if is_public_library_module
                    && let Some(projection) = self.lookup_pub_library_module_partial_projection(
                        &module_path[1],
                        &module_path[2..],
                        method,
                        &callable,
                    )
                {
                    self.type_info.record_partial_projection(projection);
                }
                let source_kind = match &kind {
                    SymbolKind::Type(type_info) => Self::source_target_kind_for_type_info(type_info).unwrap_or("type"),
                    _ => "function",
                };
                self.record_source_target(span, source_module_path.clone(), source_name.clone(), source_kind);
                self.record_source_target(
                    callee.span,
                    source_module_path.clone(),
                    source_name.clone(),
                    source_kind,
                );
                return match (kind, public_library) {
                    (SymbolKind::Function(info), _) => self.validate_stdlib_module_function_call(
                        callable.as_str(),
                        &info,
                        type_args,
                        args,
                        span,
                        expected_return_ty,
                    ),
                    (SymbolKind::FunctionOverloads(overloads), _) => self.validate_function_overload_call(
                        callable.as_str(),
                        &overloads,
                        type_args,
                        args,
                        span,
                        expected_return_ty,
                    ),
                    (
                        SymbolKind::Type(type_info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))),
                        Some(library),
                    ) => {
                        let mut source_type_path = source_module_path.iter().skip(2).cloned().collect::<Vec<_>>();
                        source_type_path.push(source_name);
                        let canonical_name = canonical_public_library_type_name(&library, &source_type_path.join("::"));
                        self.check_public_module_type_constructor_call(
                            PublicModuleConstructorContext {
                                display_name: &callable,
                                canonical_name: &canonical_name,
                                type_info: &type_info,
                            },
                            type_args,
                            args,
                            span,
                            callee.span,
                        )
                    }
                    _ => ResolvedType::Unknown,
                };
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
                    self.check_model_or_class_constructor_call(&owner_name, &owner_name, &fields, args, span);
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

            if let Some(result) = self.check_builtin_call(name, args, span, expected_return_ty) {
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
                        let constructor_ty =
                            self.check_model_or_class_constructor_call(name, name, &fields, args, span);
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
                            self.record_source_target(
                                span,
                                target.module_path.clone(),
                                target.name.clone(),
                                target.kind.clone(),
                            );
                            self.record_c_abi_function_call_target(span, target);
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
                            self.record_source_target(
                                span,
                                target.module_path.clone(),
                                target.name.clone(),
                                target.kind.clone(),
                            );
                            self.record_c_abi_function_call_target(span, target);
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
                        if let Some(sig) = compiler_owned_function_signature(info.path.as_str()) {
                            return self.validate_and_record_rust_import_function_call(
                                info.path.as_str(),
                                &sig,
                                args,
                                span,
                                callee.span,
                                expected_return_ty,
                            );
                        }
                        let metadata = info
                            .metadata
                            .clone()
                            .or_else(|| self.rust_item_metadata_for_path_blocking(info.path.as_str()));
                        if let Some(meta) = metadata.as_ref() {
                            match &meta.kind {
                                RustItemKind::Function(sig) => {
                                    return self.validate_and_record_rust_import_function_call(
                                        info.path.as_str(),
                                        sig,
                                        args,
                                        span,
                                        callee.span,
                                        expected_return_ty,
                                    );
                                }
                                RustItemKind::Type(type_info)
                                    if !type_info.fields.is_empty()
                                        || (type_info.variants.is_empty() && args.is_empty()) =>
                                {
                                    let error_count_before = self.errors.len();
                                    let result = if type_info.fields.iter().all(|field| field.name.is_empty()) {
                                        self.check_rust_tuple_struct_constructor_call(
                                            info.path.as_str(),
                                            type_info,
                                            args,
                                            span,
                                        )
                                    } else {
                                        self.check_rust_named_field_constructor_call(
                                            info.path.as_str(),
                                            type_info,
                                            args,
                                            span,
                                        )
                                    };
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
                        } else if let Some(sig) = metadata_free_function_signature(info.path.as_str()) {
                            return self.validate_and_record_rust_import_function_call(
                                info.path.as_str(),
                                &sig,
                                args,
                                span,
                                callee.span,
                                expected_return_ty,
                            );
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
                let constructor_ty = self.check_model_or_class_constructor_call(name, name, &fields, args, span);
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

    /// Resolve an ordinary `Binding.symbol(...)` expression through a checked C binding descriptor.
    ///
    /// This is deliberately an expression-level semantic hook: parser and class syntax remain ordinary Incan. The
    /// executable subset admits scalar and resource contracts, compiler-managed output positions, and a checked
    /// C string temporary for an exact `const char *` parameter. General pointers and structures remain
    /// declaration-checked until later slices provide their own bounded runtime carriers.
    pub(in crate::frontend::typechecker::check_expr) fn check_c_binding_symbol_member_call(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let Expr::Ident(binding_name) = &base.node else {
            return None;
        };
        let descriptor = self.type_info.c_abi.bindings.get(binding_name)?.clone();
        let binding = descriptor.class_name.clone();
        let Some(symbol) = descriptor.symbols.iter().find(|symbol| symbol.name == member).cloned() else {
            self.errors.push(CompileError::type_error(
                format!("C binding `{binding}` does not declare symbol `{member}`"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };

        let callable = format!("{binding}.{member}");
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                format!("C binding symbol `{callable}` requires an enclosing `unsafe:` acknowledgement"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if !Self::c_raw_call_signature_is_currently_supported(&symbol) {
            self.errors.push(CompileError::type_error(
                format!(
                    "C binding symbol `{callable}` requires native ownership or ABI emission that is not implemented yet"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }

        let parameters = symbol
            .parameters
            .iter()
            .map(|parameter| {
                CallableParam::named(
                    parameter.name.clone(),
                    Self::c_raw_call_type(&binding, &parameter.ty),
                    ParamKind::Normal,
                )
            })
            .collect::<Vec<_>>();
        let arg_types = self.check_call_arg_types_for_params(args, &parameters);
        let mut type_bindings = std::collections::HashMap::new();
        self.validate_checked_c_callable_arg_bindings(
            &callable,
            &parameters,
            args,
            &arg_types,
            &mut type_bindings,
            span,
        );
        if !self.validate_c_raw_span_arguments(&symbol, args, span) {
            return Some(ResolvedType::Unknown);
        }
        let output_slots = self.validate_c_raw_output_slots(&binding, &symbol, &descriptor.resources, args);
        self.validate_c_raw_mutable_resource_borrows(&symbol, args);
        self.record_c_raw_owned_resource_transfers(&symbol, args);
        self.type_info.record_call_site_callable_params(span, &parameters);
        let return_type = Self::c_raw_call_return_type(&binding, &symbol.return_type);
        self.type_info.c_abi.raw_calls.push(CBindingRawCall {
            span,
            owner: self.current_c_abi_raw_call_owner.clone(),
            binding: binding.clone(),
            symbol: member.to_string(),
        });
        if let Some(owner) = self.current_c_abi_raw_call_owner.as_ref()
            && owner.visibility == crate::frontend::ast::Visibility::Public
            && self.warned_public_c_abi_raw_call_owners.insert((
                owner.name.clone(),
                owner.declaration_span.start,
                owner.declaration_span.end,
            ))
        {
            self.warnings
                .push(errors::public_checked_c_call_requires_private_bridge(
                    &owner.name,
                    owner.declaration_span,
                ));
        }
        if !output_slots.is_empty() {
            self.unbound_c_abi_raw_call_results.insert(
                (span.start, span.end),
                CAbiRawCallResult {
                    binding,
                    symbol: member.to_string(),
                    local_name: None,
                    local_symbol_span: None,
                    slots_by_parameter: output_slots,
                },
            );
        }
        Some(return_type)
    }

    /// Return whether the contained direct-call bridge can preserve every argument and result contract faithfully.
    fn c_raw_call_signature_is_currently_supported(symbol: &CBindingSymbol) -> bool {
        symbol.parameters.iter().all(|parameter| {
            matches!(parameter.ty, CBindingType::Scalar(_) | CBindingType::Resource { .. })
                || matches!(
                    &parameter.ty,
                    CBindingType::Pointer {
                        mutable: false,
                        pointee,
                    } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar))
                )
                || matches!(
                    &parameter.ty,
                    CBindingType::Pointer { pointee, .. }
                        if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::U8 | c_abi::ScalarTypeId::F32))
                            && symbol.buffers.iter().any(|buffer| {
                                buffer.pointer_parameter == parameter.name
                                    && matches!(pointee.as_ref(), CBindingType::Scalar(element) if *element == buffer.element)
                            })
                )
                || matches!(
                    &parameter.ty,
                    CBindingType::Output { value, .. }
                        if matches!(value.as_ref(), CBindingType::Scalar(_)
                            | CBindingType::Resource { access: CResourceAccess::Owned, .. })
                )
        }) && (matches!(
            &symbol.return_type,
            CBindingType::Scalar(_)
                | CBindingType::Void
                | CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                }
        ) || matches!(
            &symbol.return_type,
            CBindingType::Pointer {
                mutable: false,
                pointee,
            } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar))
        ) || matches!(
            &symbol.return_type,
            CBindingType::Nullable(value)
                if matches!(value.as_ref(), CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                })
        ))
    }

    /// Require every declared typed pointer to be passed with the exact count view from the same opaque span.
    ///
    /// Nominal pointer types alone cannot encode this relation: two independent spans would otherwise have the same
    /// pointer type. The checked declaration owns the parameter pairing and this source-level check preserves it
    /// before lowering sees raw arguments.
    fn validate_c_raw_span_arguments(&mut self, symbol: &CBindingSymbol, args: &[CallArg], span: Span) -> bool {
        for buffer in &symbol.buffers {
            let Some(pointer_parameter) = symbol
                .parameters
                .iter()
                .find(|parameter| parameter.name == buffer.pointer_parameter)
            else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has a missing checked span-pointer parameter `{}`",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            };
            let CBindingType::Pointer { mutable, ref pointee } = pointer_parameter.ty else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has an invalid checked span-pointer parameter `{}`",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            };
            if !matches!(pointee.as_ref(), CBindingType::Scalar(element) if *element == buffer.element) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has a checked span-pointer `{}` whose element contract drifted",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            }
            let (Some(pointer), Some(length)) = (
                Self::c_raw_call_argument(symbol, args, &buffer.pointer_parameter),
                Self::c_raw_call_argument(symbol, args, &buffer.length_parameter),
            ) else {
                // Ordinary argument binding reports omitted or duplicated parameters. Do not replace that diagnostic.
                continue;
            };
            if !Self::c_checked_span_pair_matches(pointer, length, mutable, buffer.element) {
                let (pointer_method, length_method, span_kind) = match (mutable, buffer.element) {
                    (true, c_abi::ScalarTypeId::U8) => {
                        ("as_mut_ptr", "byte_capacity", "mutable caller-owned byte buffer")
                    }
                    (false, c_abi::ScalarTypeId::U8) => ("as_const_ptr", "byte_length", "immutable byte span"),
                    (true, c_abi::ScalarTypeId::F32) => {
                        ("as_mut_ptr", "element_capacity", "mutable caller-owned f32 span")
                    }
                    (false, c_abi::ScalarTypeId::F32) => ("as_const_ptr", "element_count", "immutable f32 span"),
                    _ => ("as_const_ptr", "element_count", "checked C span"),
                };
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` requires `{}` and `{}` to come from the same checked {} via `{}()` and `{}()`",
                        symbol.name,
                        buffer.pointer_parameter,
                        buffer.length_parameter,
                        span_kind,
                        pointer_method,
                        length_method,
                    ),
                    span,
                ));
                return false;
            }
        }
        true
    }

    /// Resolve one C parameter to its bound positional or named source argument without accepting unpacking.
    fn c_raw_call_argument<'a>(
        symbol: &CBindingSymbol,
        args: &'a [CallArg],
        parameter_name: &str,
    ) -> Option<&'a Spanned<Expr>> {
        if let Some(value) = args.iter().find_map(|argument| match argument {
            CallArg::Named(name, value) if name == parameter_name => Some(value),
            _ => None,
        }) {
            return Some(value);
        }
        let position = symbol
            .parameters
            .iter()
            .position(|parameter| parameter.name == parameter_name)?;
        args.iter()
            .filter_map(|argument| match argument {
                CallArg::Positional(value) => Some(value),
                CallArg::Named(_, _) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => None,
            })
            .nth(position)
    }

    /// Recognize the only source spelling that may provide one raw typed pointer and its paired checked bound.
    fn c_checked_span_pair_matches(
        pointer: &Spanned<Expr>,
        length: &Spanned<Expr>,
        mutable: bool,
        element: c_abi::ScalarTypeId,
    ) -> bool {
        let (pointer_receiver, pointer_method, pointer_type_args, pointer_args) = match &pointer.node {
            Expr::MethodCall(receiver, method, type_args, arguments) => (
                receiver.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            _ => return false,
        };
        let (length_receiver, length_method, length_type_args, length_args) = match &length.node {
            Expr::MethodCall(receiver, method, type_args, arguments) => (
                receiver.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            _ => return false,
        };
        let (expected_pointer_method, expected_length_method) = match (mutable, element) {
            (true, c_abi::ScalarTypeId::U8) => ("as_mut_ptr", "byte_capacity"),
            (false, c_abi::ScalarTypeId::U8) => ("as_const_ptr", "byte_length"),
            (true, c_abi::ScalarTypeId::F32) => ("as_mut_ptr", "element_capacity"),
            (false, c_abi::ScalarTypeId::F32) => ("as_const_ptr", "element_count"),
            _ => return false,
        };
        pointer_method == expected_pointer_method
            && length_method == expected_length_method
            && pointer_type_args.is_empty()
            && length_type_args.is_empty()
            && pointer_args.is_empty()
            && length_args.is_empty()
            && Self::c_checked_span_local_name(pointer_receiver)
                .zip(Self::c_checked_span_local_name(length_receiver))
                .is_some_and(|(pointer_local, length_local)| pointer_local == length_local)
    }

    /// Return the local owner of one transparent checked-span receiver expression.
    fn c_checked_span_local_name(expr: &Spanned<Expr>) -> Option<&str> {
        match &expr.node {
            Expr::Ident(name) => Some(name),
            Expr::Paren(inner) => Self::c_checked_span_local_name(inner),
            _ => None,
        }
    }

    /// Map the contained C call surface to the semantic carrier used for argument checking and local values.
    pub(in crate::frontend::typechecker::check_expr) fn c_raw_call_type(
        binding: &str,
        ty: &CBindingType,
    ) -> ResolvedType {
        match ty {
            CBindingType::Scalar(scalar) => c_abi::scalar_numeric_type(*scalar)
                .map(ResolvedType::Numeric)
                .unwrap_or(ResolvedType::Int),
            CBindingType::Void => ResolvedType::Unit,
            CBindingType::Resource { resource, .. } => {
                ResolvedType::Named(Self::c_resource_type_identity(binding, resource))
            }
            CBindingType::Nullable(value) => {
                ResolvedType::Generic("Option".to_string(), vec![Self::c_raw_call_type(binding, value)])
            }
            CBindingType::Pointer { mutable, pointee } => Self::c_pointer_type_identity(*mutable, pointee)
                .map(ResolvedType::Named)
                .unwrap_or(ResolvedType::Unknown),
            CBindingType::Struct(_) | CBindingType::Output { .. } => ResolvedType::Unknown,
        }
    }

    /// Map a raw C result to its source-visible checked carrier.
    ///
    /// Returned text pointers are scoped views, not reusable raw input pointers. The distinction keeps them out of
    /// arbitrary later C calls and admits only the bounded owning conversion supplied below.
    fn c_raw_call_return_type(binding: &str, ty: &CBindingType) -> ResolvedType {
        match ty {
            CBindingType::Pointer {
                mutable: false,
                pointee,
            } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar)) => {
                ResolvedType::Named(c_abi::SCOPED_C_STRING_VIEW_TYPE_ID.to_string())
            }
            _ => Self::c_raw_call_type(binding, ty),
        }
    }

    /// Return one compiler-internal nominal identity for a resource scoped by its binding declaration.
    fn c_resource_type_identity(binding: &str, resource: &str) -> String {
        format!("__incan_c_resource::{binding}::{resource}")
    }

    /// Return the compiler-owned nominal identity for a pointer supported by the direct checked-C bridge.
    fn c_pointer_type_identity(mutable: bool, pointee: &CBindingType) -> Option<String> {
        let CBindingType::Scalar(scalar) = pointee else {
            return None;
        };
        Some(c_abi::pointer_type_identity(
            mutable,
            c_abi::scalar_type_as_str(*scalar),
        ))
    }

    /// Recognize `c.cstr(value)` as the explicit conversion from Incan text to temporary NUL-terminated storage.
    ///
    /// It returns a private compiler-known carrier rather than a raw pointer. The only admitted extraction is
    /// `as_const_ptr()` inside an `unsafe:` region, so the temporary's storage remains live through the raw call.
    pub(super) fn check_c_abi_c_string_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if member != "cstr" {
            return None;
        }
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_aliases
            .get(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }
        if !type_args.is_empty() || args.len() != 1 || !matches!(args.first(), Some(CallArg::Positional(_))) {
            self.errors.push(CompileError::type_error(
                "c.cstr(value) requires exactly one positional str argument and no type arguments".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self
            .check_call_arg_types(args)
            .into_iter()
            .next()
            .unwrap_or(ResolvedType::Unknown);
        if !self.types_compatible(&actual, &ResolvedType::Str) {
            self.errors.push(CompileError::type_error(
                format!("c.cstr(value) requires str, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info.c_abi.uses_checked_c_strings = true;
        Some(ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::Named(c_abi::C_STRING_TYPE_ID.to_string()),
                ResolvedType::Str,
            ],
        ))
    }

    /// Recognize compiler-owned typed carriers used for bounded C span calls.
    ///
    /// Each constructor moves one owned source allocation into a private carrier. The carrier has no ordinary pointer,
    /// indexing, return, or storage surface: only the methods checked below can reach a declared raw C call.
    pub(super) fn check_c_abi_span_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let (identity, kind, value_type, value_label) = match member {
            "bytes_span" => (
                c_abi::C_BYTES_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::U8,
                    mutable: false,
                },
                ResolvedType::Bytes,
                "bytes",
            ),
            "mutable_bytes_span" => (
                c_abi::C_MUTABLE_BYTES_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::U8,
                    mutable: true,
                },
                ResolvedType::Bytes,
                "bytes",
            ),
            "f32_span" => (
                c_abi::C_F32_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::F32,
                    mutable: false,
                },
                Self::c_abi_span_storage_type(c_abi::ScalarTypeId::F32),
                "list[f32]",
            ),
            "mutable_f32_span" => (
                c_abi::C_MUTABLE_F32_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::F32,
                    mutable: true,
                },
                Self::c_abi_span_storage_type(c_abi::ScalarTypeId::F32),
                "list[f32]",
            ),
            _ => return None,
        };
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_aliases
            .get(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }
        let [CallArg::Positional(value)] = args else {
            self.errors.push(CompileError::type_error(
                format!(
                    "c.{member}(value) requires exactly one positional {value_label} argument and no type arguments"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };
        if !type_args.is_empty() {
            self.errors.push(CompileError::type_error(
                format!(
                    "c.{member}(value) requires exactly one positional {value_label} argument and no type arguments"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self.check_expr_with_expected(value, Some(&value_type));
        if !self.types_compatible(&actual, &value_type) {
            self.errors.push(CompileError::type_error(
                format!("c.{member}(value) requires {value_label}, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if !self
            .type_info
            .c_abi
            .spans
            .iter()
            .any(|existing| existing.constructor_span == span)
        {
            self.type_info.c_abi.spans.push(CAbiSpan {
                constructor_span: span,
                kind,
            });
        }
        self.unbound_c_abi_span_constructors
            .entry((span.start, span.end))
            .or_insert(kind);
        Some(ResolvedType::Named(identity.to_string()))
    }

    /// Type-check the closed typed-span bridge surface without admitting a general pointer API.
    pub(super) fn check_c_abi_span_method(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let local_name = Self::c_checked_span_local_name(base)?;
        let local = self.c_abi_span_bindings.get(local_name).copied()?;
        if let Some(consumed_at) = self
            .consumed_c_abi_span_bindings
            .get(&(local.binding_span.start, local.binding_span.end))
            .copied()
        {
            self.errors.push(CompileError::type_error(
                format!(
                    "checked mutable C span `{local_name}` was already consumed at byte range {}..{}; it cannot be used again",
                    consumed_at.start, consumed_at.end
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let mutable = local.kind.mutable;
        let immutable = !mutable;
        let (length_method, capacity_method, finish_method) = match local.kind.element {
            c_abi::ScalarTypeId::U8 => ("byte_length", "byte_capacity", "into_bytes"),
            c_abi::ScalarTypeId::F32 => ("element_count", "element_capacity", "into_f32s"),
            _ => return None,
        };
        let requires_unsafe = matches!(method, "as_const_ptr" | "as_mut_ptr")
            || method == length_method
            || method == capacity_method
            || method == finish_method;
        if requires_unsafe && self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "checked C span bridge operations require an enclosing `unsafe:` acknowledgement".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let (expected_method, access, return_type) = match (immutable, mutable, method) {
            (true, false, "as_const_ptr") => (
                "as_const_ptr",
                CAbiSpanAccessKind::ConstPointer,
                ResolvedType::Named(c_abi::pointer_type_identity(
                    false,
                    c_abi::scalar_type_as_str(local.kind.element),
                )),
            ),
            (true, false, _) if method == length_method => (
                length_method,
                CAbiSpanAccessKind::ElementCount,
                ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::USize),
            ),
            (false, true, "as_mut_ptr") => {
                if let Some(local) = Self::c_checked_span_local_name(base)
                    && !self.mutable_bindings.contains(local)
                {
                    self.errors.push(errors::mutable_c_borrow_requires_mut(local, span));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                (
                    "as_mut_ptr",
                    CAbiSpanAccessKind::MutPointer,
                    ResolvedType::Named(c_abi::pointer_type_identity(
                        true,
                        c_abi::scalar_type_as_str(local.kind.element),
                    )),
                )
            }
            (false, true, _) if method == capacity_method => (
                capacity_method,
                CAbiSpanAccessKind::ElementCapacity,
                ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::USize),
            ),
            (false, true, _) if method == finish_method => {
                if !type_args.is_empty() || args.len() != 1 || !matches!(args.first(), Some(CallArg::Positional(_))) {
                    self.errors.push(CompileError::type_error(
                        format!("a checked mutable C span requires {finish_method}(written) with one positional usize"),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                let actual = self
                    .check_call_arg_types(args)
                    .into_iter()
                    .next()
                    .unwrap_or(ResolvedType::Unknown);
                let count_type = ResolvedType::Numeric(incan_core::lang::types::numerics::NumericTypeId::USize);
                if !self.types_compatible(&actual, &count_type)
                    && !Self::checked_c_integer_bridge_compatible(&actual, &count_type)
                {
                    self.errors.push(CompileError::type_error(
                        format!("{finish_method}(written) requires an integer count, found {actual}"),
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                self.type_info.c_abi.uses_checked_c_span_buffers = true;
                self.record_c_abi_span_access(span, local.kind, CAbiSpanAccessKind::Finish);
                self.consumed_c_abi_span_bindings
                    .insert((local.binding_span.start, local.binding_span.end), span);
                return Some(ResolvedType::Generic(
                    "Result".to_string(),
                    vec![Self::c_abi_span_storage_type(local.kind.element), ResolvedType::Str],
                ));
            }
            _ => return None,
        };
        if method != expected_method || !type_args.is_empty() || !args.is_empty() {
            self.errors.push(CompileError::type_error(
                format!("a checked C span does not support `{method}` with type or value arguments"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        self.record_c_abi_span_access(span, local.kind, access);
        Some(return_type)
    }

    /// Retain one authorized span-carrier operation exactly once so lowering need not rediscover it from source text.
    fn record_c_abi_span_access(&mut self, span: Span, span_kind: CAbiSpanKind, access: CAbiSpanAccessKind) {
        if self
            .type_info
            .c_abi
            .span_accesses
            .iter()
            .any(|existing| existing.span == span && existing.access == access)
        {
            return;
        }
        self.type_info.c_abi.span_accesses.push(CAbiSpanAccess {
            span,
            span_kind,
            access,
        });
    }

    /// Return the ordinary source storage moved into one exact checked C span representation.
    fn c_abi_span_storage_type(element: c_abi::ScalarTypeId) -> ResolvedType {
        match element {
            c_abi::ScalarTypeId::U8 => ResolvedType::Bytes,
            c_abi::ScalarTypeId::F32 => ResolvedType::Generic(
                "List".to_string(),
                vec![ResolvedType::Numeric(
                    incan_core::lang::types::numerics::NumericTypeId::F32,
                )],
            ),
            _ => ResolvedType::Unknown,
        }
    }

    /// Type-check the sole raw extraction admitted for a validated temporary C string.
    pub(super) fn check_c_abi_c_string_pointer(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if method != "as_const_ptr"
            || !matches!(self.check_expr(base), ResolvedType::Named(identity) if identity == c_abi::C_STRING_TYPE_ID)
        {
            return None;
        }
        if !type_args.is_empty() || !args.is_empty() {
            self.errors.push(CompileError::type_error(
                "a checked C string pointer takes no type or value arguments".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "extracting a checked C string pointer requires an enclosing `unsafe:` acknowledgement".to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        Some(ResolvedType::Named(c_abi::pointer_type_identity(false, "c.c_char")))
    }

    /// Type-check the only owning conversion admitted for one returned scoped C string view.
    ///
    /// A caller must name a positive upper bound because a foreign terminator scan has no safe unbounded form. The
    /// helper validates UTF-8 and copies before the raw view can escape the unsafe bridge.
    pub(super) fn check_c_abi_scoped_c_string_copy(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if method != "copy_utf8"
            || !matches!(self.check_expr(base), ResolvedType::Named(identity) if identity == c_abi::SCOPED_C_STRING_VIEW_TYPE_ID)
        {
            return None;
        }
        let [CallArg::Named(name, _)] = args else {
            self.errors.push(CompileError::type_error(
                "a scoped C string view requires copy_utf8(max_bytes=<positive int>)".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };
        if !type_args.is_empty() || name != "max_bytes" {
            self.errors.push(CompileError::type_error(
                "a scoped C string view requires copy_utf8(max_bytes=<positive int>)".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "copying a scoped C string view requires an enclosing `unsafe:` acknowledgement".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self
            .check_call_arg_types(args)
            .into_iter()
            .next()
            .unwrap_or(ResolvedType::Unknown);
        if !self.types_compatible(&actual, &ResolvedType::Int) {
            self.errors.push(CompileError::type_error(
                format!("copy_utf8(max_bytes=...) requires int, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info.c_abi.uses_scoped_c_string_views = true;
        Some(ResolvedType::Generic(
            "Result".to_string(),
            vec![ResolvedType::Str, ResolvedType::Str],
        ))
    }

    /// Recognize ordinary `c.out[...]()` and `c.inout(...)` calls without adding parser grammar.
    ///
    /// The local name is supplied by the enclosing assignment, after which a checked raw symbol binds the temporary
    /// handle to one exact output parameter. The temporary itself has no user-visible runtime representation.
    pub(super) fn check_c_abi_output_slot_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_aliases
            .get(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }

        let slot = match member {
            "out" => {
                if type_args.len() != 1 || !args.is_empty() {
                    self.errors.push(CompileError::type_error(
                        "c.out[...] requires exactly one C value type and no value arguments".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                PendingCAbiOutputSlot {
                    constructor_span: span,
                    mode: COutputMode::Out,
                    declared_type: Some(type_args[0].node.to_string()),
                    initial_type: None,
                    bound: false,
                }
            }
            "inout" => {
                if !type_args.is_empty() || args.len() != 1 {
                    self.errors.push(CompileError::type_error(
                        "c.inout(value) requires exactly one ordinary value argument and no type arguments".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                let CallArg::Positional(value) = &args[0] else {
                    self.errors.push(CompileError::type_error(
                        "c.inout(value) accepts one positional value argument".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                };
                PendingCAbiOutputSlot {
                    constructor_span: span,
                    mode: COutputMode::InOut,
                    declared_type: None,
                    initial_type: Some(self.check_expr(value)),
                    bound: false,
                }
            }
            _ => return None,
        };

        self.unbound_c_abi_output_slot_constructors
            .insert((span.start, span.end), slot);
        Some(ResolvedType::Unknown)
    }

    /// Bind ordinary local output handles to exact checked C output parameters.
    fn validate_c_raw_output_slots(
        &mut self,
        binding: &str,
        symbol: &CBindingSymbol,
        resources: &[crate::frontend::typechecker::CBindingResource],
        args: &[CallArg],
    ) -> std::collections::HashMap<String, String> {
        let resource_names = resources
            .iter()
            .map(|resource| resource.name.clone())
            .collect::<HashSet<_>>();
        let struct_names = HashSet::new();
        let mut next_positional = 0usize;
        let mut slots_by_parameter = std::collections::HashMap::new();
        let mut seen_slots = HashSet::new();

        for arg in args {
            let (parameter, expr) = match arg {
                CallArg::Positional(expr) => {
                    let parameter = symbol.parameters.get(next_positional);
                    next_positional += 1;
                    (parameter, expr)
                }
                CallArg::Named(name, expr) => {
                    (symbol.parameters.iter().find(|parameter| parameter.name == *name), expr)
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => continue,
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let CBindingType::Output { mode, value } = &parameter.ty else {
                continue;
            };
            let Expr::Ident(local_name) = &expr.node else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C output parameter `{binding}.{}.{}` requires a local c.out[...]() or c.inout(...) slot",
                        symbol.name, parameter.name
                    ),
                    expr.span,
                ));
                continue;
            };
            let Some(slot) = self.pending_c_abi_output_slots.get(local_name).cloned() else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C output parameter `{binding}.{}.{}` requires local `{local_name}` to be initialized by c.out[...]() or c.inout(...)",
                        symbol.name, parameter.name
                    ),
                    expr.span,
                ));
                continue;
            };
            if slot.bound {
                self.errors.push(CompileError::type_error(
                    format!("C output slot `{local_name}` is already bound to an earlier raw call"),
                    expr.span,
                ));
                continue;
            }
            if slot.mode != *mode {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C output parameter `{binding}.{}.{}` requires {}, but `{local_name}` was created as {}",
                        symbol.name,
                        parameter.name,
                        if matches!(mode, COutputMode::Out) {
                            "c.out[...]()"
                        } else {
                            "c.inout(...)"
                        },
                        if matches!(slot.mode, COutputMode::Out) {
                            "c.out[...]()"
                        } else {
                            "c.inout(...)"
                        },
                    ),
                    expr.span,
                ));
                continue;
            }
            if !seen_slots.insert(local_name.clone()) {
                self.errors.push(CompileError::type_error(
                    format!("C output slot `{local_name}` may be passed to only one output parameter per call"),
                    expr.span,
                ));
                continue;
            }

            let value_is_valid = match mode {
                COutputMode::Out => {
                    slot.declared_type
                        .as_deref()
                        .and_then(|source| Self::c_binding_type(source, &resource_names, &struct_names, false))
                        == Some((**value).clone())
                }
                COutputMode::InOut => slot
                    .initial_type
                    .as_ref()
                    .is_some_and(|initial| self.types_compatible(initial, &Self::c_raw_call_type(binding, value))),
            };
            if !value_is_valid {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C output slot `{local_name}` does not match `{binding}.{}.{}`'s checked value contract",
                        symbol.name, parameter.name
                    ),
                    expr.span,
                ));
                continue;
            }

            let slot_identity = c_abi::output_slot_type_identity(
                binding,
                &symbol.name,
                &parameter.name,
                slot.constructor_span.start,
                slot.constructor_span.end,
            );
            if let Some(symbol_id) = self.symbols.lookup(local_name)
                && let Some(symbol) = self.symbols.get_mut(symbol_id)
                && let SymbolKind::Variable(variable) = &mut symbol.kind
            {
                variable.ty = ResolvedType::Named(slot_identity.clone());
            }
            if let Some(pending) = self.pending_c_abi_output_slots.get_mut(local_name) {
                pending.bound = true;
            }
            if matches!(mode, COutputMode::InOut)
                && symbol
                    .outcomes
                    .iter()
                    .all(|outcome| !outcome.invalidates.contains(&parameter.name))
            {
                self.available_c_abi_output_slots.insert(slot_identity.clone());
            }
            self.type_info.c_abi.output_slots.push(CAbiOutputSlot {
                constructor_span: slot.constructor_span,
                identity: slot_identity.clone(),
                local_name: local_name.clone(),
                binding: binding.to_string(),
                symbol: symbol.name.clone(),
                parameter: parameter.name.clone(),
                mode: *mode,
                value: (**value).clone(),
            });
            slots_by_parameter.insert(parameter.name.clone(), slot_identity);
        }
        slots_by_parameter
    }

    /// Record local handle transfers for direct raw C calls.
    fn record_c_raw_owned_resource_transfers(&mut self, symbol: &CBindingSymbol, args: &[CallArg]) {
        self.for_each_c_raw_resource_argument(symbol, args, CResourceAccess::Owned, |this, name, span| {
            this.transferred_c_resource_bindings.insert(name.to_string(), span);
        });
    }

    /// Reject an immutable local binding when a direct C call requires exclusive access to its resource wrapper.
    fn validate_c_raw_mutable_resource_borrows(&mut self, symbol: &CBindingSymbol, args: &[CallArg]) {
        self.for_each_c_raw_resource_argument(symbol, args, CResourceAccess::BorrowedMut, |this, name, span| {
            if !this.mutable_bindings.contains(name) {
                this.errors.push(errors::mutable_c_borrow_requires_mut(name, span));
            }
        });
    }

    /// Apply one ownership-mode check to each direct local resource argument in a fixed C call signature.
    fn for_each_c_raw_resource_argument(
        &mut self,
        symbol: &CBindingSymbol,
        args: &[CallArg],
        expected_access: CResourceAccess,
        mut apply: impl FnMut(&mut Self, &str, Span),
    ) {
        let mut next_positional = 0usize;
        for arg in args {
            let (parameter, expr) = match arg {
                CallArg::Positional(expr) => {
                    let parameter = symbol.parameters.get(next_positional);
                    next_positional += 1;
                    (parameter, expr)
                }
                CallArg::Named(name, expr) => {
                    (symbol.parameters.iter().find(|parameter| parameter.name == *name), expr)
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => continue,
            };
            let Some(CBindingType::Resource { access, .. }) = parameter.map(|parameter| &parameter.ty) else {
                continue;
            };
            if *access != expected_access {
                continue;
            }
            let Some((name, resource_span)) = Self::c_raw_resource_ident(expr) else {
                continue;
            };
            apply(self, name, resource_span);
        }
    }

    /// Return the source binding named by a transparent parenthesized resource expression.
    fn c_raw_resource_ident(expr: &Spanned<Expr>) -> Option<(&str, Span)> {
        match &expr.node {
            Expr::Ident(name) => Some((name, expr.span)),
            Expr::Paren(inner) => Self::c_raw_resource_ident(inner),
            _ => None,
        }
    }

    /// Validate a constructor reached through a checked public module namespace while retaining its provider-owned
    /// nominal identity.
    pub(in crate::frontend::typechecker::check_expr) fn check_public_module_type_constructor_call(
        &mut self,
        context: PublicModuleConstructorContext<'_>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Span,
    ) -> ResolvedType {
        let PublicModuleConstructorContext {
            display_name,
            canonical_name: type_name,
            type_info,
        } = context;
        if let Some(ret) = self.check_type_constructor_hook_call(type_name, type_info, type_args, args, span) {
            self.record_expr_type(callee_span, ret.clone());
            self.type_info
                .expressions
                .ident_kinds
                .insert((callee_span.start, callee_span.end), IdentKind::TypeName);
            return ret;
        }

        let explicit_context = self.explicit_constructor_type_context(type_name, type_info, type_args, span);
        let explicit_ty = explicit_context.as_ref().map(|(ty, _)| ty.clone());
        if let TypeInfo::Model(model) = type_info
            && model
                .derives
                .iter()
                .any(|derive| derives::from_str(derive.as_str()) == Some(DeriveId::Validate))
        {
            self.check_call_args(args);
            self.errors
                .push(errors::validate_derive_disallows_raw_construction(display_name, span));
            return ResolvedType::Unknown;
        }

        let constructor_ty = match type_info {
            TypeInfo::Newtype(newtype) => {
                let [CallArg::Positional(value)] = args else {
                    self.check_call_args(args);
                    self.errors.push(errors::newtype_constructor_shape(display_name, span));
                    return explicit_ty.unwrap_or_else(|| self.constructor_result_type(type_name));
                };
                let value_ty = self.check_expr_with_expected(value, Some(&newtype.underlying));
                if !self.types_compatible(&value_ty, &newtype.underlying) {
                    self.errors.push(errors::type_mismatch(
                        &newtype.underlying.to_string(),
                        &value_ty.to_string(),
                        value.span,
                    ));
                }
                self.constructor_result_type(type_name)
            }
            TypeInfo::Model(model) => {
                let mut fields = model.fields.clone();
                if let Some((_, type_bindings)) = &explicit_context {
                    for field in fields.values_mut() {
                        field.ty = substitute_resolved_type(&field.ty, type_bindings);
                    }
                }
                self.check_model_or_class_constructor_call(type_name, display_name, &fields, args, span)
            }
            TypeInfo::Class(class) => {
                let mut fields = class.fields.clone();
                if let Some((_, type_bindings)) = &explicit_context {
                    for field in fields.values_mut() {
                        field.ty = substitute_resolved_type(&field.ty, type_bindings);
                    }
                }
                self.check_model_or_class_constructor_call(type_name, display_name, &fields, args, span)
            }
            _ => ResolvedType::Unknown,
        };
        let result = explicit_ty.unwrap_or(constructor_ty);
        self.record_expr_type(callee_span, result.clone());
        self.type_info
            .expressions
            .ident_kinds
            .insert((callee_span.start, callee_span.end), IdentKind::TypeName);
        result
    }

    /// Resolve a direct call to a top-level same-name overload set.
    ///
    /// Candidate checking runs through the ordinary function-call validator and rolls back failed candidates.
    /// That keeps overload dispatch on the ordinary checker while preserving argument-shape metadata for lowering.
    pub(in crate::frontend::typechecker) fn validate_function_overload_call(
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
                let shape_match = overloads
                    .iter()
                    .find(|overload| Self::function_call_shape_accepts(&overload.info, type_args, args));
                if let Some(candidate) = shape_match.or_else(|| overloads.first()) {
                    self.validate_function_call(func_name, &candidate.info, type_args, args, span, expected_return_ty)
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

    /// Return whether one overload accepts the supplied generic and value-argument counts.
    fn function_call_shape_accepts(
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
    ) -> bool {
        if !explicit_type_args.is_empty() && explicit_type_args.len() != info.type_params.len() {
            return false;
        }
        let normal_params = info
            .params
            .iter()
            .filter(|param| param.kind == ParamKind::Normal)
            .collect::<Vec<_>>();
        let required = normal_params.iter().filter(|param| !param.has_default).count();
        let accepts_extra = info
            .params
            .iter()
            .any(|param| matches!(param.kind, ParamKind::RestPositional | ParamKind::RestKeyword));
        args.len() >= required && (accepts_extra || args.len() <= normal_params.len())
    }

    /// Return whether inspected type metadata proves omitted named fields can be filled through `Default`.
    fn rust_type_supports_default_field_fill(type_info: &RustTypeInfo) -> bool {
        let is_default = |path: &str| {
            traits::rust_paths(TraitId::Default).contains(&path) || path == traits::as_str(TraitId::Default)
        };
        type_info
            .implemented_traits
            .iter()
            .any(|implemented| !implemented.mutable_reference && is_default(implemented.path.as_str()))
            || type_info
                .expanded_derive_traits
                .iter()
                .any(|implemented| is_default(implemented.path.as_str()))
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
                    let arg_ty = self.check_rust_struct_field_expr(path, field, expr);
                    self.validate_rust_boundary_value(path, field.type_display.as_str(), expr, &arg_ty, true);
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
                    let arg_ty = self.check_rust_struct_field_expr(path, field, expr);
                    self.validate_rust_boundary_value(path, field.type_display.as_str(), expr, &arg_ty, true);
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

        let supports_default_fill = Self::rust_type_supports_default_field_fill(type_info);
        let mut omitted_fields = false;
        for field in &type_info.fields {
            if !provided.contains(&field.name) {
                omitted_fields = true;
                if !supports_default_fill {
                    self.errors.push(errors::missing_required_constructor_field(
                        path,
                        field.name.as_str(),
                        span,
                    ));
                    has_shape_error = true;
                }
            }
        }

        if !has_shape_error {
            self.type_info
                .record_rust_named_field_constructor_fields(span, selected_fields);
            if omitted_fields {
                self.type_info.record_rust_named_field_constructor_fills_defaults(span);
            }
        }
        ResolvedType::RustPath(path.to_string())
    }

    /// Type-check a positional imported Rust tuple-struct constructor using rust-inspect field metadata.
    ///
    /// Empty field labels are the shared metadata representation for tuple positions. Keeping that distinction in
    /// metadata lets lowering emit `Type(value)` instead of incorrectly treating the type as an unconstructible
    /// named-field record.
    fn check_rust_tuple_struct_constructor_call(
        &mut self,
        path: &str,
        type_info: &RustTypeInfo,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let mut has_shape_error = false;
        let mut positional_index = 0usize;
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
                    let arg_ty = self.check_rust_struct_field_expr(path, field, expr);
                    self.validate_rust_boundary_value(path, field.type_display.as_str(), expr, &arg_ty, true);
                }
                CallArg::Named(_, expr) | CallArg::PositionalUnpack(expr) | CallArg::KeywordUnpack(expr) => {
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

        if positional_index != type_info.fields.len() && !emitted_arity_error {
            self.errors
                .push(errors::builtin_arity(path, type_info.fields.len(), args.len(), span));
            has_shape_error = true;
        }

        if !has_shape_error {
            self.type_info
                .record_rust_named_field_constructor_fields(span, vec![String::new(); type_info.fields.len()]);
        }
        ResolvedType::RustPath(path.to_string())
    }

    /// Type-check a Rust type-shaped constructor when metadata is unavailable but source names every field.
    ///
    /// Metadata is still required for positional construction because field order must come from Rust. Named source
    /// arguments already carry the field names lowering needs, so this path can preserve IncQL/Substrait-style protobuf
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

    /// Validate one Rust import call and record its callable semantics when validation succeeds.
    fn validate_and_record_rust_import_function_call(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
        callee_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let error_count_before = self.errors.len();
        let result = self.validate_rust_function_call_with_expected(path, signature, args, span, expected_return_ty);
        if self.errors.len() == error_count_before {
            self.record_expr_type(
                callee_span,
                self.resolved_function_type_from_rust_sig_for_owner_path(signature, false, path),
            );
            self.type_info
                .expressions
                .ident_kinds
                .insert((callee_span.start, callee_span.end), IdentKind::RustImport);
        }
        result
    }

    /// Type-check a Rust struct field argument with the metadata-provided target type as context.
    fn check_rust_struct_field_expr(
        &mut self,
        owner_path: &str,
        field: &RustFieldInfo,
        expr: &Spanned<Expr>,
    ) -> ResolvedType {
        let expected = self
            .resolved_rust_boundary_target_from_param_display_for_owner_path(field.type_display.as_str(), owner_path);
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
