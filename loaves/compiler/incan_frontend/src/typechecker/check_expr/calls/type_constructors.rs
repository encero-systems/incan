//! Public module type constructors, Rust struct constructors, Rust import calls and graph constructors.

use super::*;

impl TypeChecker {
    /// Validate a constructor reached through a checked public module namespace while retaining its provider-owned
    /// nominal identity.
    pub(in crate::typechecker::check_expr) fn check_public_module_type_constructor_call(
        &mut self,
        context: PublicModuleConstructorContext<'_>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Span,
    ) -> ResolvedType {
        self.type_info.calls.body_ir_module_constructor_bindings.remove(&span);
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
                .insert(callee_span, IdentKind::TypeName);
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

        let errors_before_constructor = self.errors.len();
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
                self.check_model_or_class_constructor_call(
                    type_name,
                    display_name,
                    &fields,
                    &model.field_order,
                    args,
                    span,
                )
            }
            TypeInfo::Class(class) => {
                let mut fields = class.fields.clone();
                if let Some((_, type_bindings)) = &explicit_context {
                    for field in fields.values_mut() {
                        field.ty = substitute_resolved_type(&field.ty, type_bindings);
                    }
                }
                self.check_model_or_class_constructor_call(
                    type_name,
                    display_name,
                    &fields,
                    &class.field_order,
                    args,
                    span,
                )
            }
            _ => ResolvedType::Unknown,
        };
        if self.errors.len() == errors_before_constructor && args.is_empty() {
            self.retain_body_ir_defaulted_module_class(type_info, span);
        }
        let result = explicit_ty.unwrap_or(constructor_ty);
        self.retain_module_constructor_owner(span, &result);
        self.record_expr_type(callee_span, result.clone());
        self.type_info
            .expressions
            .ident_kinds
            .insert(callee_span, IdentKind::TypeName);
        result
    }

    /// Preserve the selected module constructor's exact nominal owner for Body IR type projection.
    ///
    /// Two source modules may publish the same type name. The ordinary checked result keeps that source-facing name,
    /// while the private constructor carrier keys the executable type by the canonical identity already selected at
    /// the call. This prevents a qualified constructor from borrowing another same-named declaration's layout.
    fn retain_module_constructor_owner(&mut self, span: Span, result: &ResolvedType) {
        let Some(identity) = self.type_info.resolved_identity(span).cloned() else {
            return;
        };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(&identity) else {
            return;
        };
        let retained = match result {
            ResolvedType::Named(_) => ResolvedType::Named(carrier.clone()),
            ResolvedType::Generic(_, arguments) => ResolvedType::Generic(carrier.clone(), arguments.clone()),
            _ => return,
        };
        self.type_info
            .declarations
            .named_type_identities
            .insert(carrier, identity);
        self.type_info.calls.body_ir_constructor_types.insert(span, retained);
    }

    /// Return whether inspected type metadata proves omitted named fields can be filled through `Default`.
    pub(super) fn rust_type_supports_default_field_fill(type_info: &RustTypeInfo) -> bool {
        let is_default = |path: &str| {
            traits::rust_paths(TraitId::Default).contains(&path) || path == traits::as_str(TraitId::Default)
        };
        type_info.implemented_traits.iter().any(|implemented| {
            implemented.is_unconditional() && !implemented.mutable_reference && is_default(implemented.path.as_str())
        }) || type_info
            .expanded_derive_traits
            .iter()
            .any(|implemented| is_default(implemented.path.as_str()))
    }

    /// Type-check a call to an imported Rust named-field struct using checked field metadata.
    pub(super) fn check_rust_named_field_constructor_call(
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
                    let Some(field) = Self::rust_field_for_source_name(type_info, field_name.node.as_str()) else {
                        self.check_expr(expr);
                        self.errors
                            .push(errors::missing_field(path, &field_name.node, field_name.span));
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

        if !has_shape_error
            && !omitted_fields
            && let Some(result) = self.retain_rust_construction(path, type_info, args, &selected_fields, span)
        {
            return result;
        }
        ResolvedType::RustPath(path.to_string())
    }

    /// Type-check a positional imported Rust tuple-struct constructor using checked field metadata.
    ///
    /// Empty field labels are the shared metadata representation for tuple positions. Keeping that distinction in
    /// metadata lets lowering emit `Type(value)` instead of incorrectly treating the type as an unconstructible
    /// named-field record.
    pub(super) fn check_rust_tuple_struct_constructor_call(
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

        if !has_shape_error
            && type_info.fields.is_empty()
            && let Some(result) = self.retain_rust_construction(path, type_info, args, &[], span)
        {
            return result;
        }
        ResolvedType::RustPath(path.to_string())
    }

    /// Type-check a Rust type-shaped constructor when metadata is unavailable but source names every field.
    ///
    /// Metadata is still required for positional construction because field order must come from Rust. Named source
    /// arguments already carry the field names, so this path can preserve IncQL/Substrait-style protobuf
    /// constructors without emitting invalid tuple calls.
    pub(super) fn check_metadata_free_rust_named_field_constructor_call(
        &mut self,
        path: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        let mut provided = HashSet::new();
        let mut has_shape_error = false;
        let mut emitted_metadata_error = false;

        for arg in args {
            match arg {
                CallArg::Named(field_name, expr) => {
                    self.check_expr(expr);
                    if !provided.insert(field_name.node.clone()) {
                        self.errors.push(errors::duplicate_constructor_field(
                            path,
                            field_name.node.as_str(),
                            field_name.span,
                        ));
                        has_shape_error = true;
                    }
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
            return ResolvedType::RustPath(path.to_string());
        }
        ResolvedType::Unknown
    }

    /// Validate one Rust import call and record its callable semantics when validation succeeds. Synchronous safe
    /// diverging calls additionally retain their selected path for Body IR; legacy reference identities stay intact.
    pub(super) fn validate_and_record_rust_import_function_call(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let error_count_before = self.errors.len();
        let result = self.validate_rust_function_call_with_type_arguments(
            path,
            signature,
            explicit_type_args,
            args,
            span,
            expected_return_ty,
        );
        if self.errors.len() == error_count_before {
            if result == ResolvedType::Never && !signature.is_async && !signature.is_unsafe {
                self.type_info
                    .rust
                    .body_ir_diverging_import_calls
                    .insert(span, path.split("::").map(str::to_owned).collect());
            }
            self.record_expr_type(
                callee_span,
                self.resolved_function_type_from_rust_sig_for_owner_path(signature, false, path),
            );
            self.type_info
                .expressions
                .ident_kinds
                .insert(callee_span, IdentKind::RustImport);
        }
        result
    }

    /// Type-check a Rust struct field argument with the metadata-provided target type as context.
    pub(super) fn check_rust_struct_field_expr(
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
    pub(super) fn check_graph_constructor_call(
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
                type_params,
                &Self::written_type_args(type_args),
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
