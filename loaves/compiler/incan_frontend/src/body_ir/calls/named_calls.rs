//! Call lowering entry points and named, intrinsic, local-callable and declared calls, and display arguments.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower a call to a locally held callable value, a nominal construction, or a direct named function.
    ///
    /// A bare identifier that resolves to one of this body's locals is deliberately a
    /// [`bir::CallableTarget::Local`] call: it carries the local read's ownership fact, so a closure's lexical
    /// environment is not lost by pretending the identifier were a declaration. Its callable signature also
    /// enforces the stored value's fixed callable contract before any call arguments are lowered. An identifier the
    /// typechecker resolved to a `model`/`class` construction lowers to a constructor aggregate instead of a call
    /// (see [`Self::lower_nominal_construction`]) -- construction is not invocation, and representing it as a call
    /// would invite a consumer to execute it as one. Any other bare identifier remains a direct
    /// [`bir::Callee::Function`] call.
    ///
    /// Every one of those paths binds its arguments through the same [`plan_declared_args`] planner and records the
    /// result as a [`bir::ArgumentBinding`], so named, out-of-order, and defaulted spellings resolve identically
    /// regardless of how the callee was reached. A direct call whose signature the typechecker did not resolve
    /// (notably a builtin) still lowers its arguments faithfully, recording
    /// [`bir::ArgumentBinding::UnresolvedPositional`], and refuses only a named spelling it cannot bind without one.
    /// Argument spreads lower as [`bir::ArgumentElement::Spread`] elements, since a spread has no declared slot to
    /// bind to by construction. A checked callable expression is evaluated into a local target before its arguments;
    /// missing signatures and unretained type applications remain explicit unsupported forms.
    pub(in crate::body_ir) fn lower_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let call = CallLoweringSite {
            type_args,
            args,
            span,
            scope,
            hir_span: hir_span(span),
        };
        if let Some(value) = self.lower_known_direct_call(callee, &call, out) {
            return value;
        }
        self.settle_open_call_arguments(span, args);
        let ast::Expr::Ident(name) = &callee.node else {
            return self.lower_evaluated_callable_call(callee, type_args, args, span, scope, out);
        };
        self.lower_named_call(callee, name.clone(), &call, out)
    }

    /// Lower compiler-retained direct-call forms before open argument settlement and ordinary name dispatch.
    pub(super) fn lower_known_direct_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let (type_args, args, span, scope) = (call.type_args, call.args, call.span, call.scope);
        if let Some(value) = self.lower_diverging_error_helper(callee, type_args, args, span, scope, out) {
            return Some(value);
        }
        if let Some(owner) = self.type_info.body_ir_native_constructor_identity(span)
            && incan_semantics_core::body_ir::native_nominal_item(owner)
                .is_some_and(|path| self.type_info.rust.checked_layouts.contains_key(&path))
        {
            let name = owner.declaration_name.clone();
            return Some(self.lower_nominal_construction(&name, args, span, callee.span, scope, out));
        }
        if let Some(value) = self.lower_checked_rust_call(None, args, span, scope, out) {
            return Some(value);
        }
        if let Some(value) = self.lower_testing_raises_call(callee, type_args, args, span, scope, out) {
            return Some(value);
        }
        if let Some(value) = self.lower_type_field_count(type_args, args, span, scope, out) {
            return Some(value);
        }
        if let Some(value) = self.lower_checked_length_call(args, span, scope, out) {
            return Some(value);
        }
        if let Some(error) = self.lower_validation_error_construction(callee, type_args, args, span, scope, out) {
            return Some(error);
        }
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return Some(self.lower_checked_isinstance_call(type_args, args, span, scope, out));
        }
        None
    }

    /// Lower checked List, Set and Dict conversion or empty construction without guessing from the written name.
    pub(super) fn lower_checked_collection_call(
        &mut self,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let (type_args, args, span, scope) = (call.type_args, call.args, call.span, call.scope);
        if type_args.is_empty()
            && let [ast::CallArg::Positional(source)] = args
        {
            match self.type_info.resolved_collection_constructor(span) {
                Some(CollectionTypeId::Set) => {
                    return Some(self.lower_set_constructor_source(source, span, scope, out));
                }
                Some(CollectionTypeId::List) => {
                    return Some(self.lower_list_constructor_source(source, span, scope, out));
                }
                Some(CollectionTypeId::Dict) => {
                    return Some(self.lower_dict_constructor_source(source, span, scope, out));
                }
                _ => {}
            }
        }
        if args.is_empty()
            && type_args.is_empty()
            && let Some(constructor) = self.type_info.resolved_collection_constructor(span)
        {
            match constructor {
                CollectionTypeId::List => {
                    return Some(self.lower_aggregate(bir::AggregateKind::List, &[], span, scope, out));
                }
                CollectionTypeId::Set => {
                    return Some(self.lower_aggregate(bir::AggregateKind::Set, &[], span, scope, out));
                }
                CollectionTypeId::Dict => return Some(self.lower_dict(&[], span, scope, out)),
                _ => {}
            }
        }
        None
    }

    /// Lower a checked identifier call after compiler-known forms and argument settlement have completed.
    pub(super) fn lower_named_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: String,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (args, span, scope) = (call.args, call.span, call.scope);
        if let Some(value) = self.lower_checked_collection_call(call, out) {
            return value;
        }

        // A recorded constructor field binding is the typechecker's own statement that this spelling constructs a
        // nominal value, which is what distinguishes `P(x=1)` from a call to a function that happens to be named
        // `P`. A construction may carry call-site type arguments (`Box[int]()` is accepted), but the typechecker
        // records no monomorphization for them, and the constructed value's own type already carries the resolved
        // arguments -- so this deliberately does not duplicate them on the constructor target rather than claiming
        // construction cannot be generic.
        if self.type_info.body_ir_constructor_field_binding(span).is_some() {
            return self.lower_nominal_construction(&name, args, span, callee.span, scope, out);
        }
        if let Some(value) = self.lower_intrinsic_named_call(callee, &name, call, out) {
            return value;
        }
        self.lower_resolved_named_call(callee, name, call, out)
    }

    /// Lower intrinsic Result and Option payloads; same-spelled source declarations retain ordinary call dispatch.
    pub(super) fn lower_intrinsic_named_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: &str,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let (type_args, args, span, scope, hir_span_value) =
            (call.type_args, call.args, call.span, call.scope, call.hir_span);
        // `Ok` and `Err` are intrinsic Result constructors, not ordinary direct calls. Retain that checked
        // distinction explicitly: a same-spelled source binding (a local callable, local function, or imported
        // target) must remain on the normal call path and refuse unless its own direct callable facts are available.
        // The direct runtime never resolves a constructor name dynamically.
        if type_args.is_empty()
            && let Some(kind) = self.intrinsic_result_constructor(callee, span)
        {
            let result_ty = self.resolve_ty(span);
            let Some((ok_type, error_type)) = result_type_parts(&result_ty) else {
                return Some(self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` without a resolved Result carrier"),
                    scope,
                    hir_span_value,
                    out,
                ));
            };
            let [ast::CallArg::Positional(payload)] = args else {
                return Some(self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` requires one positional payload"),
                    scope,
                    hir_span_value,
                    out,
                ));
            };
            let operand = self.lower_expr_to_operand(payload, scope, out);
            // A Result owns its payload, so a generic or nominal field follows the same clone boundary as a value
            // argument.
            let payload = self.owned_parameter_operand(payload, operand);
            let recorded = self
                .type_info
                .expressions
                .expression_type_identities
                .get(&span)
                .cloned()
                .unwrap_or_default();
            // A side its destination settled can name leaves the checker's own constructor type does not.
            let canonical_types = if self.settled_types.contains_key(&span) {
                self.settled_result_identities(&result_ty, &recorded)
            } else {
                recorded
            };
            return Some(self.push_assign_temp(
                bir::Rvalue::ResultVariant(bir::ResultVariant {
                    kind,
                    payload,
                    ok_type: ok_type.clone(),
                    error_type: error_type.clone(),
                    canonical_types,
                }),
                result_ty,
                scope,
                hir_span_value,
                out,
            ));
        }

        if self.intrinsic_option_constructor(callee)
            && type_args.is_empty()
            && matches!(
                self.resolve_ty(span).collection_arguments(CollectionTypeId::Option),
                Some([_])
            )
            && let [ast::CallArg::Positional(payload)] = args
        {
            let payload = self.lower_expr_to_operand(payload, scope, out);
            return Some(self.push_assign_temp(
                bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, fixed_elements(vec![payload])),
                self.resolve_ty(span),
                scope,
                hir_span_value,
                out,
            ));
        }
        None
    }

    /// Resolve explicit call-site arguments before selecting an operator, local callable or named declaration.
    pub(super) fn lower_resolved_named_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: String,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (type_args, span, scope, hir_span_value) = (call.type_args, call.span, call.scope, call.hir_span);
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // Calling a value whose type adopts `__call__` (a `Callable1` adopter, or a type parameter bounded by one) is a
        // call of that method, as the checker resolved it; the value is the receiver, borrowed like any receiver.
        if let Some(dispatch) = self.type_info.resolved_operator_call(span)
            && dispatch.kind == ResolvedOperatorKind::Call
        {
            return self.lower_call_operator_value(
                callee,
                &name,
                dispatch.method.clone(),
                resolved_type_args,
                call,
                out,
            );
        }
        if let Some(&local) = self.bindings.get(&name) {
            return self.lower_local_callable_value(callee, &name, local, call, out);
        }
        self.lower_declared_named_call(callee, name, resolved_type_args, call, out)
    }

    /// Borrow the checked call-dunder receiver before binding arguments to its retained method slots.
    pub(super) fn lower_call_operator_value(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: &str,
        method: String,
        resolved_type_args: Vec<IncanType>,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (args, span, scope, hir_span_value) = (call.args, call.span, call.scope, call.hir_span);
        let receiver_place = self.lower_expr_to_place(callee, scope, out);
        let receiver = bir::Operand::place(receiver_place, bir::OwnershipFact::Borrow, false);
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let (mut operands, binding) =
            match self.bind_declared_args(&format!("`{method}` of `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
        let mut call_args = Vec::with_capacity(operands.len() + 1);
        call_args.push(bir::ArgumentElement::One(receiver));
        call_args.append(&mut operands);
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                native_trait_delegation: None,
                name: method,
                intrinsic_carrier: None,
                operation: None,
                loaf_trait: None,
                trait_arguments: self
                    .checked_trait_arguments(self.type_info.resolved_method_call(span).map(|call| &call.dispatch)),
                canonical: self.type_info.resolved_identity(span).cloned(),
                type_args: resolved_type_args,
                binding,
            }),
            call_args,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Read a proven live local callable and its ownership fact before lowering its declared arguments.
    pub(super) fn lower_local_callable_value(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: &str,
        local: bir::LocalId,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (args, span, scope, hir_span_value) = (call.args, call.span, call.scope, call.hir_span);
        let checked = self.resolve_ty(callee.span);
        let storage = &self.locals[local.index()].ty;
        let local_ty = if storage.union_members().is_some() {
            checked.clone()
        } else {
            storage.clone()
        };
        let IncanType::Function { params, return_type: _ } = local_ty else {
            return self.unsupported_operand(
                format!("call to non-callable local `{name}`"),
                scope,
                hir_span_value,
                out,
            );
        };
        let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_semantic_param).collect();

        // Source evaluation observes the callable value before its arguments. The target read also performs the
        // one ownership/last-use decision for that lexical environment, which `CallableTarget::Local` preserves
        // for a later executor instead of re-deriving it from the local's source spelling.
        let mut place = bir::Place::from_local(local);
        super::carrier_projection::retain_checked_narrowing(&mut place, &self.locals[local.index()].ty, &checked);
        let (fact, last_use) = self.ownership_fact_for_place(&place, &self.locals[local.index()].ty.clone());
        let target = self.type_info.resolved_identity(callee.span).cloned();
        let explicit = self.bind_explicit_input_args(target.as_ref(), args, span, scope, out);
        let (mut operands, binding) = match explicit.unwrap_or_else(|| {
            self.bind_declared_args(&format!("local callable `{name}`"), Some(slots), args, scope, out)
        }) {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        let shared = params
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| matches!(parameter.ty, IncanType::Ref(_)).then_some(index))
            .collect::<Vec<_>>();
        self.share_argument_slots(&mut operands, &binding, &shared);
        let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
            operand: bir::PlaceOperand { place, fact, last_use },
            binding,
        }));
        let ty = self.resolve_ty(span);
        self.push_call_temp(callee, operands, ty, scope, hir_span_value, false, out)
    }

    /// Resolve the checked declaration and its factory/provider contract before binding a named call.
    pub(super) fn lower_declared_named_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        name: String,
        resolved_type_args: Vec<IncanType>,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (args, span, scope, hir_span_value) = (call.args, call.span, call.scope, call.hir_span);
        // A selected static constructor executes its checked method. Only an ordinary nominal construction without
        // a field binding was declined by the checker and must refuse before generic call lowering.
        let factory = self.checked_nominal_factory_declaration(callee.span, span).is_some();
        if !factory
            && (self.type_info.declarations.class_layouts.contains_key(&name)
                || self.type_info.declarations.model_field_visibilities.contains_key(&name))
        {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let declaration = match self.declared_slots_for_direct_call(&name, callee.span, span) {
            Ok(declaration) => declaration,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // A provider operation is selected by the canonical identity this call already resolved to, never by the
        // spelling written here (#1213). The lookup therefore sits after resolution rather than in place of it, and
        // a callee with no proven identity finds nothing rather than being guessed at by name.
        if let Some(operation) = declaration.canonical.clone()
            && let Some(record) = self.provider_operation_record(&operation)
        {
            return self.lower_provider_operation(&name, &operation, record, declaration, args, span, scope, out);
        }

        let is_async = declaration.is_async;
        if let Some(identity) = &declaration.canonical {
            self.settle_declared_enum_arguments(identity, args);
        }
        let shared_receiver = declaration.canonical.as_ref().is_some_and(|identity| {
            incan_semantics_core::canonical_module_identity(identity).as_deref() == Some(self.module_identity)
                && self
                    .type_info
                    .declarations
                    .method_decorator_receiver_slots
                    .get(&identity.declaration_span)
                    .is_some_and(|slot| {
                        !slot.mutable && slot.role == crate::typechecker::MethodDecoratorReceiverRole::Replacement
                    })
        });
        let legacy_enumerate_argument = legacy_enumerate_argument(declaration.builtin, args);
        let args = legacy_enumerate_argument.as_deref().unwrap_or(args);
        // Constructor syntax writes owner arguments; only the checker's method binders belong on the callee.
        let resolved_type_args = if factory {
            self.checked_factory_method_type_arguments(span)
        } else {
            resolved_type_args
        };
        let sdk_factory = factory
            && self
                .type_info
                .resolved_identity(callee.span)
                .is_some_and(super::is_loaf_identity);
        let bound = if let Some(explicit) =
            self.bind_explicit_input_args(declaration.canonical.as_ref(), args, span, scope, out)
        {
            explicit
        } else if sdk_factory {
            self.bind_sdk_factory_args(&format!("function `{name}`"), declaration.slots, args, scope, out)
        } else if factory {
            self.bind_checked_factory_args(&format!("function `{name}`"), declaration.slots, args, scope, out)
        } else {
            self.bind_declared_args_with_order(&format!("function `{name}`"), declaration.slots, args, scope, out, true)
        };
        let (mut operands, binding) = match bound {
            Ok(bound) => bound,
            Err(description) => {
                if factory {
                    return self.unsupported_typed_operand(
                        description,
                        self.resolve_ty(span),
                        scope,
                        hir_span_value,
                        out,
                    );
                }
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        self.display_selected_arguments(args, &mut operands, scope, out);
        if declaration.builtin == Some(BuiltinFnId::Print) {
            self.display_type_parameter_arguments(args, &mut operands, scope, out);
        }
        if shared_receiver {
            self.share_argument_slots(&mut operands, &binding, &[0]);
        }
        self.finish_named_call(
            callee,
            call,
            BoundNamedCall {
                name,
                canonical: declaration.canonical,
                direct_call_id: declaration.direct_call_id,
                builtin: declaration.builtin,
                type_args: resolved_type_args,
                binding,
                arguments: operands,
                is_async,
            },
            out,
        )
    }

    /// Refine a bound call's checked result without overriding a provenance refusal, then retain its exact named
    /// target.
    pub(super) fn finish_named_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        call: &CallLoweringSite<'_>,
        bound: BoundNamedCall,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (span, scope, hir_span_value) = (call.span, call.scope, call.hir_span);
        // The checked expression type is the eventual output for immediately awaited source calls. Body IR
        // represents construction and awaiting separately, so their intervening local must own the future.
        let checked_return = self.resolve_ty(span);
        // A published signature may refine an open carrier, but it cannot override a checked provenance refusal.
        let returned = if matches!(checked_return, IncanType::Unsupported(_)) {
            checked_return
        } else {
            bound
                .canonical
                .as_ref()
                .and_then(|identity| self.published_callable_returns.get(identity))
                .filter(|ty| **ty != IncanType::Unknown)
                .cloned()
                .unwrap_or(checked_return)
        };
        let mut ty = if bound.is_async {
            IncanType::Generic {
                base: "Awaitable".to_string(),
                args: vec![returned],
            }
        } else {
            returned
        };
        let target = bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
            receiver_type: self.checked_nominal_factory_owner(callee.span, span),
            name: bound.name,
            direct_call_id: bound.direct_call_id,
            canonical: bound.canonical,
            builtin: bound.builtin,
            type_args: bound.type_args,
            binding: bound.binding,
        }));
        // A call returning a view of a parameter's place runs for its effects and continues on the substituted place;
        // a call carrying one holds its carrier, as a method call's does.
        if let Some(view) = self.view_call_place(span, None, &bound.arguments) {
            return match view {
                Ok((place, access)) if bound.is_async => {
                    self.awaited_view_places.insert(span, (place, access));
                    match super::returned_views::async_reference_carrier(ty, access) {
                        Ok(ty) => self.push_call_temp(target, bound.arguments, ty, scope, hir_span_value, false, out),
                        Err(description) => self.unsupported_operand(description, scope, hir_span_value, out),
                    }
                }
                Ok((place, access)) => self.push_view_call(target, bound.arguments, place, access, hir_span_value, out),
                Err(description) => self.unsupported_operand(description, scope, hir_span_value, out),
            };
        }
        match self.carried_view_place(span, None, &bound.arguments) {
            Some(Ok((place, access, payload))) => {
                self.carried_view_places.insert(span, (place, access, payload));
                ty = super::returned_views::reference_carrier(ty, access, payload);
            }
            Some(Err(description)) => return self.unsupported_operand(description, scope, hir_span_value, out),
            None => {}
        }
        if let Some((access, _, _, component)) = self.type_info.selected_aggregate_view(span) {
            if bound.is_async {
                return self.unsupported_operand(
                    "async aggregate returned views need retained future component facts".into(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
            ty = super::returned_views::reference_authored_component(ty, access, &component);
        }
        self.push_call_temp(target, bound.arguments, ty, scope, hir_span_value, false, out)
    }

    /// Replace only operands whose source display method was already selected by the accepting checker.
    pub(super) fn display_selected_arguments(
        &mut self,
        args: &[ast::CallArg],
        operands: &mut [bir::ArgumentElement],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        if args.len() != operands.len() {
            return;
        }
        for (argument, element) in args.iter().zip(operands) {
            let ast::CallArg::Positional(expression) = argument else {
                continue;
            };
            let bir::ArgumentElement::One(operand) = element else {
                continue;
            };
            *operand = self.lower_selected_display_operand(expression.span, operand.clone(), scope, out);
        }
    }

    /// Emit the exact checked string method as an ordinary call, borrowing the evaluated receiver once and retaining
    /// its field projection.
    pub(in crate::body_ir) fn lower_selected_display_operand(
        &mut self,
        span: ast::Span,
        operand: bir::Operand,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some(canonical) = self.type_info.calls.source_display_methods.get(&span).cloned() else {
            return operand;
        };
        let place = self.materialize_operand_to_place(operand, self.resolve_ty(span), scope, hir_span(span), out);
        let receiver = bir::Operand::place(place, bir::OwnershipFact::Borrow, false);
        let target = bir::MethodTarget {
            name: canonical.declaration_name.clone(),
            canonical: Some(canonical),
            trait_arguments: None,
            native_trait_delegation: None,
            operation: None,
            intrinsic_carrier: None,
            loaf_trait: None,
            type_args: Vec::new(),
            binding: bir::ArgumentBinding::resolved_positional(0),
        };
        self.push_call_temp(
            bir::Callee::Method(target),
            vec![bir::ArgumentElement::One(receiver)],
            IncanType::Primitive(IncanPrimitiveType::Str),
            scope,
            hir_span(span),
            false,
            out,
        )
    }

    /// Give each positional `print`/`println` argument whose checked type is a type parameter its display text as
    /// generic code spells it, through the type argument's own display ([`bir::FormatStyle::TypeParameterDisplay`]).
    ///
    /// Legacy prints such a value with `{}`, so a closed instance must not apply the formatting a concrete value of
    /// that type receives (an integral float keeps no fractional part); the argument becomes that one-part formatted
    /// string. Arguments of any other type, and calls whose written arguments do not line up one-to-one with their
    /// lowered elements, are left as lowered.
    pub(super) fn display_type_parameter_arguments(
        &mut self,
        args: &[ast::CallArg],
        operands: &mut [bir::ArgumentElement],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        if args.len() != operands.len() {
            return;
        }
        for (arg, element) in args.iter().zip(operands.iter_mut()) {
            let ast::CallArg::Positional(expr) = arg else {
                continue;
            };
            if !matches!(self.resolve_ty(expr.span), IncanType::TypeVar(_)) {
                continue;
            }
            let bir::ArgumentElement::One(operand) = element else {
                continue;
            };
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("fstring".to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            let displayed = self.push_assign_temp(
                bir::Rvalue::Format(vec![bir::FormatPart::Expr {
                    operand: Box::new(operand.clone()),
                    style: bir::FormatStyle::TypeParameterDisplay,
                }]),
                IncanType::Primitive(IncanPrimitiveType::Str),
                scope,
                hir_span(expr.span),
                out,
            );
            *element = bir::ArgumentElement::One(displayed);
        }
    }

    /// Return the typechecker's callable type for a closure or local partial value that Body IR constructs itself.
    ///
    /// Local partials use the typechecker's canonical full signature with overrideable preset-default slots, so the
    /// binding, its [`bir::Rvalue::Closure`], and a later [`Self::lower_call`] share one arity/default contract.
    pub(in crate::body_ir) fn callable_value_ty(&self, expr: &ast::Spanned<ast::Expr>) -> Option<IncanType> {
        match &expr.node {
            ast::Expr::Closure(_, _) | ast::Expr::Partial(_) => Some(self.resolve_ty(expr.span)),
            _ => None,
        }
    }
}
