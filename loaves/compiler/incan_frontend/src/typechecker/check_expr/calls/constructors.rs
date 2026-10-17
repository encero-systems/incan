//! Constructor-style call validation for models, classes, enums, and surface constructors.

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Literal, ParamKind, Span, Spanned, Type};
use crate::diagnostics::errors::{self, TypeArgumentOrigin};
use crate::resolved_type_subst::type_param_subst_map_call_site;
use crate::symbols::{CallableParam, FieldInfo, ResolvedType, SymbolKind, TypeInfo, ValueEnumInfo};
use crate::typechecker::helpers::option_ty;
use crate::typechecker::type_info::ConstructorFieldBinding;
use incan_lang::lang::conventions::TYPE_CONSTRUCTOR_HOOK;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};

impl TypeChecker {
    /// Retain a checked newtype payload's nominal arguments, permitting only parameters owned by the active frame.
    /// Concrete calls become closed carriers; a declaration's own parameter remains explicit for later specialization.
    fn retain_body_ir_newtype_payload(
        &mut self,
        name: &str,
        parameters: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        span: Span,
    ) {
        if parameters.is_empty()
            || parameters.iter().any(|parameter| {
                bindings
                    .get(parameter)
                    .is_none_or(|ty| !Self::is_settled_type_argument(ty) && self.active_type_param_name(ty).is_none())
            })
        {
            return;
        }
        let carrier = self.constructor_result_type_with_bindings(name, bindings);
        self.type_info
            .calls
            .body_ir_constructor_types
            .insert((span.start, span.end), carrier);
    }

    /// Retain an executable constructor binding without changing contextual types or legacy call facts.
    ///
    /// The constructor's original fields identify its own binders, whereas contextual checking can replace them
    /// with a generic callee's still-open binders. Checked value types close the original field contract here; a
    /// settled destination takes precedence, and every declared nominal bound must hold before Body IR uses it.
    pub(in crate::typechecker::check_expr::calls) fn retain_body_ir_constructor_type(
        &mut self,
        name: &str,
        declaration: &TypeInfo,
        context: Option<&(ResolvedType, std::collections::HashMap<String, ResolvedType>)>,
        args: &[CallArg],
        span: Span,
    ) {
        let (parameters, fields) = match declaration {
            TypeInfo::Model(model) => (&model.type_params, &model.fields),
            TypeInfo::Class(class) => (&class.type_params, &class.fields),
            _ => return,
        };
        if parameters.is_empty() {
            return;
        }

        // ---- Checked destination and original field binders ----
        let mut bindings = context
            .map(|(_, bindings)| bindings)
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .filter(|(_, ty)| Self::is_settled_type_argument(ty))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        for argument in args {
            let CallArg::Named(field, value) = argument else {
                return;
            };
            let Some((_, original)) = self.resolve_field_info(fields, &field.node, true, true) else {
                return;
            };
            let Some(actual) = self.type_info.expr_type(value.span) else {
                return;
            };
            self.infer_type_param_bindings(&original.ty, actual, &mut bindings);
        }
        if parameters.iter().any(|parameter| {
            bindings
                .get(parameter)
                .is_none_or(|ty| !Self::is_settled_type_argument(ty))
        }) {
            return;
        }

        // ---- Bound proof and Body IR-only publication ----
        if let Some(bounds) = self.declared_nominal_bounds(name) {
            for (parameter, requirements) in bounds {
                let Some(actual) = bindings.get(parameter) else {
                    return;
                };
                if requirements
                    .iter()
                    .any(|bound| !self.type_satisfies_explicit_bound_info(actual, bound, &bindings))
                {
                    return;
                }
            }
        }
        let carrier = self.constructor_result_type_with_bindings(name, &bindings);
        self.type_info
            .calls
            .body_ir_constructor_types
            .insert((span.start, span.end), carrier);
    }

    /// Validate model/class constructor arguments, including RFC 017 coercions for typed field initializers.
    pub(in crate::typechecker::check_expr::calls) fn check_model_or_class_constructor_call(
        &mut self,
        type_name: &str,
        display_name: &str,
        fields: &std::collections::HashMap<String, FieldInfo>,
        args: &[CallArg],
        call_span: Span,
    ) -> ResolvedType {
        // v0.1: only named args for model/class constructors (stable field ordering not guaranteed).
        if args.iter().any(|a| matches!(a, CallArg::Positional(_))) {
            // Typecheck argument expressions regardless, so type errors in expressions still show up.
            self.check_call_args(args);
            self.errors.push(errors::positional_constructor_args_not_supported(
                display_name,
                call_span,
            ));
            return self.constructor_result_type(type_name);
        }

        // Track provided fields and validate existence/duplicates/type compatibility.
        let errors_before_fields = self.errors.len();
        let mut field_arguments: Vec<(&Spanned<Expr>, ResolvedType)> = Vec::new();
        let mut provided: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
        let mut type_bindings: std::collections::HashMap<String, ResolvedType> = std::collections::HashMap::new();
        // Canonical field bound by each written argument, in written source order, for #1158's Body IR binding fact.
        // `None` marks an argument this check already rejected, which keeps the recorded binding fail-closed.
        let mut bound_fields: Vec<Option<String>> = Vec::with_capacity(args.len());
        for arg in args {
            let CallArg::Named(field_name, expr) = arg else {
                // Positional arguments already returned above, so this is a `*`/`**` spread: a written argument that
                // resolves to no single field. Recording `None` keeps `bound_fields` aligned with `args` and makes
                // the fail-closed guard below actually fire, rather than silently recording a shorter binding.
                bound_fields.push(None);
                continue;
            };

            let Some((canonical_name, field_info)) = self.resolve_field_info(fields, &field_name.node, true, true)
            else {
                bound_fields.push(None);
                // Still typecheck the expression exactly once so nested diagnostics are preserved.
                self.check_expr(expr);
                self.errors
                    .push(errors::missing_field(display_name, &field_name.node, field_name.span));
                continue;
            };

            if let Some(identity) = field_info.identity.clone() {
                self.type_info.record_resolved_identity(field_name.span, identity);
            }
            self.record_body_ir_constructor_field_requirement(type_name, field_info, field_name.span);

            let value_ty = self.check_expr_with_expected(expr, Some(&field_info.ty));
            self.record_value_destination_if_compatible(expr.span, &value_ty, &field_info.ty);

            if provided.contains_key(&canonical_name) {
                self.errors.push(errors::duplicate_field_in_call(
                    type_name,
                    canonical_name.as_str(),
                    expr.span,
                ));
                bound_fields.push(None);
                continue;
            }
            provided.insert(canonical_name.clone(), expr.span);
            bound_fields.push(Some(canonical_name.clone()));

            let is_model = matches!(self.lookup_type_info(type_name), Some(TypeInfo::Model(_)));
            if is_model && self.private_field_is_inaccessible(type_name, field_info) {
                self.errors
                    .push(errors::private_field(type_name, &field_name.node, field_name.span));
                continue;
            }

            self.infer_type_param_bindings(&field_info.ty, &value_ty, &mut type_bindings);
            field_arguments.push((expr, field_info.ty.clone()));

            if !self.types_compatible(&value_ty, &field_info.ty)
                && !self.record_validated_newtype_field_coercion_if_possible(
                    &value_ty,
                    &field_info.ty,
                    &canonical_name,
                    expr.span,
                )
            {
                self.errors.push(errors::field_type_mismatch(
                    &field_name.node,
                    &field_info.ty.to_string(),
                    &value_ty.to_string(),
                    expr.span,
                ));
            }
        }

        // Enforce required fields (those without defaults) are present.
        let mut every_required_field_supplied = true;
        for (field_name, info) in fields {
            if !info.has_default && !provided.contains_key(field_name) {
                every_required_field_supplied = false;
                self.errors.push(errors::missing_required_constructor_field(
                    display_name,
                    field_name,
                    call_span,
                ));
            }
        }

        if every_required_field_supplied {
            self.record_constructor_field_binding_for_lowering(type_name, &bound_fields, &provided, call_span);
        }

        // ---- Fields that bind one type parameter bind it to one type (#1561) ----
        self.finish_constructor_type_bindings(
            type_name,
            display_name,
            &field_arguments,
            type_bindings,
            errors_before_fields,
            call_span,
        )
    }

    /// Finish checked field inference and retain concrete executable owner facts before caller context is applied.
    ///
    /// This supplements the contextual expression type without changing the legacy checker result or introducing
    /// argument inference in Body IR consumers.
    fn finish_constructor_type_bindings(
        &mut self,
        type_name: &str,
        display_name: &str,
        field_arguments: &[(&Spanned<Expr>, ResolvedType)],
        mut type_bindings: std::collections::HashMap<String, ResolvedType>,
        errors_before_fields: usize,
        call_span: Span,
    ) -> ResolvedType {
        let type_params = self
            .lookup_type_info(type_name)
            .map(|info| Self::constructor_hook_owner_type_params(info).to_vec())
            .unwrap_or_default();
        let arguments = field_arguments.iter().map(|(expr, ty)| (*expr, ty)).collect::<Vec<_>>();
        self.unify_argument_type_bindings(&type_params, &arguments, &mut type_bindings);
        if self.errors.len() == errors_before_fields {
            self.refuse_conflicting_type_arguments(display_name, &type_params, &arguments, &type_bindings);
        }
        if self.errors.len() == errors_before_fields {
            self.record_argument_destinations_at_bindings(&type_params, &arguments, &type_bindings);
        }
        self.refuse_unsatisfied_nominal_type_arguments(
            type_name,
            &type_bindings,
            TypeArgumentOrigin::Inferred,
            call_span,
        );

        let result = self.constructor_result_type_with_bindings(type_name, &type_bindings);
        if self.errors.len() == errors_before_fields
            && !type_params.is_empty()
            && type_params.iter().all(|parameter| {
                type_bindings.get(parameter).is_some_and(|ty| {
                    !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
                        && super::generic_bounds::first_open_type_param(ty, &type_params).is_none()
                })
            })
        {
            self.type_info
                .calls
                .inferred_constructor_types
                .insert((call_span.start, call_span.end), result.clone());
        }
        result
    }

    /// Retain the checked constructor owner for a field argument without exporting its private storage identity.
    /// The existing field lookup and lexical type binding supply both canonical identities. A missing binding or
    /// a non-nominal target grants no layout requirement; execution never guesses an owner from a field's spelling.
    fn record_body_ir_constructor_field_requirement(&mut self, type_name: &str, field: &FieldInfo, span: Span) {
        let Some(identity) = field.identity.as_ref() else {
            return;
        };
        let Some(owner) = self.named_type_reference_identity(type_name) else {
            return;
        };
        if identity.kind != incan_semantics_core::SemanticSourceTargetKind::Field
            || !matches!(
                owner.kind,
                incan_semantics_core::SemanticSourceTargetKind::Model
                    | incan_semantics_core::SemanticSourceTargetKind::Class
            )
        {
            return;
        }
        self.type_info
            .calls
            .body_ir_constructor_field_requirements
            .insert((span.start, span.end), (identity.clone(), owner));
    }

    /// Record the resolved argument-to-field binding for one `model`/`class` construction, for Body IR lowering.
    ///
    /// Declared field order lives in the symbol table (`ModelInfo::field_order` / `ClassInfo::field_order`), which
    /// Body IR lowering cannot reach — it holds only a [`TypeCheckInfo`](crate::typechecker::TypeCheckInfo)
    /// reference. Without this fact, lowering would have to re-resolve field aliases and re-derive field order from
    /// AST spelling, duplicating a decision this check has already made.
    ///
    /// Deliberately fail-closed: nothing is recorded unless every written argument resolved to exactly one declared
    /// field and every declared field is either supplied or defaulted. A partial binding is worse than none, because
    /// lowering would represent a construction the source never validly expressed; with no fact recorded, lowering
    /// refuses by name instead. An argument spread records `None` for the same reason -- it is a written argument
    /// that supplies an unknown set of fields, so it must suppress the fact rather than shorten it.
    ///
    /// A recorded binding means the *field binding* is resolvable, not that the construction is otherwise valid: a
    /// private-field or field-type violation is diagnosed separately and leaves the binding recorded. That is safe
    /// because lowering only consumes these facts after a clean check.
    fn record_constructor_field_binding_for_lowering(
        &mut self,
        type_name: &str,
        bound_fields: &[Option<String>],
        provided: &std::collections::HashMap<String, Span>,
        call_span: Span,
    ) {
        let field_order: Vec<String> = match self.lookup_type_info(type_name) {
            Some(TypeInfo::Model(info)) => info.field_order.clone(),
            Some(TypeInfo::Class(info)) => info.field_order.clone(),
            _ => return,
        };

        let mut argument_slots = Vec::with_capacity(bound_fields.len());
        for bound in bound_fields {
            let Some(field_name) = bound.as_deref() else {
                return;
            };
            let Some(slot) = field_order.iter().position(|declared| declared == field_name) else {
                return;
            };
            argument_slots.push(slot);
        }

        let defaulted_slots: Vec<usize> = field_order
            .iter()
            .enumerate()
            .filter_map(|(slot, field_name)| (!provided.contains_key(field_name)).then_some(slot))
            .collect();

        self.type_info.record_constructor_field_binding(
            call_span,
            ConstructorFieldBinding {
                argument_slots,
                defaulted_slots,
                field_count: field_order.len(),
            },
        );
    }

    /// Retain a module-selected class's complete zero-argument default binding from its checked declaration.
    /// Canonical call identity, nongeneric owner and a default on every declared field are mandatory. Missing or
    /// inconsistent field metadata grants no binding; source and legacy constructor tables remain untouched.
    pub(in crate::typechecker::check_expr::calls) fn retain_body_ir_defaulted_module_class(
        &mut self,
        declaration: &TypeInfo,
        span: Span,
    ) {
        let TypeInfo::Class(class) = declaration else { return };
        if !class.type_params.is_empty()
            || class.field_order.len() != class.fields.len()
            || class
                .field_order
                .iter()
                .any(|name| class.fields.get(name).is_none_or(|field| !field.has_default))
        {
            return;
        }
        let Some(owner) = self
            .type_info
            .resolved_identity(span)
            .filter(|owner| owner.kind == incan_semantics_core::SemanticSourceTargetKind::Class)
            .cloned()
        else {
            return;
        };
        self.type_info.calls.body_ir_module_constructor_bindings.insert(
            (span.start, span.end),
            (
                owner,
                ConstructorFieldBinding {
                    argument_slots: Vec::new(),
                    defaulted_slots: (0..class.field_order.len()).collect(),
                    field_count: class.field_order.len(),
                },
            ),
        );
    }

    /// Record Html's native tuple owner only for a checked String payload. Other accepted legacy shapes have no
    /// direct construction fact, so they keep a named refusal instead of becoming Html<String> with another payload.
    pub(in crate::typechecker::check_expr::calls) fn record_body_ir_html_constructor(
        &mut self,
        args: &[CallArg],
        span: Span,
    ) {
        let [CallArg::Positional(value)] = args else {
            return;
        };
        let is_string = match self.type_info.expr_type(value.span) {
            Some(ty) => ty == &ResolvedType::Str,
            None => match &value.node {
                Expr::Literal(Literal::String(_)) => true,
                Expr::Ident(name) => self.lookup_symbol(name).is_some_and(
                    |symbol| matches!(&symbol.kind, SymbolKind::Variable(variable) if variable.ty == ResolvedType::Str),
                ),
                _ => false,
            },
        };
        if is_string && let Some(owner) = self.stdlib_cache.native_web_identity(SurfaceTypeId::Html) {
            self.type_info.record_body_ir_native_constructor(span, owner);
            self.type_info
                .calls
                .body_ir_native_argument_types
                .insert((value.span.start, value.span.end), ResolvedType::Str);
            if let Expr::Ident(name) = &value.node
                && let Some(identity) = self
                    .symbols
                    .lookup(name)
                    .and_then(|id| self.symbols.identity_of(id))
                    .cloned()
            {
                self.type_info
                    .calls
                    .body_ir_native_argument_identities
                    .insert((value.span.start, value.span.end), identity);
            }
        }
    }

    /// Type-check a JSON/Query constructor call (`Json(...)` / `Query(...)`).
    ///
    /// NOTE: This method is called from multiple dispatch points in the typechecker because calls can be classified
    /// differently by the parser (bare identifier call, constructor call, builtin call, or model/class constructor).
    /// Each dispatch point returns early after handling, preventing double-checking.
    pub(in crate::typechecker::check_expr::calls) fn check_json_query_constructor_call(
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
                CallArg::Named(name, e) if name.node == "value" => {
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
        } else if let Some(owner) = self.stdlib_cache.native_web_identity(tid) {
            self.type_info.record_body_ir_native_constructor(call_span, owner);
        }

        ResolvedType::Generic(surface_types::as_str(tid).to_string(), vec![inner])
    }
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

    /// Typecheck `Enum.from_value(...)` against the value enum backing type.
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
        option_ty(ResolvedType::Named(enum_name.to_string()))
    }

    /// Typecheck `value()` calls on value enum instances.
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
                        self.retain_body_ir_newtype_payload(name, &newtype.type_params, &type_bindings, span);
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
                let ctor_fields: Option<std::collections::HashMap<String, FieldInfo>> =
                    self.lookup_type_info(name).and_then(|info| match info {
                        TypeInfo::Model(m) => Some(m.fields.clone()),
                        TypeInfo::Class(c) => Some(c.fields.clone()),
                        _ => None,
                    });
                if let Some(fields) = ctor_fields {
                    self.check_model_or_class_constructor_call(name, name, &fields, args, span)
                } else {
                    self.constructor_result_type(name)
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

    /// Type-check a type constructor call by delegating to its source-defined static `__incan_new` method.
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
        Some(self.check_resolved_generic_method_call(
            TYPE_CONSTRUCTOR_HOOK,
            hook,
            &[],
            args,
            &[],
            span,
            &receiver_ty,
            None,
        ))
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
    fn constructor_hook_method(type_info: &TypeInfo) -> Option<crate::symbols::MethodInfo> {
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
