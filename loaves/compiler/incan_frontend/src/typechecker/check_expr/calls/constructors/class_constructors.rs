//! Model and class constructor calls, constructor field bindings, and HTML, JSON-query and extractor constructors.

use super::*;

impl TypeChecker {
    /// Retain a checked newtype payload's nominal arguments, permitting only parameters owned by the active frame.
    /// Concrete calls become closed carriers; a declaration's own parameter remains explicit for later specialization.
    pub(super) fn retain_body_ir_newtype_payload(
        &mut self,
        name: &str,
        declaration: &crate::symbols::NewtypeInfo,
        value: &Spanned<Expr>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let parameters = &declaration.type_params;
        let mut bindings = bindings.clone();
        if let Some(actual) = self.body_ir_checked_value_type(value.span) {
            let expected = self.rust_inference_type(&declaration.underlying, parameters);
            let actual = self.rust_inference_type(actual, &self.native_call_type_parameters());
            self.infer_type_param_bindings(&expected, &actual, &mut bindings);
        }
        if parameters.is_empty()
            || parameters.iter().any(|parameter| {
                bindings
                    .get(parameter)
                    .is_none_or(|ty| !Self::is_settled_type_argument(ty) && self.active_type_param_name(ty).is_none())
            })
        {
            return;
        }
        self.refuse_unsatisfied_nominal_type_arguments(name, &bindings, TypeArgumentOrigin::Inferred, span);
        let carrier = self.constructor_result_type_with_bindings(name, &bindings);
        self.type_info.calls.body_ir_constructor_types.insert(span, carrier);
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
        self.type_info.calls.body_ir_constructor_types.insert(span, carrier);
    }

    /// Validate model/class constructor arguments against the selected declaration, including RFC 017 coercions for
    /// typed field initializers. The ordered fields belong to that same declaration; module-qualified owners need not
    /// have a lexical type binding.
    pub(in crate::typechecker::check_expr::calls) fn check_model_or_class_constructor_call(
        &mut self,
        type_name: &str,
        display_name: &str,
        fields: &std::collections::HashMap<String, FieldInfo>,
        field_order: &[String],
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
        let is_class = matches!(self.lookup_type_info(type_name), Some(TypeInfo::Class(_)));
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

            let field_value_ty = self
                .explicit_ownership_value_type(&field_info.ty)
                .unwrap_or_else(|| field_info.ty.clone());
            let value_ty = self.check_expr_with_expected(expr, Some(&field_value_ty));
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

            if self.private_field_is_inaccessible(type_name, field_info) {
                let error = if is_class {
                    errors::private_class_constructor_field(
                        field_info.owner.as_deref().map_or(display_name, |owner| owner),
                        &field_name.node,
                        field_name.span,
                    )
                } else {
                    self.private_field_error(type_name, field_info, &field_name.node, field_name.span)
                };
                self.errors.push(error);
                continue;
            }

            self.infer_type_param_bindings(&field_info.ty, &value_ty, &mut type_bindings);
            field_arguments.push((expr, field_info.ty.clone()));

            if !self.types_compatible(&value_ty, &field_value_ty)
                && !self.record_validated_newtype_field_coercion_if_possible(
                    &value_ty,
                    &field_value_ty,
                    &canonical_name,
                    expr.span,
                )
            {
                self.errors.push(errors::field_type_mismatch(
                    &field_name.node,
                    &field_value_ty.to_string(),
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
            self.record_constructor_field_binding_for_lowering(field_order, &bound_fields, &provided, call_span);
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
    pub(super) fn finish_constructor_type_bindings(
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
        self.unify_constructor_argument_type_bindings(&type_params, &arguments, &mut type_bindings);
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
                .insert(call_span, result.clone());
        }
        if self.errors.len() == errors_before_fields {
            let owner = self.named_type_reference_identity(type_name);
            if let Some(templates) = self.constructor_field_templates(type_name, call_span) {
                for ((expr, expected), template) in field_arguments.iter().zip(&templates) {
                    let expected = substitute_resolved_type(expected, &type_bindings);
                    if !type_params.is_empty() {
                        self.retain_inferred_constructor_argument_type(expr, &expected);
                    }
                    self.retain_contextual_literal_type(expr, &expected, owner.as_ref(), Some(template));
                }
            }
        }
        result
    }

    /// Publish a concrete destination for a nested model or class constructor after the enclosing constructor closes
    /// its field binders.
    ///
    /// A phantom parameter cannot be inferred from the nested constructor's own fields. The enclosing checked field
    /// type is therefore the first concrete executable fact. Canonical declaration identity and the recorded field
    /// binding authenticate that destination; other calls and incompatible nominal owners retain their original type.
    fn retain_inferred_constructor_argument_type(&mut self, expression: &Spanned<Expr>, destination: &ResolvedType) {
        let Expr::Call(callee, type_arguments, _) = &expression.node else {
            return;
        };
        if !type_arguments.is_empty()
            || !Self::is_settled_type_argument(destination)
            || self.type_info.constructor_field_binding(expression.span).is_none()
        {
            return;
        }
        let Some(constructor) = self.type_info.resolved_identity(callee.span).cloned() else {
            return;
        };
        if !matches!(
            constructor.kind,
            incan_semantics_core::SemanticSourceTargetKind::Model
                | incan_semantics_core::SemanticSourceTargetKind::Class
        ) {
            return;
        }
        let destination_name = match destination {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return,
        };
        if self.named_type_reference_identity(destination_name).as_ref() != Some(&constructor) {
            return;
        }
        let Some(actual) = self.type_info.expr_type(expression.span) else {
            return;
        };
        if self.types_compatible(actual, destination) {
            self.type_info
                .calls
                .body_ir_constructor_types
                .insert(expression.span, destination.clone());
        }
    }

    /// Read original field templates through the constructor's checked argument slots, before explicit type
    /// arguments can erase which leaves belong to the declaration and which belong to its caller.
    pub(super) fn constructor_field_templates(&self, owner: &str, span: Span) -> Option<Vec<ResolvedType>> {
        let (fields, order) = match self.lookup_type_info(owner)? {
            TypeInfo::Model(info) => (&info.fields, &info.field_order),
            TypeInfo::Class(info) => (&info.fields, &info.field_order),
            _ => return None,
        };
        self.type_info
            .constructor_field_binding(span)?
            .argument_slots
            .iter()
            .map(|slot| Some(fields.get(order.get(*slot)?)?.ty.clone()))
            .collect()
    }

    /// Retain the checked constructor owner for a field argument without exporting its private storage identity.
    /// The existing field lookup and lexical type binding supply both canonical identities. A missing binding or
    /// a non-nominal target grants no layout requirement; execution never guesses an owner from a field's spelling.
    pub(super) fn record_body_ir_constructor_field_requirement(
        &mut self,
        type_name: &str,
        field: &FieldInfo,
        span: Span,
    ) {
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
            .insert(span, (identity.clone(), owner));
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
    pub(super) fn record_constructor_field_binding_for_lowering(
        &mut self,
        field_order: &[String],
        bound_fields: &[Option<String>],
        provided: &std::collections::HashMap<String, Span>,
        call_span: Span,
    ) {
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
            span,
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
        if let Some(owner) = self.surface_type_identity(SurfaceTypeId::Html) {
            self.record_body_ir_html_constructor_with_owner(args, span, &owner);
        }
    }

    /// Record Html's native tuple owner from an already resolved qualified or lexical binding.
    pub(in crate::typechecker::check_expr::calls) fn record_body_ir_html_constructor_with_owner(
        &mut self,
        args: &[CallArg],
        span: Span,
        owner: &CanonicalSymbolId,
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
        if is_string {
            self.type_info.record_body_ir_native_constructor(span, owner.clone());
            self.type_info
                .calls
                .body_ir_native_argument_types
                .insert(value.span, ResolvedType::Str);
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
                    .insert(value.span, identity);
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
        } else if let Some(owner) = self.surface_type_identity(tid) {
            self.type_info
                .record_body_ir_native_constructor(call_span, owner.clone());
            self.retain_extractor_constructor(&owner, args, call_span);
        }

        ResolvedType::Generic(surface_types::as_str(tid).to_string(), vec![inner])
    }

    /// Retain the imported extractor's inspected tuple layout and infer its owner from the checked payload.
    /// Named source value arguments bind the native slot selected by metadata, without re-evaluating their expressions.
    pub(super) fn retain_extractor_constructor(
        &mut self,
        owner: &incan_semantics_core::CanonicalSymbolId,
        args: &[CallArg],
        span: Span,
    ) {
        let Some(path) = incan_semantics_core::body_ir::native_nominal_item(owner) else {
            return;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(&path) else {
            return;
        };
        let incan_lang::interop::RustItemKind::Type(info) = &metadata.kind else {
            return;
        };
        if info.fields.len() == 1 && info.fields[0].name == "0" {
            self.retain_rust_construction(&path, info, args, &["0".into()], span);
        }
    }
}
