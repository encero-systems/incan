//! Direct calls to declared types, functions, Rust items, legacy constructors, decorated generics and values.

use super::*;

impl TypeChecker {
    /// Check construction hooks and surface constructors before contextual nominal construction.
    pub(super) fn check_declared_type_direct_call(
        &mut self,
        name: &str,
        type_info: &TypeInfo,
        call: &DirectCallSite<'_>,
    ) -> ResolvedType {
        let (callee, type_args, args, span) = (call.callee, call.type_args, call.args, call.span);
        if self.refuse_explicit_ownership_constructor(name, span) {
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }
        if let Some(ret) = self.check_type_constructor_hook_call(name, type_info, type_args, args, span) {
            self.record_direct_callee_identity(name, callee.span);
            self.record_expr_type(callee.span, ResolvedType::Named(name.to_owned()));
            self.type_info
                .expressions
                .ident_kinds
                .insert(callee.span, IdentKind::TypeName);
            return ret;
        }
        if stdlib::is_graph_constructor_type(name) && args.is_empty() {
            self.record_direct_callee_identity(name, callee.span);
            return self.check_graph_constructor_call(name, type_info, type_args, args, span);
        }
        if let Some(tid) = self.constructor_surface_type(name) {
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                self.check_call_args(args);
                return ResolvedType::Unknown;
            }
            self.record_direct_callee_identity(name, callee.span);
            self.record_expr_type(callee.span, ResolvedType::Named(name.to_owned()));
            self.type_info
                .expressions
                .ident_kinds
                .insert(callee.span, IdentKind::TypeName);
            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                return self.check_json_query_constructor_call(tid, args, span);
            }
            if matches!(tid, SurfaceTypeId::Html) {
                self.record_body_ir_html_constructor(args, span);
                // The value is typed by the binding the call spells, so `Page(...)` under
                // `from std.web import Html as Page` is a `Page`, as its annotations are.
                return ResolvedType::Named(name.to_owned());
            }
            if matches!(tid, SurfaceTypeId::ValidationError) {
                return self.check_constructor(name, args, span);
            }
        }
        self.check_contextual_nominal_direct_call(name, type_info, call)
    }

    /// Bind explicit or destination-provided nominal arguments before validating fields and retaining Body IR facts.
    pub(super) fn check_contextual_nominal_direct_call(
        &mut self,
        name: &str,
        type_info: &TypeInfo,
        call: &DirectCallSite<'_>,
    ) -> ResolvedType {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        let explicit_constructor_context = self.explicit_constructor_type_context(name, type_info, type_args, span);
        if let Some((_, type_bindings)) = &explicit_constructor_context {
            self.refuse_unsatisfied_nominal_type_arguments(name, type_bindings, TypeArgumentOrigin::Explicit, span);
        }
        let constructor_context = explicit_constructor_context
            .or_else(|| self.expected_constructor_type_context(name, type_info, expected_return_ty));
        let contextual_constructor_ty = constructor_context.as_ref().map(|(ty, _)| ty.clone());
        if let TypeInfo::Model(model) = type_info
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
            self.record_direct_callee_identity(name, callee.span);
            self.record_expr_type(callee.span, ResolvedType::Named(name.to_owned()));
            self.type_info
                .expressions
                .ident_kinds
                .insert(callee.span, IdentKind::TypeName);
            let constructor_ty = self.check_constructor(name, args, span);
            return contextual_constructor_ty.unwrap_or(constructor_ty);
        }
        let ctor_fields = match type_info {
            TypeInfo::Model(info) => Some((info.fields.clone(), info.field_order.clone())),
            TypeInfo::Class(info) => Some((info.fields.clone(), info.field_order.clone())),
            _ => None,
        };
        let Some((mut fields, field_order)) = ctor_fields else {
            return self.check_constructor(name, args, span);
        };
        self.record_direct_callee_identity(name, callee.span);
        if let Some((_, type_bindings)) = &constructor_context {
            for field in fields.values_mut() {
                field.ty = substitute_resolved_type(&field.ty, type_bindings);
            }
        }
        let errors_before_constructor = self.errors.len();
        let constructor_ty = self.check_model_or_class_constructor_call(name, name, &fields, &field_order, args, span);
        if self.errors.len() == errors_before_constructor {
            self.retain_body_ir_constructor_type(name, type_info, constructor_context.as_ref(), args, span);
        }
        self.record_expr_type(callee.span, ResolvedType::Named(name.to_owned()));
        self.type_info
            .expressions
            .ident_kinds
            .insert(callee.span, IdentKind::TypeName);
        contextual_constructor_ty.unwrap_or(constructor_ty)
    }

    /// Validate a direct function call and attach capturing, async and related-declaration facts in diagnostic order.
    pub(super) fn check_declared_function_direct_call(
        &mut self,
        name: &str,
        func_info: &FunctionInfo,
        call: &DirectCallSite<'_>,
    ) -> ResolvedType {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        self.record_direct_callee_identity(name, callee.span);
        let declaration = self.type_info.resolved_identity(callee.span).cloned();
        let first_error = self.errors.len();
        let result = self.validate_function_call_with_identity(
            name,
            func_info,
            type_args,
            args,
            span,
            expected_return_ty,
            declaration.clone(),
        );
        self.check_capturing_call_arguments(declaration.as_ref(), name, &func_info.params, args);
        self.retain_body_ir_async_call_target(func_info.is_async, name, span, callee.span, first_error);
        if let Some(declaration) = declaration {
            self.attach_related_declaration_to_new_errors(first_error, &declaration);
        }
        result
    }

    /// Use authoritative Rust signatures or constructible layout metadata before metadata-free fallback.
    pub(super) fn check_declared_rust_direct_call(
        &mut self,
        info: &RustItemInfo,
        call: &DirectCallSite<'_>,
    ) -> Option<ResolvedType> {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if let Some(sig) = compiler_owned_function_signature(info.path.as_str()) {
            return Some(self.validate_and_record_rust_import_function_call(
                info.path.as_str(),
                &sig,
                args,
                span,
                callee.span,
                expected_return_ty,
            ));
        }
        let metadata = info
            .metadata
            .clone()
            .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
        if let Some(meta) = metadata.as_ref() {
            return Some(self.check_rust_metadata_direct_call(&info.path, &meta.kind, call));
        }
        if let Some(sig) = metadata_free_function_signature(info.path.as_str()) {
            return Some(self.validate_and_record_rust_import_function_call(
                info.path.as_str(),
                &sig,
                args,
                span,
                callee.span,
                expected_return_ty,
            ));
        }
        if rust_path_last_segment_looks_like_type(info.path.as_str()) {
            return Some(self.check_metadata_free_rust_named_field_constructor_call(info.path.as_str(), args, span));
        }
        None
    }

    /// Check a cached Rust item without weakening metadata completeness or emitting constructor facts on error.
    pub(super) fn check_rust_metadata_direct_call(
        &mut self,
        rust_path: &str,
        kind: &RustItemKind,
        call: &DirectCallSite<'_>,
    ) -> ResolvedType {
        let (callee, args, span, expected_return_ty) = (call.callee, call.args, call.span, call.expected_return_ty);
        match kind {
            RustItemKind::Function(sig) => {
                return self.validate_and_record_rust_import_function_call(
                    rust_path,
                    sig,
                    args,
                    span,
                    callee.span,
                    expected_return_ty,
                );
            }
            RustItemKind::Type(type_info)
                if type_info.metadata_completeness.has_constructible_layout()
                    && (!type_info.fields.is_empty() || (type_info.variants.is_empty() && args.is_empty())) =>
            {
                let error_count_before = self.errors.len();
                let result = if type_info.fields.iter().all(|field| field.name.is_empty()) {
                    self.check_rust_tuple_struct_constructor_call(rust_path, type_info, args, span)
                } else {
                    self.check_rust_named_field_constructor_call(rust_path, type_info, args, span)
                };
                if self.errors.len() == error_count_before {
                    self.record_expr_type(callee.span, ResolvedType::RustPath(rust_path.to_owned()));
                    self.type_info
                        .expressions
                        .ident_kinds
                        .insert(callee.span, IdentKind::RustImport);
                }
                return result;
            }
            RustItemKind::Type(_) => {
                self.check_call_args(args);
                self.errors
                    .push(errors::rust_constructor_metadata_unavailable(rust_path, span));
                return ResolvedType::Unknown;
            }
            RustItemKind::Unsupported { description } => {
                self.check_call_args(args);
                self.errors.push(errors::rust_item_shape_not_supported(
                    rust_path,
                    description.as_str(),
                    span,
                ));
                return ResolvedType::Unknown;
            }
            _ => {
                self.check_call_args(args);
                self.errors
                    .push(errors::rust_constructor_metadata_unavailable(rust_path, span));
                return ResolvedType::Unknown;
            }
        }
    }

    /// Preserve legacy in-scope surface aliases and constructor fallback after resolved declarations decline.
    pub(super) fn check_legacy_named_constructor_call(
        &mut self,
        name: &str,
        call: &DirectCallSite<'_>,
    ) -> Option<ResolvedType> {
        let (callee, args, span) = (call.callee, call.args, call.span);
        let in_scope = self.symbols.lookup(name).is_some();
        if in_scope && let Some(tid) = self.constructor_surface_type(name) {
            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                return Some(self.check_json_query_constructor_call(tid, args, span));
            }
            if matches!(tid, SurfaceTypeId::Html) {
                self.record_body_ir_html_constructor(args, span);
                return Some(ResolvedType::Named(name.to_owned()));
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
            return Some(ResolvedType::Unknown);
        }

        // Model/class constructor calls: validate field arguments at the Incan level.
        // NOTE: `lookup_type_info` returns a reference into `self`, so we clone the needed field map to avoid
        // borrow conflicts (we need `&mut self` for validation).
        let ctor_fields: Option<(std::collections::HashMap<String, FieldInfo>, Vec<String>)> =
            self.lookup_type_info(name).and_then(|info| match info {
                TypeInfo::Model(m) => Some((m.fields.clone(), m.field_order.clone())),
                TypeInfo::Class(c) => Some((c.fields.clone(), c.field_order.clone())),
                _ => None,
            });
        if let Some((fields, field_order)) = ctor_fields {
            let constructor_ty =
                self.check_model_or_class_constructor_call(name, name, &fields, &field_order, args, span);
            self.record_expr_type(callee.span, ResolvedType::Named(name.to_owned()));
            self.type_info
                .expressions
                .ident_kinds
                .insert(callee.span, IdentKind::TypeName);
            if in_scope && let Some(tid) = self.constructor_surface_type(name) {
                if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                    return Some(self.check_json_query_constructor_call(tid, args, span));
                }
                if matches!(tid, SurfaceTypeId::Html) {
                    self.record_body_ir_html_constructor(args, span);
                    return Some(ResolvedType::Named(name.to_owned()));
                }
            }
            return Some(constructor_ty);
        }
        None
    }

    /// Check decorated generic calls through the declaration's binders for explicit and inferred type arguments.
    /// Ordinary callable-value inference cannot retain the closed declaration instance required by Body IR. The
    /// resolved symbol's declaration span excludes local values shadowing a decorated declaration's name.
    pub(super) fn check_decorated_generic_direct_call(&mut self, call: &DirectCallSite<'_>) -> Option<ResolvedType> {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        if let Expr::Ident(name) = &callee.node
            && let Some(symbol) = self.lookup_symbol(name)
            && let Some(binding) = self
                .type_info
                .declarations
                .decorated_function_bindings_by_span
                .get(&symbol.span)
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
            // This branch returns before `check_expr(callee)`, so retain the declaration identity here as well.
            self.record_direct_callee_identity(name, callee.span);
            return Some(self.validate_function_call(name, &info, type_args, args, span, expected_return_ty));
        }
        None
    }

    /// Infer callable values and operator receivers only after declaration-specific call policies decline.
    pub(super) fn check_direct_value_call(&mut self, call: &DirectCallSite<'_>) -> ResolvedType {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
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
                self.check_capturing_value_call_arguments(callee, &params, args);
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
}
