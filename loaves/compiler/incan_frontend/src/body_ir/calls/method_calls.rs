//! Method calls: receiver bindings, special and bound forms, receivers, evaluated callables and static methods.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Complete a caller-open receiver from one retained source nominal and its physical checked method signature.
    ///
    /// Existing call identities remain authoritative. A generic owner, overloaded member, or method without an
    /// ordinary immutable instance contract stays refused; selecting a concrete source method here does not infer
    /// runtime dispatch or reconstruct a canonical identity from its spelling.
    pub(super) fn retained_receiver_method_binding(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> Option<&'type_info crate::typechecker::FunctionBindingInfo> {
        if self.type_info.resolved_identity(span).is_some() {
            return None;
        }
        let recorded = self
            .type_info
            .expr_type(receiver.span)
            .map(|ty| self.checked_type(ty))?;
        if !self.leaves_type_open(&recorded) {
            return None;
        }
        let IncanType::Named(owner_name) = self.resolve_ty(receiver.span) else {
            return None;
        };
        let owner = super::parameter_types::retained_nominal_identity(&owner_name, self.type_info)?;
        self.local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == owner && declaration.type_parameters.is_empty())?;
        let selected = collect::checked_slot_method_name(self.type_info, owner, name);
        let mut candidates = self
            .local_receiver_methods
            .iter()
            .filter(|method| &method.owner == owner && method.name == selected);
        let method = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        let facts: &'type_info TypeCheckInfo = self.type_info;
        let binding = facts.declarations.method_bindings_by_span.get(&method.span)?;
        let identity = binding.identity.as_ref()?;
        (identity.kind == SemanticSourceTargetKind::Method).then_some(binding)
    }

    /// Lower a checked method expression. An ordinary instance call prepends its receiver as a shared or exclusive
    /// borrow operand.
    ///
    /// Argument binding goes through the same [`plan_declared_args`] planner every other call shape uses, against
    /// the typechecker's own rest-aware call-site signature for this span -- which already has the receiver's
    /// generic arguments substituted, so a generic method's slots are concrete here. The receiver is deliberately
    /// outside the recorded binding: its slots index the method's declared parameters, so a consumer reads
    /// `args[0]` as the receiver and `args[1..]` as the bound arguments. A method call whose signature the
    /// typechecker did not record still lowers positional arguments faithfully and refuses only the spellings it
    /// cannot bind — a named spelling with no spread beside it — matching [`Self::lower_call`]'s treatment of an
    /// unresolved direct callee. Spread arguments lower here too, after the receiver.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::body_ir) fn lower_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
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
        if let Some(value) = self.lower_initial_method_forms(recv, name, &call, out) {
            return value;
        }
        self.settle_open_call_arguments(span, args);
        if let Some(value) = self.lower_special_method_forms(recv, name, &call, out) {
            return value;
        }
        self.lower_bound_method_call(recv, name, &call, out)
    }

    /// Lower checked foreign, bridge, enum and isinstance forms before open arguments are settled.
    pub(super) fn lower_initial_method_forms(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let (type_args, args, span, scope) = (call.type_args, call.args, call.span, call.scope);
        if let Some(owner) = self.type_info.body_ir_native_constructor_identity(span)
            && incan_semantics_core::body_ir::native_nominal_item(owner)
                .is_some_and(|path| self.type_info.rust.checked_layouts.contains_key(&path))
        {
            let owner_name = owner.declaration_name.clone();
            return Some(self.lower_nominal_construction(&owner_name, args, span, span, scope, out));
        }
        if let Some(value) = self.lower_checked_foreign_call(args, span, scope, out) {
            return Some(value);
        }
        if let Some(value) = self.lower_checked_bridge_call(recv, args, span, scope, out) {
            return Some(value);
        }
        if let Some(value) = self.lower_checked_rust_call(Some(recv), args, span, scope, out) {
            return Some(value);
        }
        if let Some(target) = self.checked_enum_variant_target(recv, name, span) {
            return Some(self.lower_enum_variant_call(target, args, span, scope, out));
        }
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return Some(self.lower_checked_isinstance_call(type_args, args, span, scope, out));
        }
        None
    }

    /// Lower retained builtin, module, metadata, static and newtype forms after argument settlement.
    pub(super) fn lower_special_method_forms(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let (type_args, args, span, scope) = (call.type_args, call.args, call.span, call.scope);
        if let Some(lowered) = self.lower_checked_builtin_method_call(type_args, args, span, scope, out) {
            return Some(lowered);
        }
        if let Some(lowered) = self.lower_list_repeat_call(recv, name, type_args, args, span, scope, out) {
            return Some(lowered);
        }
        if type_args.is_empty()
            && let Some(lowered) = self.lower_text_codec_call(recv, args, span, scope, out)
        {
            return Some(lowered);
        }
        if let Some(lowered) = self.lower_module_qualified_call(recv, name, type_args, args, span, scope, out) {
            return Some(lowered);
        }
        if let Some(value) = self.lower_type_member_metadata(recv, type_args, args, span, scope, out) {
            return Some(value);
        }
        if self.static_receiver_type(recv, span).is_some() {
            return Some(self.lower_static_method_call(recv, name, type_args, args, span, scope, out));
        }
        if let Some(sum) = self.lower_newtype_sum(recv, name, type_args, args, span, scope, out) {
            return Some(sum);
        }
        None
    }

    /// Bind an ordinary method's retained receiver and declaration facts before selecting its final runtime target.
    pub(super) fn lower_bound_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        call: &CallLoweringSite<'_>,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let (type_args, args, span, scope, hir_span_value) =
            (call.type_args, call.args, call.span, call.scope, call.hir_span);
        let retained_binding = self.retained_receiver_method_binding(recv, name, span);
        let helper = match self.checked_string_helper_for_method_call(recv, name, span) {
            Ok(helper) => helper,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };

        let MethodCallBinding {
            canonical,
            declared,
            native_member,
            retained_method,
        } = self.method_call_binding(recv, name, call, retained_binding);

        let intrinsic_carrier = self.checked_intrinsic_carrier(recv, name);
        let (receiver_operand, mut arg_operands, mut binding) =
            match self.bind_explicit_method_input(canonical.as_ref(), recv, args, span, scope, out) {
                Some(Ok(bound)) => bound,
                Some(Err(description)) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
                None => {
                    let receiver_operand =
                        self.lower_method_receiver(recv, name, intrinsic_carrier.as_deref(), span, scope, out);
                    match self.bind_declared_args(&format!("method `{name}`"), declared, args, scope, out) {
                        Ok((operands, binding)) => (receiver_operand, operands, binding),
                        Err(description) => {
                            return self.unsupported_operand(description, scope, hir_span_value, out);
                        }
                    }
                }
            };
        self.retain_trait_partial_presets(name, canonical.as_ref(), &mut arg_operands, &mut binding);
        arg_operands = self.retain_list_probe_carrier(recv, args, arg_operands, span, scope, out);

        // The receiver is `args[0]` and is never spliced, so it is always a single-value element.
        let mut call_args = Vec::with_capacity(arg_operands.len() + 1);
        call_args.push(bir::ArgumentElement::One(receiver_operand));
        call_args.append(&mut arg_operands);
        self.retain_result_call_channels(span, recv.span, &call_args);
        let checked = self.method_call_result_type(span);
        let ty = native_member.as_ref().map_or_else(
            || match retained_binding {
                Some(binding) if self.leaves_type_open(&checked) => self.checked_type(&binding.return_type),
                _ => checked,
            },
            |member| member.return_type.clone(),
        );
        let ty = retained_method.as_ref().map_or(ty, |method| method.return_type.clone());
        let async_target = self
            .type_info
            .calls
            .body_ir_async_call_targets
            .get(&span)
            .is_some_and(|identity| canonical.as_ref() == Some(identity));
        let ty = if async_target {
            IncanType::Generic {
                base: "Awaitable".into(),
                args: vec![ty],
            }
        } else {
            ty
        };
        if let Some(helper) = helper {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                call_args,
                ty,
                scope,
                hir_span_value,
                false,
                out,
            );
        }
        let target = bir::MethodTarget {
            native_trait_delegation: None,
            reflection: self.checked_reflection_operation(span),
            name: name.to_string(),
            intrinsic_carrier,
            loaf_trait: None,
            trait_arguments: self
                .checked_trait_arguments(self.type_info.resolved_method_call(span).map(|call| &call.dispatch)),
            // Checked provider and inferred targets precede retained source aliases and registry numeric helpers.
            canonical,
            type_args: resolved_type_args,
            binding,
        };
        self.finish_bound_method_call(target, call_args, ty, async_target, span, scope, hir_span_value, out)
    }

    /// Emit a fully bound source method call, preserving direct and carried returned-view representations. Async
    /// direct views keep their physical future reference until `await` restores the authenticated source-place alias.
    #[allow(clippy::too_many_arguments)]
    fn finish_bound_method_call(
        &mut self,
        target: bir::MethodTarget,
        call_args: Vec<bir::ArgumentElement>,
        ty: IncanType,
        async_target: bool,
        span: ast::Span,
        scope: bir::ScopeId,
        hir_span_value: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let receiver_argument = match call_args.first() {
            Some(bir::ArgumentElement::One(operand)) => Some(operand.clone()),
            _ => None,
        };
        if let Some(view) =
            self.view_call_place(span, receiver_argument.as_ref(), call_args.get(1..).unwrap_or_default())
        {
            return match view {
                Ok((place, access)) if async_target => {
                    self.awaited_view_places.insert(span, (place, access));
                    match super::returned_views::async_reference_carrier(ty, access) {
                        Ok(ty) => self.push_call_temp(
                            bir::Callee::Method(target),
                            call_args,
                            ty,
                            scope,
                            hir_span_value,
                            false,
                            out,
                        ),
                        Err(description) => self.unsupported_operand(description, scope, hir_span_value, out),
                    }
                }
                Ok((place, access)) => self.push_view_call(
                    bir::Callee::Method(target),
                    call_args,
                    place,
                    access,
                    hir_span_value,
                    out,
                ),
                Err(description) => self.unsupported_operand(description, scope, hir_span_value, out),
            };
        }
        // A carried view's call holds its callee's carrier, whose carrying payload is the view's reference; `?` and a
        // match continue on the substituted place recorded here.
        let ty = match self.carried_view_place(span, receiver_argument.as_ref(), call_args.get(1..).unwrap_or_default())
        {
            Some(Ok((place, access, payload))) => {
                self.carried_view_places.insert(span, (place, access, payload));
                super::returned_views::reference_carrier(ty, access, payload)
            }
            Some(Err(description)) => return self.unsupported_operand(description, scope, hir_span_value, out),
            None => ty,
        };
        self.push_call_temp(
            bir::Callee::Method(target),
            call_args,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Select method identity and declaration slots in checked precedence order before evaluating the receiver.
    pub(super) fn method_call_binding(
        &self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        call: &CallLoweringSite<'_>,
        retained_binding: Option<&crate::typechecker::FunctionBindingInfo>,
    ) -> MethodCallBinding {
        let (type_args, args, span) = (call.type_args, call.args, call.span);
        let native_member = super::runtime_nominals::member(self.type_info, &self.resolve_ty(recv.span), name);
        let canonical = native_member
            .as_ref()
            .map(|member| member.canonical.clone())
            .or_else(|| self.type_info.calls.body_ir_string_slice_members.get(&span).cloned())
            .or_else(|| {
                self.type_info
                    .resolved_identity(span)
                    .or_else(|| self.type_info.traits.inferred_method_identities.get(&span))
                    .or_else(|| self.type_info.traits.self_method_identities.get(&span))
                    .or_else(|| retained_binding.and_then(|binding| binding.identity.as_ref()))
                    .cloned()
            });
        // Open Self calls bind to retained trait slots; receiver-only SDK members supply an empty ordinary signature.
        let declared = self
            .native_source_method_slots(span)
            .or_else(|| {
                self.type_info
                    .traits
                    .self_method_params
                    .get(&span)
                    .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
            })
            .or_else(|| {
                retained_binding.map(|binding| binding.params.iter().map(DeclaredSlot::from_checked_param).collect())
            })
            .or_else(|| native_member.as_ref().map(|_| Vec::new()));
        let retained_method = self.retained_alias_method(recv, name, type_args, args, span);
        let canonical = canonical
            .or_else(|| retained_method.as_ref().map(|method| method.canonical.clone()))
            .or_else(|| self.direct_builtin_method_identity(&self.resolve_ty(recv.span), name));
        let declared = declared.or_else(|| retained_method.as_ref().map(|_| Vec::new()));
        MethodCallBinding {
            canonical,
            declared,
            native_member,
            retained_method,
        }
    }

    /// Retain the existing checked standard-carrier method profile once for both receiver evaluation and dispatch.
    /// User-defined methods and other carriers keep their canonical source method path.
    pub(super) fn checked_intrinsic_carrier(&self, receiver: &ast::Spanned<ast::Expr>, name: &str) -> Option<String> {
        match self.resolve_ty(receiver.span).collection()?.0 {
            carrier @ CollectionTypeId::Option if matches!(name, "is_some" | "is_none" | "unwrap" | "unwrap_or") => {
                Some(collections::as_str(carrier).to_string())
            }
            carrier @ CollectionTypeId::Result if matches!(name, "is_ok" | "is_err" | "unwrap" | "unwrap_or") => {
                Some(collections::as_str(carrier).to_string())
            }
            _ => None,
        }
    }

    /// Read consuming carriers before their defaults and borrow checked immutable constant values from typed
    /// temporaries. Result transformations retain the same ownership policy, so fresh native errors need no Clone
    /// implementation. Ordinary receivers retain borrowed places. Real global storage stays delayed because legacy
    /// evaluates a static wrapper's arguments before locking and cloning storage. Every method receiver retains the
    /// checker's selected mut-self fact, including projected static receivers.
    pub(super) fn lower_method_receiver(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        intrinsic_carrier: Option<&str>,
        call_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let transforms_result = matches!(
            self.checked_result_method(call_span),
            Some(
                ResultMethodId::Map
                    | ResultMethodId::MapErr
                    | ResultMethodId::AndThen
                    | ResultMethodId::OrElse
                    | ResultMethodId::Inspect
                    | ResultMethodId::InspectErr
            )
        );
        if transforms_result || (intrinsic_carrier.is_some() && matches!(name, "unwrap" | "unwrap_or")) {
            return self.lower_consumed_carrier_receiver(receiver, scope, out);
        }
        if let ast::Expr::Field(base, _member) = &receiver.node
            && self.local_value_enum_variant_target(base, receiver.span).is_some()
        {
            return self.lower_expr_to_operand(receiver, scope, out);
        }
        let mut place = self.lower_expr_to_place(receiver, scope, out);
        // Immutable published constants have values, not retained global storage. Materialize the checked value before
        // borrowing a method receiver; real statics preserve their delayed storage access below.
        let receiver_ty = self.resolve_ty(receiver.span);
        if let Some(constant) = self.checked_constant(&place, &receiver_ty) {
            let span = hir_span(receiver.span);
            let local = self.new_temp(receiver_ty, scope, span);
            place = bir::Place::from_local(local);
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: place.clone(),
                    rvalue: bir::Rvalue::Use(bir::Operand::Constant(constant)),
                },
                span,
            });
        }
        let key = call_span;
        let mutable = self
            .type_info
            .resolved_identity(call_span)
            .is_some_and(collection_method_changes_receiver)
            || self.type_info.calls.mutable_receiver_method_calls.contains(&key);
        let fact = if mutable {
            bir::OwnershipFact::MutBorrow
        } else {
            bir::OwnershipFact::Borrow
        };
        bir::Operand::place(place, fact, false)
    }

    /// Materialize a consuming local, field or mutable-parameter carrier under its checked ownership facts, so later
    /// argument effects cannot change its read. A field cannot move out of its owner, so the retained value is a clone,
    /// as legacy's consumed-receiver policy requires. An enclosing local read inside a loop stays owned for the next
    /// iteration even at its final textual occurrence. Global reads retain their static access boundary instead.
    pub(super) fn lower_consumed_carrier_receiver(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let ty = self.resolve_ty(receiver.span);
        let mut value = self.lower_expr_to_operand(receiver, scope, out);
        if let bir::Operand::Place(read) = &mut value {
            if read.place.global().is_some() {
                read.fact = bir::OwnershipFact::Borrow;
                return value;
            }
            if read.fact == bir::OwnershipFact::Borrow {
                read.fact = bir::OwnershipFact::Clone;
            }
            if read.fact == bir::OwnershipFact::Move
                && !self.loop_break_targets.is_empty()
                && let Some(local) = read.place.local_id()
                && self.locals[local.index()].scope != scope
            {
                read.fact = bir::OwnershipFact::Clone;
                read.last_use = false;
                self.moved_out.remove(&local);
            }
        }
        self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(receiver.span), out)
    }

    /// Invoke a checked callable expression through its evaluated value, retaining source evaluation order.
    ///
    /// The expression's retained function signature owns argument binding. A call result or closure expression is an
    /// owned value evaluated before its arguments. Reads of existing callable storage, explicit type applications,
    /// rest parameters and open carriers remain named refusals; no declaration is selected by source spelling.
    pub(super) fn lower_evaluated_callable_call(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if !matches!(
            expression.node,
            ast::Expr::Call(..) | ast::Expr::MethodCall(..) | ast::Expr::Closure(..)
        ) {
            return self.unsupported_operand(
                "callable expression without a retained owned evaluation".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let ty = self.resolve_ty(expression.span);
        let IncanType::Function { params, .. } = &ty else {
            return self.unsupported_operand(
                "callable expression without a checked function signature".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if !type_args.is_empty() || self.leaves_type_open(&ty) {
            return self.unsupported_operand(
                "callable expression with an unretained type application".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        if params
            .iter()
            .any(|parameter| parameter.kind != IncanCallableParamKind::Normal)
        {
            return self.unsupported_operand(
                "callable expression with rest parameters".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let slots = params.iter().map(DeclaredSlot::from_semantic_param).collect::<Vec<_>>();
        let planned = match plan_declared_args("callable expression", &slots, args) {
            Ok(planned) => planned,
            Err(description) => return self.unsupported_operand(description, scope, hir_span_value, out),
        };

        // ---- Context: freeze the callable before evaluating its arguments ----
        let callable = self.lower_expr_to_operand(expression, scope, out);
        let callable = self.push_assign_temp(bir::Rvalue::Use(callable), ty, scope, hir_span(expression.span), out);
        let bir::Operand::Place(operand) = callable else {
            return self.unsupported_operand(
                "callable expression without a retained value".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };

        // ---- Context: bind the physical arguments and invoke that evaluated value ----
        let (arguments, binding) = match self.lower_planned_args(
            &planned,
            slots.len(),
            &slots.iter().map(|slot| slot.is_mut).collect::<Vec<_>>(),
            &slots.iter().map(|slot| slot.ty.clone()).collect::<Vec<_>>(),
            scope,
            out,
        ) {
            Ok(bound) => bound,
            Err(description) => return self.unsupported_operand(description, scope, hir_span_value, out),
        };
        let mut arguments = fixed_elements(arguments);
        let shared = slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| matches!(slot.ty, IncanType::Ref(_)).then_some(index))
            .collect::<Vec<_>>();
        self.share_argument_slots(&mut arguments, &binding, &shared);
        let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
            operand,
            binding,
        }));
        self.push_call_temp(
            callee,
            arguments,
            self.resolve_ty(span),
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Whether the member read at `span` is a computed property the checker resolved, rather than a stored field.
    pub(in crate::body_ir) fn is_property_read(&self, span: ast::Span) -> bool {
        self.type_info
            .resolved_identity(span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Property)
    }

    /// Retain reflection only when the checked canonical member belongs to the compiler-generated owner table.
    /// Generic-binder ownership selects string field values or generic metadata before substitution; concrete
    /// ownership selects typed values and exact-declaration metadata.
    pub(super) fn checked_reflection_operation(&self, span: ast::Span) -> Option<bir::ReflectionOperation> {
        let identity = self.type_info.resolved_identity(span)?;
        let owner = self
            .type_info
            .calls
            .compiler_generated_member_identities
            .get(identity)?;
        let generic = owner.kind == SemanticSourceTargetKind::GenericBinder;
        match (identity.declaration_name.as_str(), generic) {
            ("__fields__", false) => Some(bir::ReflectionOperation::Metadata),
            ("__fields__", true) => Some(bir::ReflectionOperation::GenericMetadata(owner.clone())),
            ("__field_value__", false) => Some(bir::ReflectionOperation::TypedValue),
            ("__field_items__", false) => Some(bir::ReflectionOperation::TypedItems),
            ("__field_value__", true) => Some(bir::ReflectionOperation::StringValue),
            ("__field_items__", true) => Some(bir::ReflectionOperation::StringItems),
            _ => None,
        }
    }

    /// Retain a checked builtin list lookup's numeric element carrier even when its legacy signature table is absent.
    ///
    /// The canonical builtin member and list-method registry authorize the single probe slot. The receiver's checked
    /// element type supplies its destination; a user method with the same spelling and unshaped spreads stay unchanged.
    pub(super) fn retain_list_probe_carrier(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        mut elements: Vec<bir::ArgumentElement>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let Some(identity) = self.type_info.resolved_identity(span) else {
            return elements;
        };
        if identity.origin != SymbolOrigin::Builtin {
            return elements;
        }
        let Some(member) = identity.declaration_name.strip_prefix("List.") else {
            return elements;
        };
        if !matches!(
            list_methods::from_str(member),
            Some(
                list_methods::ListMethodId::Count
                    | list_methods::ListMethodId::Contains
                    | list_methods::ListMethodId::Index
            )
        ) {
            return elements;
        }
        let [argument] = args else { return elements };
        let expression = match argument {
            ast::CallArg::Positional(expression) | ast::CallArg::Named(_, expression) => expression,
            _ => return elements,
        };
        if elements.len() != 1 {
            return elements;
        }
        let receiver_type = self.resolve_ty(receiver.span);
        let Some([element_type]) = receiver_type.collection_arguments(CollectionTypeId::List) else {
            return elements;
        };
        match elements.pop() {
            Some(bir::ArgumentElement::One(operand)) => elements.push(bir::ArgumentElement::One(
                self.retain_numeric_destination(expression, operand, element_type, scope, out),
            )),
            Some(other) => elements.push(other),
            None => {}
        }
        elements
    }

    /// Lower `Type.method(args)`, a call to a type's static method, as a direct call to the method's body.
    ///
    /// A static method has no receiver, so it is called exactly like a function: the checker's selected method is
    /// the canonical identity, and when that method is declared in this module its declaration span is the
    /// `direct_call_id` the method's own [`bir::Body`] carries. A selection that is not a method (an enum variant
    /// constructor, say, or a member the checker did not resolve) refuses rather than being called by its spelling.
    /// An applied receiver already carries its owner arguments, so its checked method instantiation supplies only
    /// method binders. Bare factory receivers still need the supplemental checked owner frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_static_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let spelling = match &recv.node {
            ast::Expr::Ident(owner) => format!("{owner}.{name}"),
            _ => name.to_string(),
        };
        let canonical = self
            .type_info
            .resolved_identity(span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Method)
            .cloned();
        let Some(canonical) = canonical else {
            return self.unsupported_operand(
                format!("static member `{spelling}` without a resolved method declaration"),
                scope,
                hir_span_value,
                out,
            );
        };
        let receiver_type = self.static_receiver_type(recv, span);
        let method_arguments = self
            .type_info
            .calls
            .call_site_monomorph_type_args
            .get(&span)
            .filter(|_| matches!(receiver_type, Some(IncanType::Generic { .. })));
        let resolved_type_args = match method_arguments
            .map(|arguments| Ok(arguments.iter().map(|ty| self.checked_type(ty)).collect()))
            .unwrap_or_else(|| self.call_site_type_arguments(span, type_args))
        {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };
        let direct_call_id = self
            .type_info
            .declarations
            .method_bindings_by_span
            .iter()
            .find(|(_, binding)| binding.identity.as_ref() == Some(&canonical))
            .map(|(span, _)| CompilerNodeId::declaration_at(self.module_identity, *span));
        let declared = self.native_source_method_slots(span);
        let (operands, binding) =
            match self.bind_declared_args(&format!("static method `{spelling}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type,
                name: spelling,
                direct_call_id,
                canonical: Some(canonical),
                builtin: None,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Use an atomic accepted owner instance before ordinary shared signature facts, preserving the same binder.
    pub(super) fn native_source_method_slots(&self, span: ast::Span) -> Option<Vec<DeclaredSlot>> {
        self.type_info
            .body_ir_newtypes
            .callable_params(span, self.type_info)
            .or_else(|| self.type_info.body_ir_call_site_callable_params(span))
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
    }

    /// Return the helper operation selected by a checked string-method call, or a source-span-preserving refusal.
    ///
    /// Body IR only maps the retained [`StringMethodId`](incan_lang::lang::surface::string_methods::StringMethodId)
    /// to a [`bir::HelperOp`]. The source registry is consulted solely to validate that an admitted spelling still
    /// agrees with the checked identity, so a missing or corrupted fact cannot turn a raw method name into runtime
    /// dispatch.
    pub(super) fn checked_string_helper_for_method_call(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> Result<Option<bir::HelperOp>, String> {
        let checked = self.type_info.resolved_string_helper_call(span);
        let string_receiver = matches!(
            self.resolve_ty(receiver.span),
            IncanType::Primitive(IncanPrimitiveType::Str | IncanPrimitiveType::FrozenStr)
        );
        if !string_receiver {
            return match checked {
                Some(_) => Err("checked string helper identity has a non-string receiver".to_string()),
                None => Ok(None),
            };
        }
        let source = incan_lang::lang::surface::string_methods::from_str(name);
        match checked {
            Some(method) => {
                let Some(helper) = bir::HelperOp::for_selected_string_method(method) else {
                    return Err(format!("unadmitted checked string helper identity `{method:?}`"));
                };
                if source != Some(method) {
                    return Err("checked string helper identity does not match the source method".to_string());
                }
                Ok(Some(helper))
            }
            None if source.and_then(bir::HelperOp::for_selected_string_method).is_some() => {
                Err("selected string helper call is missing its checked string helper identity".to_string())
            }
            None => Ok(None),
        }
    }
}
