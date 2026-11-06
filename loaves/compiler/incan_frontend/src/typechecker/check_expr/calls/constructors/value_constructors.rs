//! Value-enum calls, constructor result types and contexts, constructor checking, and type constructor hooks.

use super::*;

impl TypeChecker {
    /// Return whether a call target names an enum type.
    pub(in crate::typechecker::check_expr::calls) fn is_enum_type_name_expr_for_call(
        &self,
        expr: &Spanned<Expr>,
    ) -> bool {
        let Expr::Ident(name) = &expr.node else {
            return false;
        };
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Enum(_))))
    }

    /// Typecheck `Enum.from_value(...)` against the backing type and retain its generated member identity for Body IR.
    pub(in crate::typechecker::check_expr::calls) fn check_value_enum_from_value_call(
        &mut self,
        enum_name: &str,
        value_enum: &ValueEnumInfo,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }

        let expected = value_enum.value_type.resolved_type();
        let params = vec![CallableParam::named("value", expected.clone(), ParamKind::Normal)];
        let arg_types = self.check_call_arg_types_for_params(args, &params);
        if args.len() != 1 {
            self.errors.push(errors::builtin_arity(
                &format!("{enum_name}.from_value()"),
                1,
                args.len(),
                span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some((arg_ty, arg)) = arg_types.first().zip(args.first()) {
            let expr = Self::call_arg_expr(arg);
            if !self.types_compatible(arg_ty, &expected) {
                self.errors.push(errors::type_mismatch(
                    &expected.to_string(),
                    &arg_ty.to_string(),
                    expr.span,
                ));
            }
        }
        self.record_compiler_generated_member_identity(enum_name, "from_value", None, span);
        option_ty(ResolvedType::Named(enum_name.to_string()))
    }

    /// Typecheck `value()` calls and retain the exact generated member of their checked enum owner for Body IR.
    pub(in crate::typechecker::check_expr::calls) fn check_value_enum_value_call(
        &mut self,
        enum_name: &str,
        value_enum: &ValueEnumInfo,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        self.check_call_args(args);
        if !args.is_empty() {
            self.errors.push(errors::builtin_arity(
                &format!("{enum_name}.value()"),
                0,
                args.len(),
                span,
            ));
            return ResolvedType::Unknown;
        }
        self.record_compiler_generated_member_identity(enum_name, "value", None, span);
        value_enum.value_type.resolved_type()
    }

    /// Return the result type produced by a constructor call.
    pub(in crate::typechecker::check_expr::calls) fn constructor_result_type(&self, name: &str) -> ResolvedType {
        self.constructor_result_type_with_bindings(name, &std::collections::HashMap::new())
    }

    /// Resolve explicit constructor type arguments and build the type-parameter bindings they imply.
    pub(in crate::typechecker::check_expr::calls) fn explicit_constructor_type_context(
        &mut self,
        name: &str,
        type_info: &TypeInfo,
        type_args: &[Spanned<Type>],
        span: Span,
    ) -> Option<(ResolvedType, std::collections::HashMap<String, ResolvedType>)> {
        if type_args.is_empty() {
            return None;
        }
        let type_params = match type_info {
            TypeInfo::Model(info) => &info.type_params,
            TypeInfo::Class(info) => &info.type_params,
            TypeInfo::Newtype(info) => &info.type_params,
            TypeInfo::Enum(info) => &info.type_params,
            _ => return None,
        };
        if type_args.len() != type_params.len() {
            self.errors.push(errors::explicit_type_arg_arity(
                name,
                type_params,
                &Self::written_type_args(type_args),
                span,
            ));
            return Some((ResolvedType::Unknown, std::collections::HashMap::new()));
        }
        let resolved_args: Vec<ResolvedType> = type_args.iter().map(|ty| self.resolve_type_checked(ty)).collect();
        let bindings = type_param_subst_map_call_site(type_params, &resolved_args);
        Some((ResolvedType::Generic(name.to_string(), resolved_args), bindings))
    }

    /// Use an expected generic result to specialize a constructor whose call does not spell type arguments.
    ///
    /// Constructor fields must be checked after this substitution so destination-typed literals inside the call use
    /// the result's concrete or method-generic arguments rather than the enclosing owner's same-named parameters.
    pub(in crate::typechecker::check_expr::calls) fn expected_constructor_type_context(
        &self,
        name: &str,
        type_info: &TypeInfo,
        expected: Option<&ResolvedType>,
    ) -> Option<(ResolvedType, std::collections::HashMap<String, ResolvedType>)> {
        let type_params = Self::constructor_hook_owner_type_params(type_info);
        let Some(ResolvedType::Generic(expected_name, expected_args)) = expected else {
            return None;
        };
        if expected_name != name || expected_args.len() != type_params.len() {
            return None;
        }
        let bindings = type_param_subst_map_call_site(type_params, expected_args);
        Some((expected.cloned()?, bindings))
    }

    /// Compute the constructor result surface type, substituting any generic bindings inferred from constructor fields.
    ///
    /// Unbound type parameters remain `Unknown` so callers can continue typechecking even when inference is partial.
    pub(super) fn constructor_result_type_with_bindings(
        &self,
        name: &str,
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> ResolvedType {
        match self.lookup_type_info(name) {
            Some(TypeInfo::Model(model)) if !model.type_params.is_empty() => ResolvedType::Generic(
                name.to_string(),
                model
                    .type_params
                    .iter()
                    .map(|type_param| bindings.get(type_param).cloned().unwrap_or(ResolvedType::Unknown))
                    .collect(),
            ),
            Some(TypeInfo::Class(class)) if !class.type_params.is_empty() => ResolvedType::Generic(
                name.to_string(),
                class
                    .type_params
                    .iter()
                    .map(|type_param| bindings.get(type_param).cloned().unwrap_or(ResolvedType::Unknown))
                    .collect(),
            ),
            Some(TypeInfo::Newtype(newtype)) if !newtype.type_params.is_empty() => ResolvedType::Generic(
                name.to_string(),
                newtype
                    .type_params
                    .iter()
                    .map(|type_param| bindings.get(type_param).cloned().unwrap_or(ResolvedType::Unknown))
                    .collect(),
            ),
            Some(TypeInfo::Enum(enum_info)) if !enum_info.type_params.is_empty() => ResolvedType::Generic(
                name.to_string(),
                enum_info
                    .type_params
                    .iter()
                    .map(|type_param| bindings.get(type_param).cloned().unwrap_or(ResolvedType::Unknown))
                    .collect(),
            ),
            _ => ResolvedType::Named(name.to_string()),
        }
    }

    /// Typecheck a constructor-style call and return its constructed value type. A type name without declared
    /// construction semantics is refused here; scalar conversions already resolve through builtin call checking.
    pub(in crate::typechecker::check_expr) fn check_constructor(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        if let Some(type_info) = self.lookup_type_info(name).cloned()
            && let Some(ret) = self.check_type_constructor_hook_call(name, &type_info, &[], args, span)
        {
            self.record_direct_callee_identity(name, span);
            return ret;
        }

        self.check_call_args(args);

        if self
            .lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Trait(_)))
        {
            self.errors.push(errors::cannot_instantiate_trait(name, span));
            return ResolvedType::Unknown;
        }

        if self.symbols.lookup(name).is_some()
            && let Some(tid) = self.constructor_surface_type(name)
        {
            if matches!(tid, SurfaceTypeId::ValidationError) {
                self.check_validation_error_fields(args, span);
                self.record_direct_callee_identity(name, span);
                return ResolvedType::Named(surface_types::as_str(tid).to_string());
            }
            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                self.record_direct_callee_identity(name, span);
                return self.check_json_query_constructor_call(tid, args, span);
            }
            if matches!(tid, SurfaceTypeId::Html) {
                self.record_direct_callee_identity(name, span);
                self.record_body_ir_html_constructor(args, span);
                return ResolvedType::Named(surface_types::as_str(tid).to_string());
            }
        }

        match self.lookup_symbol(name).map(|s| &s.kind) {
            Some(SymbolKind::Type(_)) => {
                self.record_direct_callee_identity(name, span);
                if let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(name).cloned() {
                    let [CallArg::Positional(value)] = args else {
                        self.errors.push(errors::newtype_constructor_shape(name, span));
                        return self.constructor_result_type(name);
                    };
                    let errors_before_payload = self.errors.len();
                    let value_ty = self.check_expr_with_expected(value, Some(&newtype.underlying));
                    if !self.types_compatible(&value_ty, &newtype.underlying) {
                        self.errors.push(errors::type_mismatch(
                            &newtype.underlying.to_string(),
                            &value_ty.to_string(),
                            value.span,
                        ));
                    }
                    // The type arguments a generic newtype's value implies must satisfy its declared bounds (#1867).
                    let mut type_bindings = std::collections::HashMap::new();
                    self.infer_type_param_bindings(&newtype.underlying, &value_ty, &mut type_bindings);
                    self.refuse_unsatisfied_nominal_type_arguments(
                        name,
                        &type_bindings,
                        TypeArgumentOrigin::Inferred,
                        span,
                    );
                    if self.errors.len() == errors_before_payload {
                        self.retain_body_ir_newtype_payload(name, &newtype, value, &type_bindings, span);
                    }
                    self.type_info.record_constructor_field_binding(
                        span,
                        ConstructorFieldBinding {
                            argument_slots: vec![0],
                            defaulted_slots: Vec::new(),
                            field_count: 1,
                        },
                    );
                    return self.constructor_result_type(name);
                }
                let ctor_fields: Option<(std::collections::HashMap<String, FieldInfo>, Vec<String>)> =
                    self.lookup_type_info(name).and_then(|info| match info {
                        TypeInfo::Model(m) => Some((m.fields.clone(), m.field_order.clone())),
                        TypeInfo::Class(c) => Some((c.fields.clone(), c.field_order.clone())),
                        _ => None,
                    });
                if let Some((fields, field_order)) = ctor_fields {
                    self.check_model_or_class_constructor_call(name, name, &fields, &field_order, args, span)
                } else {
                    self.errors.push(errors::type_constructor_unavailable(name, span));
                    ResolvedType::Unknown
                }
            }
            Some(SymbolKind::Variant(info)) => {
                let enum_name = info.enum_name.clone();
                self.record_direct_callee_identity(name, span);
                ResolvedType::Named(enum_name)
            }
            Some(_) => ResolvedType::Unknown,
            None => {
                self.errors.push(errors::unknown_symbol(name, span));
                ResolvedType::Unknown
            }
        }
    }

    /// Check ValidationError's single message and optional code without admitting unknown names or unpacking.
    /// Payloads retain their string expectation and source order, including independent checks of invalid fields.
    pub(super) fn check_validation_error_fields(&mut self, args: &[CallArg], span: Span) {
        let mut message_count = 0usize;
        let mut invalid_named = false;
        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    message_count += 1;
                    let actual = self.check_expr_with_expected(expr, Some(&ResolvedType::Str));
                    if !self.types_compatible(&actual, &ResolvedType::Str) {
                        self.errors
                            .push(errors::type_mismatch("str", &actual.to_string(), expr.span));
                    }
                }
                CallArg::Named(field, expr) if field.node == "message" => {
                    message_count += 1;
                    let actual = self.check_expr_with_expected(expr, Some(&ResolvedType::Str));
                    if !self.types_compatible(&actual, &ResolvedType::Str) {
                        self.errors
                            .push(errors::type_mismatch("str", &actual.to_string(), expr.span));
                    }
                }
                CallArg::Named(field, expr) if field.node == "code" => {
                    let actual = self.check_expr_with_expected(expr, Some(&ResolvedType::Str));
                    if !self.types_compatible(&actual, &ResolvedType::Str) {
                        self.errors
                            .push(errors::type_mismatch("str", &actual.to_string(), expr.span));
                    }
                }
                CallArg::Named(_, expr) => {
                    invalid_named = true;
                    self.check_expr(expr);
                }
                CallArg::PositionalUnpack(expr) | CallArg::KeywordUnpack(expr) => {
                    invalid_named = true;
                    self.check_expr(expr);
                }
            }
        }
        if message_count != 1 || invalid_named {
            self.errors.push(errors::validation_error_constructor_shape(span));
        }
    }

    /// Type-check a type constructor call by delegating to its source-defined static `__incan_new` method. Written
    /// brackets specialize the owner, retained by the checked result; the method's own arguments stay separate so
    /// executable calls do not count the owner binders twice.
    pub(in crate::typechecker::check_expr::calls) fn check_type_constructor_hook_call(
        &mut self,
        type_name: &str,
        type_info: &TypeInfo,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if Self::is_named_field_constructor_call(type_info, args) {
            return None;
        }
        let hook = Self::constructor_hook_method(type_info)?;
        if hook.receiver.is_some() {
            return None;
        }
        let type_params = Self::constructor_hook_owner_type_params(type_info);
        if !type_args.is_empty() && type_args.len() != type_params.len() {
            self.errors.push(errors::explicit_type_arg_arity(
                type_name,
                type_params,
                &Self::written_type_args(type_args),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let resolved_type_args = type_args
            .iter()
            .map(|ty| self.resolve_type_checked(ty))
            .collect::<Vec<_>>();
        let receiver_ty = if resolved_type_args.is_empty() {
            ResolvedType::Named(type_name.to_string())
        } else {
            ResolvedType::Generic(type_name.to_string(), resolved_type_args)
        };
        let errors_before_call = self.errors.len();
        let result = self.check_resolved_generic_method_call(
            TYPE_CONSTRUCTOR_HOOK,
            hook,
            &[],
            args,
            &[],
            span,
            &receiver_ty,
            None,
        );
        let key = span;
        if self.errors.len() == errors_before_call
            && let Some(arguments) = self.type_info.calls.call_site_monomorph_type_args.get(&key).cloned()
        {
            self.type_info.calls.body_ir_call_type_arguments.insert(key, arguments);
        }
        Some(result)
    }

    /// Return whether a call's named arguments exactly describe normal model/class field construction.
    pub(in crate::typechecker::check_expr::calls) fn is_named_field_constructor_call(
        type_info: &TypeInfo,
        args: &[CallArg],
    ) -> bool {
        let fields = match type_info {
            TypeInfo::Model(info) => &info.fields,
            TypeInfo::Class(info) => &info.fields,
            _ => return false,
        };
        !args.is_empty()
            && args.iter().all(|arg| match arg {
                CallArg::Named(field, _) => fields.contains_key(&field.node),
                _ => false,
            })
    }

    /// Resolve the static constructor hook method for a type that supports direct checked construction.
    pub(super) fn constructor_hook_method(type_info: &TypeInfo) -> Option<crate::symbols::MethodInfo> {
        match type_info {
            TypeInfo::Model(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Class(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Enum(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Newtype(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            _ => None,
        }
    }

    /// Return owner type parameters whose explicit constructor arguments specialize the `Self` receiver.
    pub(super) fn constructor_hook_owner_type_params(type_info: &TypeInfo) -> &[String] {
        match type_info {
            TypeInfo::Model(info) => &info.type_params,
            TypeInfo::Class(info) => &info.type_params,
            TypeInfo::Enum(info) => &info.type_params,
            TypeInfo::Newtype(info) => &info.type_params,
            _ => &[],
        }
    }
}
