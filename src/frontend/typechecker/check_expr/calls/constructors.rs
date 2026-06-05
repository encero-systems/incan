//! Constructor-style call validation for models, classes, enums, and surface constructors.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Expr, ParamKind, Span, Spanned, Type};
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::type_param_subst_map_call_site;
use crate::frontend::symbols::{CallableParam, FieldInfo, ResolvedType, SymbolKind, TypeInfo, ValueEnumInfo};
use crate::frontend::typechecker::helpers::option_ty;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};

const TYPE_CONSTRUCTOR_HOOK: &str = "__incan_new";

impl TypeChecker {
    /// Validate model/class constructor arguments, including RFC 017 coercions for typed field initializers.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_model_or_class_constructor_call(
        &mut self,
        type_name: &str,
        fields: &std::collections::HashMap<String, FieldInfo>,
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        // v0.1: only named args for model/class constructors (stable field ordering not guaranteed).
        if args.iter().any(|a| matches!(a, CallArg::Positional(_))) {
            // Typecheck argument expressions regardless, so type errors in expressions still show up.
            self.check_call_args(args);
            self.errors
                .push(errors::positional_constructor_args_not_supported(type_name, call_span));
            return self.constructor_result_type(type_name);
        }

        // Track provided fields and validate existence/duplicates/type compatibility.
        let mut provided: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
        let mut type_bindings: std::collections::HashMap<String, ResolvedType> = std::collections::HashMap::new();
        for arg in args {
            let CallArg::Named(field_name, expr) = arg else {
                continue;
            };

            let Some((canonical_name, field_info)) = self.resolve_field_info(fields, field_name, true, true) else {
                // Still typecheck the expression exactly once so nested diagnostics are preserved.
                self.check_expr(expr);
                self.errors
                    .push(errors::missing_field(type_name, field_name, expr.span));
                continue;
            };

            let value_ty = self.check_expr_with_expected(expr, Some(&field_info.ty));

            if provided.contains_key(&canonical_name) {
                self.errors.push(errors::duplicate_field_in_call(
                    type_name,
                    canonical_name.as_str(),
                    expr.span,
                ));
                continue;
            }
            provided.insert(canonical_name.clone(), expr.span);
            self.infer_type_param_bindings(&field_info.ty, &value_ty, &mut type_bindings);

            if !self.types_compatible(&value_ty, &field_info.ty)
                && !self.record_validated_newtype_field_coercion_if_possible(
                    &value_ty,
                    &field_info.ty,
                    &canonical_name,
                    expr.span,
                )
            {
                self.errors.push(errors::field_type_mismatch(
                    field_name,
                    &field_info.ty.to_string(),
                    &value_ty.to_string(),
                    expr.span,
                ));
            }
        }

        // Enforce required fields (those without defaults) are present.
        for (field_name, info) in fields {
            if !info.has_default && !provided.contains_key(field_name) {
                self.errors.push(errors::missing_required_constructor_field(
                    type_name, field_name, call_span,
                ));
            }
        }

        self.constructor_result_type_with_bindings(type_name, &type_bindings)
    }
    /// Type-check a JSON/Query constructor call (`Json(...)` / `Query(...)`).
    ///
    /// NOTE: This method is called from multiple dispatch points in the typechecker because calls can be classified
    /// differently by the parser (bare identifier call, constructor call, builtin call, or model/class constructor).
    /// Each dispatch point returns early after handling, preventing double-checking.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_json_query_constructor_call(
        &mut self,
        tid: SurfaceTypeId,
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        let mut inner = ResolvedType::Unknown;
        let mut has_inner = false;
        let mut positional_count = 0;
        let mut named_value_count = 0;
        let mut has_invalid_named = false;

        for arg in args {
            match arg {
                CallArg::Positional(e) => {
                    positional_count += 1;
                    if !has_inner {
                        inner = self.check_expr(e);
                        has_inner = true;
                    } else {
                        self.check_expr(e);
                    }
                }
                CallArg::Named(name, e) if name == "value" => {
                    named_value_count += 1;
                    if !has_inner {
                        inner = self.check_expr(e);
                        has_inner = true;
                    } else {
                        self.check_expr(e);
                    }
                }
                CallArg::Named(_, e) => {
                    has_invalid_named = true;
                    self.check_expr(e);
                }
                CallArg::PositionalUnpack(e) | CallArg::KeywordUnpack(e) => {
                    has_invalid_named = true;
                    self.check_expr(e);
                }
            }
        }

        let total_allowed = positional_count + named_value_count;
        if has_invalid_named || total_allowed != 1 || (positional_count > 0 && named_value_count > 0) {
            let name = surface_types::as_str(tid);
            self.errors
                .push(errors::constructor_single_arg_required(name, args.len(), call_span));
        }

        ResolvedType::Generic(surface_types::as_str(tid).to_string(), vec![inner])
    }
    /// Return whether a call target names an enum type.
    pub(in crate::frontend::typechecker::check_expr::calls) fn is_enum_type_name_expr_for_call(
        &self,
        expr: &Spanned<Expr>,
    ) -> bool {
        let Expr::Ident(name) = &expr.node else {
            return false;
        };
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Enum(_))))
    }

    /// Typecheck `Enum.from_value(...)` against the value enum backing type.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_value_enum_from_value_call(
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
        option_ty(ResolvedType::Named(enum_name.to_string()))
    }

    /// Typecheck `value()` calls on value enum instances.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_value_enum_value_call(
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
        value_enum.value_type.resolved_type()
    }
    /// Return the result type produced by a constructor call.
    pub(in crate::frontend::typechecker::check_expr::calls) fn constructor_result_type(
        &self,
        name: &str,
    ) -> ResolvedType {
        self.constructor_result_type_with_bindings(name, &std::collections::HashMap::new())
    }

    /// Resolve explicit constructor type arguments and build the type-parameter bindings they imply.
    pub(in crate::frontend::typechecker::check_expr::calls) fn explicit_constructor_type_context(
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
                type_params.len(),
                type_args.len(),
                span,
            ));
            return Some((ResolvedType::Unknown, std::collections::HashMap::new()));
        }
        let resolved_args: Vec<ResolvedType> = type_args.iter().map(|ty| self.resolve_type_checked(ty)).collect();
        let bindings = type_param_subst_map_call_site(type_params, &resolved_args);
        Some((ResolvedType::Generic(name.to_string(), resolved_args), bindings))
    }

    /// Compute the constructor result surface type, substituting any generic bindings inferred from constructor fields.
    ///
    /// Unbound type parameters remain `Unknown` so callers can continue typechecking even when inference is partial.
    fn constructor_result_type_with_bindings(
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
    /// Typecheck a constructor-style call and return its constructed value type.
    pub(in crate::frontend::typechecker::check_expr) fn check_constructor(
        &mut self,
        name: &str,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        if let Some(type_info) = self.lookup_type_info(name).cloned()
            && let Some(ret) = self.check_type_constructor_hook_call(name, &type_info, &[], args, span)
        {
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
            && let Some(tid) = surface_types::from_str(name)
        {
            if matches!(tid, SurfaceTypeId::ValidationError) {
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
                        CallArg::Named(field, expr) if field == "message" => {
                            message_count += 1;
                            let actual = self.check_expr_with_expected(expr, Some(&ResolvedType::Str));
                            if !self.types_compatible(&actual, &ResolvedType::Str) {
                                self.errors
                                    .push(errors::type_mismatch("str", &actual.to_string(), expr.span));
                            }
                        }
                        CallArg::Named(field, expr) if field == "code" => {
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
                return ResolvedType::Named(surface_types::as_str(tid).to_string());
            }
            if matches!(tid, SurfaceTypeId::Json | SurfaceTypeId::Query) {
                return self.check_json_query_constructor_call(tid, args, span);
            }
            if matches!(tid, SurfaceTypeId::Html) {
                return ResolvedType::Named(surface_types::as_str(tid).to_string());
            }
        }

        match self.lookup_symbol(name).map(|s| &s.kind) {
            Some(SymbolKind::Type(_)) => {
                if let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(name).cloned() {
                    let [CallArg::Positional(value)] = args else {
                        self.errors.push(errors::newtype_constructor_shape(name, span));
                        return self.constructor_result_type(name);
                    };
                    let value_ty = self.check_expr_with_expected(value, Some(&newtype.underlying));
                    if !self.types_compatible(&value_ty, &newtype.underlying) {
                        self.errors.push(errors::type_mismatch(
                            &newtype.underlying.to_string(),
                            &value_ty.to_string(),
                            value.span,
                        ));
                    }
                    return self.constructor_result_type(name);
                }
                let ctor_fields: Option<std::collections::HashMap<String, FieldInfo>> =
                    self.lookup_type_info(name).and_then(|info| match info {
                        TypeInfo::Model(m) => Some(m.fields.clone()),
                        TypeInfo::Class(c) => Some(c.fields.clone()),
                        _ => None,
                    });
                if let Some(fields) = ctor_fields {
                    self.check_model_or_class_constructor_call(name, &fields, args, span)
                } else {
                    self.constructor_result_type(name)
                }
            }
            Some(SymbolKind::Variant(info)) => ResolvedType::Named(info.enum_name.clone()),
            Some(_) => ResolvedType::Unknown,
            None => {
                self.errors.push(errors::unknown_symbol(name, span));
                ResolvedType::Unknown
            }
        }
    }

    /// Type-check a type constructor call by delegating to its source-defined static `__incan_new` method.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_type_constructor_hook_call(
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
                type_params.len(),
                type_args.len(),
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
        Some(self.check_generic_method_call(TYPE_CONSTRUCTOR_HOOK, hook, &[], args, &[], span, &receiver_ty, None))
    }

    /// Return whether a call's named arguments exactly describe normal model/class field construction.
    pub(in crate::frontend::typechecker::check_expr::calls) fn is_named_field_constructor_call(
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
                CallArg::Named(field, _) => fields.contains_key(field),
                _ => false,
            })
    }

    /// Resolve the static constructor hook method for a type that supports direct checked construction.
    fn constructor_hook_method(type_info: &TypeInfo) -> Option<crate::frontend::symbols::MethodInfo> {
        match type_info {
            TypeInfo::Model(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Class(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Enum(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            TypeInfo::Newtype(info) => info.methods.get(TYPE_CONSTRUCTOR_HOOK).cloned(),
            _ => None,
        }
    }

    /// Return owner type parameters whose explicit constructor arguments specialize the `Self` receiver.
    fn constructor_hook_owner_type_params(type_info: &TypeInfo) -> &[String] {
        match type_info {
            TypeInfo::Model(info) => &info.type_params,
            TypeInfo::Class(info) => &info.type_params,
            TypeInfo::Enum(info) => &info.type_params,
            TypeInfo::Newtype(info) => &info.type_params,
            _ => &[],
        }
    }
}
