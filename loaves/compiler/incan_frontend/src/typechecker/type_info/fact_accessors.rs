//! Recorded fact accessors and call-site fact transfer on `TypeCheckInfo`.

use super::*;

impl TypeCheckInfo {
    /// Return the final compiler-selected type of a binding introduced by an assignment statement.
    pub fn assignment_binding_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.assignment_binding_types.get(&span)
    }

    /// Return the type of the place the value at `span` is written to, if the checker recorded one because the value
    /// needs `Some` layers or a numeric widening there (#1858, RFC 009).
    pub fn value_destination_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expressions.value_destination_types.get(&span)
    }

    /// Return computed-property metadata for a field-access expression, if that access resolved to a property.
    pub fn computed_property_access(&self, span: Span) -> Option<&ComputedPropertyAccessInfo> {
        self.expressions.computed_property_accesses.get(&span)
    }

    /// Record that a field-access expression resolved to a computed property read.
    pub fn record_computed_property_access(&mut self, span: Span, owner_type: &str, property: &str) {
        self.expressions.computed_property_accesses.insert(
            span,
            ComputedPropertyAccessInfo {
                owner_type: owner_type.to_string(),
                property: property.to_string(),
            },
        );
    }

    /// Return the RFC 038 fixed/static unpack plan recorded for an unpack operand, if any.
    pub fn fixed_unpack_plan(&self, span: Span) -> Option<&FixedUnpackPlan> {
        self.calls.fixed_unpack_plans.get(&span)
    }

    /// Return how the identifier expression at `span` resolved in the symbol table.
    pub fn ident_kind(&self, span: Span) -> Option<IdentKind> {
        self.expressions.ident_kinds.get(&span).copied()
    }

    /// Return the canonical identity proven for an imported binding, if import resolution proved one.
    ///
    /// Absent means unproven, never "assume the written import path": see
    /// [`DeclarationArtifacts::resolved_import_identities`].
    pub fn resolved_import_identity(&self, local_name: &str) -> Option<&CanonicalSymbolId> {
        self.declarations.resolved_import_identities.get(local_name)
    }

    /// Select the executable identity of a checked call while preserving a module binding's navigation identity.
    /// A matching imported function refines that module binding; otherwise an existing selected target wins over a
    /// validated reexport fallback. An absent call-site identity stays absent; only checking may publish a proven
    /// import selection at a successful call site. Source shadows never inherit an import's authority.
    pub fn resolved_call_identity<'a>(
        &'a self,
        name: &str,
        span: Span,
        reexport: Option<&'a CanonicalSymbolId>,
    ) -> Option<&'a CanonicalSymbolId> {
        let selected = self.resolved_identity(span);
        let imported = self.resolved_import_identity(name);
        match (selected, imported) {
            (Some(module), Some(function))
                if module.kind == SemanticSourceTargetKind::Module
                    && function.kind == SemanticSourceTargetKind::Function
                    && module.origin == function.origin =>
            {
                Some(function)
            }
            _ => selected.or(reexport),
        }
    }

    /// Return the declaration a module-qualified type annotation resolved to, keyed by its dotted spelling.
    ///
    /// Absent means the checker did not prove a type for that spelling and reported it at the annotation; lowering
    /// must not fall back to the written segments: see [`DeclarationArtifacts::qualified_type_references`].
    pub fn qualified_type_reference(&self, spelling: &str) -> Option<&QualifiedTypeReferenceInfo> {
        self.declarations.qualified_type_references.get(spelling)
    }

    /// Return the target of a non-generic type alias this module declares or imports.
    pub fn type_alias_target(&self, name: &str) -> Option<&ResolvedType> {
        self.declarations.type_alias_targets.get(name)
    }

    /// Return a compiler-proven source target for the expression at `span`, if one was recorded.
    pub fn source_target(&self, span: Span) -> Option<&SourceTargetInfo> {
        self.expressions.source_targets.get(&span)
    }

    /// Return the RFC 120 canonical identity resolved for the reference at `span`, if resolution proved one.
    ///
    /// Absent means unproven — never permission to rebuild an identity from the reference's spelling or from
    /// [`Self::source_target`], which is a string-shaped codegraph projection rather than an identity.
    pub fn resolved_identity(&self, span: Span) -> Option<&CanonicalSymbolId> {
        self.references.resolved_identities.get(&span)
    }

    /// Record the RFC 120 canonical identity proven for one source reference.
    ///
    /// Expression and type-reference checking share this write boundary so consumers never need to know which AST
    /// category produced the reference fact. Callers must pass an identity obtained from the resolved symbol; this
    /// method deliberately does not reconstruct identities from source spellings.
    pub fn record_resolved_identity(&mut self, span: Span, identity: CanonicalSymbolId) {
        self.references.resolved_identities.insert(span, identity);
    }

    /// Return the canonical binding selected for a statement-owned write target.
    pub fn resolved_write_identity(&self, span: Span, name: &str) -> Option<&CanonicalSymbolId> {
        self.references.resolved_write_identities.get(&(span, name.to_string()))
    }

    /// Return the checked type of a statement-owned write target.
    pub fn resolved_write_type(&self, span: Span, name: &str) -> Option<&ResolvedType> {
        self.references.resolved_write_types.get(&(span, name.to_string()))
    }

    /// Record the canonical binding selected for a statement-owned write target.
    pub fn record_resolved_write_identity(
        &mut self,
        span: Span,
        name: &str,
        identity: CanonicalSymbolId,
        ty: ResolvedType,
    ) {
        let key = (span, name.to_string());
        self.references.resolved_write_identities.insert(key.clone(), identity);
        self.references.resolved_write_types.insert(key, ty);
    }

    /// Return whether the `dict.get(key)` call at `span` only has its result read (see
    /// [`ExpressionArtifacts::read_only_dict_lookups`]).
    pub fn is_read_only_dict_lookup(&self, span: Span) -> bool {
        self.expressions.read_only_dict_lookups.contains(&span)
    }

    /// Record that the `dict.get(key)` call at `span` only has its result read.
    pub fn record_read_only_dict_lookup(&mut self, span: Span) {
        self.expressions.read_only_dict_lookups.insert(span);
    }

    /// Return partial projection metadata for a visible partial binding, if collection recorded one.
    pub fn partial_projection(&self, local_name: &str) -> Option<&PartialProjectionInfo> {
        self.declarations.partial_projections.get(local_name)
    }

    /// Record partial projection metadata for a visible partial binding.
    pub fn record_partial_projection(&mut self, projection: PartialProjectionInfo) {
        self.declarations
            .partial_projections
            .insert(projection.name.clone(), projection);
    }

    /// Return overload candidates for one source binding, if any.
    pub fn function_overloads(&self, local_name: &str) -> Option<&[FunctionOverloadInfo]> {
        self.declarations.function_overloads.get(local_name).map(Vec::as_slice)
    }

    /// Record overload candidates for one source binding.
    pub fn record_function_overloads(&mut self, local_name: String, overloads: Vec<FunctionOverloadInfo>) {
        self.declarations.function_overloads.insert(local_name, overloads);
    }

    /// Return the computed const value for `name`, when const evaluation succeeded.
    pub fn const_value(&self, name: &str) -> Option<&ConstValue> {
        self.consts.const_values.get(name)
    }

    /// Return the recorded Rust-boundary argument coercion for the expression at `span`, if any.
    pub fn rust_arg_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust.arg_coercions.get(&span)
    }

    /// Return the validated-newtype coercion recorded for the expression at `span`, if any.
    pub fn validated_newtype_coercion(&self, span: Span) -> Option<&ValidatedNewtypeCoercionInfo> {
        self.expressions.validated_newtype_coercions.get(&span)
    }

    /// Record a typechecker-approved validated-newtype coercion for a source expression span.
    pub fn record_validated_newtype_coercion(&mut self, span: Span, info: ValidatedNewtypeCoercionInfo) {
        self.expressions.validated_newtype_coercions.insert(span, info);
    }

    /// Return rest-aware callable metadata recorded for the full call expression span, if any.
    pub fn call_site_callable_params(&self, span: Span) -> Option<&[CallableParam]> {
        self.calls.call_site_callable_params.get(&span).map(Vec::as_slice)
    }

    /// Return closed checked call parameters for Body IR, preserving the legacy signature when no complete fact exists.
    pub fn body_ir_call_site_callable_params(&self, span: Span) -> Option<&[CallableParam]> {
        self.calls
            .body_ir_call_parameter_types
            .get(&span)
            .map(Vec::as_slice)
            .or_else(|| self.call_site_callable_params(span))
    }

    /// Return the resolved winner-binding type for the `race for` arm whose awaitable is at `span` (#1164).
    pub fn race_arm_binding_type(&self, span: Span) -> Option<&ResolvedType> {
        self.calls.race_arm_binding_types.get(&span)
    }

    /// Record the resolved winner-binding type for one `race for` arm (#1164).
    pub fn record_race_arm_binding_type(&mut self, span: Span, ty: ResolvedType) {
        self.calls.race_arm_binding_types.insert(span, ty);
    }

    /// Return the resolved `model`/`class` construction field binding recorded for `span`, if any (#1158).
    pub fn constructor_field_binding(&self, span: Span) -> Option<&ConstructorFieldBinding> {
        self.calls.constructor_field_bindings.get(&span)
    }

    /// Return checked ordinary, module-default or native tuple slots only to Body IR, leaving legacy's getter
    /// unchanged. A module fallback must still match the exact canonical owner selected at this call.
    pub fn body_ir_constructor_field_binding(&self, span: Span) -> Option<&ConstructorFieldBinding> {
        self.constructor_field_binding(span)
            .or_else(|| {
                self.calls
                    .body_ir_module_constructor_bindings
                    .get(&span)
                    .filter(|(owner, _)| self.resolved_identity(span) == Some(owner))
                    .map(|(_, binding)| binding)
            })
            .or_else(|| {
                self.calls
                    .body_ir_native_constructor_bindings
                    .get(&span)
                    .map(|(_, binding)| binding)
            })
    }

    /// The exact foreign owner checked for a native surface construction; ordinary source calls retain their
    /// existing expression identity instead.
    pub fn body_ir_native_constructor_identity(&self, span: Span) -> Option<&CanonicalSymbolId> {
        self.calls
            .body_ir_native_constructor_bindings
            .get(&span)
            .map(|(owner, _)| owner)
    }

    /// Record a validated native tuple construction in the direct-route table only, never in legacy's slot table.
    pub fn record_body_ir_native_constructor(&mut self, span: Span, owner: CanonicalSymbolId) {
        self.calls.body_ir_native_constructor_bindings.insert(
            span,
            (
                owner,
                ConstructorFieldBinding {
                    argument_slots: vec![0],
                    defaulted_slots: Vec::new(),
                    field_count: 1,
                },
            ),
        );
    }

    /// Record the resolved field binding for one nominal construction call site, including a newtype's tuple slot
    /// (#1158).
    pub fn record_constructor_field_binding(&mut self, span: Span, binding: ConstructorFieldBinding) {
        self.calls.constructor_field_bindings.insert(span, binding);
    }

    /// Return whether the function-typed parameter declared at `span` holds any callable of its type, so lowering
    /// spells it `impl Fn(A) -> R` (#1561).
    pub fn is_closure_holding_param(&self, span: Span) -> bool {
        self.calls.closure_holding_params.contains(&span)
    }

    /// Return whether the function or method whose function return type is written at `span` returns a closure that
    /// captures local values, so lowering spells the return type `impl Fn(A) -> R` (#1561).
    pub fn is_closure_returning_type(&self, span: Span) -> bool {
        self.calls.closure_returning_callables.contains(&span)
    }

    /// Return whether the assignment statement at `span` binds a new local to a capturing callable, whose own type the
    /// binding takes (#1561).
    pub fn binds_capturing_callable(&self, span: Span) -> bool {
        self.calls.capturing_callable_bindings.contains(&span)
    }

    /// Select checked executable context without changing the reference's navigation identity.
    /// Constructor requirements apply only while the exact checked field still occupies this reference site;
    /// replacing its binding cannot reuse an earlier constructor's layout authorization.
    pub(super) fn body_ir_reference_requirement(
        &self,
        span: crate::ast::Span,
        identity: &CanonicalSymbolId,
    ) -> Option<&CanonicalSymbolId> {
        self.calls
            .compiler_generated_member_identities
            .get(identity)
            .or_else(|| {
                self.calls
                    .body_ir_constructor_field_requirements
                    .get(&span)
                    .filter(|(field, _)| field == identity)
                    .map(|(_, owner)| owner)
            })
    }

    /// Return whether `identity` names a compiler-generated member rather than a source declaration.
    pub fn is_compiler_generated_member_identity(&self, identity: &CanonicalSymbolId) -> bool {
        self.calls.compiler_generated_member_identities.contains_key(identity)
    }

    /// Preserve a generated member's exact declaring nominal as its executable context requirement.
    pub fn record_compiler_generated_member_identity(&mut self, identity: CanonicalSymbolId, owner: CanonicalSymbolId) {
        self.calls.compiler_generated_member_identities.insert(identity, owner);
    }

    /// Return the canonical collection constructor selected for one source call.
    pub fn resolved_collection_constructor(&self, span: Span) -> Option<CollectionTypeId> {
        self.calls.resolved_collection_constructors.get(&span).copied()
    }

    /// Return the checked target fact for one compiler-owned `isinstance` call.
    pub fn isinstance_target(&self, call_span: Span) -> Option<&IsInstanceTargetInfo> {
        self.calls.isinstance_targets.get(&call_span)
    }

    /// Return the compiler-owned builtin selected for one checked call.
    pub fn resolved_builtin_call(&self, call_span: Span) -> Option<BuiltinFnId> {
        self.calls.resolved_builtin_calls.get(&call_span).copied()
    }

    /// Return the compiler-owned builtin method selected for one checked call.
    pub fn resolved_builtin_method_call(&self, call_span: Span) -> Option<BuiltinMethodId> {
        self.calls.resolved_builtin_method_calls.get(&call_span).copied()
    }

    /// Return the `message()` call a declared type displays through, or `None` when it has a `Display` of its own or
    /// does not adopt `Error`.
    ///
    /// See [`CallArtifacts::error_message_display_types`].
    pub fn error_message_display_type(&self, type_name: &str) -> Option<&ErrorMessageDisplay> {
        self.calls.error_message_display_types.get(type_name)
    }

    /// Record the `message()` call a declared type displays through.
    pub fn record_error_message_display_type(&mut self, type_name: &str, display: ErrorMessageDisplay) {
        self.calls
            .error_message_display_types
            .insert(type_name.to_string(), display);
    }

    /// Record the compiler-owned builtin selected for one checked call.
    pub fn record_resolved_builtin_call(&mut self, call_span: Span, builtin: BuiltinFnId) {
        self.calls.resolved_builtin_calls.insert(call_span, builtin);
    }

    /// Record the compiler-owned builtin method selected for one checked call.
    pub fn record_resolved_builtin_method_call(&mut self, call_span: Span, builtin: BuiltinMethodId) {
        self.calls.resolved_builtin_method_calls.insert(call_span, builtin);
    }

    /// Record the checked target fact for one compiler-owned `isinstance` call.
    pub fn record_isinstance_target(&mut self, call_span: Span, target: IsInstanceTargetInfo) {
        self.calls.isinstance_targets.insert(call_span, target);
    }

    /// Record the canonical collection constructor selected for one source call.
    pub fn record_resolved_collection_constructor(&mut self, span: Span, constructor: CollectionTypeId) {
        self.calls.resolved_collection_constructors.insert(span, constructor);
    }

    /// Record callable metadata needed by lowering when the callee expression alone cannot carry it.
    pub fn record_call_site_callable_params(&mut self, span: Span, params: &[CallableParam]) {
        if params
            .iter()
            .any(|param| param.kind != ParamKind::Normal || callable_param_needs_boundary_snapshot(&param.ty))
        {
            self.calls.call_site_callable_params.insert(span, params.to_vec());
        }
    }

    /// Record exact callable metadata when overload/source-method resolution selected a concrete callable.
    pub fn record_call_site_callable_params_exact(&mut self, span: Span, params: &[CallableParam]) {
        self.calls.call_site_callable_params.insert(span, params.to_vec());
    }

    /// Record callable metadata required by an explicit lowered dispatch path.
    pub fn record_call_site_callable_params_for_dispatch(&mut self, span: Span, params: &[CallableParam]) {
        self.calls.call_site_callable_params.insert(span, params.to_vec());
    }

    /// Return a typechecker-resolved user-defined operator call for `span`, if any.
    pub fn resolved_operator_call(&self, span: Span) -> Option<&ResolvedOperatorCall> {
        self.calls.resolved_operator_calls.get(&span)
    }

    /// Return a typechecker-resolved method call for `span`, if any.
    pub fn resolved_method_call(&self, span: Span) -> Option<&ResolvedMethodCall> {
        self.calls.resolved_method_calls.get(&span)
    }

    /// Return custom iteration protocol metadata for `span`, if any.
    pub fn protocol_iteration(&self, span: Span) -> Option<&ProtocolIterationInfo> {
        self.protocols.iterations.get(&span)
    }

    /// Record a user-defined operator call that lowering should emit as a direct dunder method call.
    pub fn record_resolved_operator_call(&mut self, span: Span, method: impl Into<String>, kind: ResolvedOperatorKind) {
        self.calls.resolved_operator_calls.insert(
            span,
            ResolvedOperatorCall {
                method: method.into(),
                kind,
            },
        );
    }

    /// Record a resolved method dispatch that lowering should preserve explicitly.
    ///
    /// A standard Iterator source default supplies the typed operation only when receiver checking did not already
    /// select a concrete builtin carrier such as `Generator`; trait resolution must not erase that receiver family.
    pub fn record_resolved_method_call(
        &mut self,
        span: Span,
        method: impl Into<String>,
        dispatch: ResolvedMethodDispatch,
    ) {
        let method = method.into();
        let ResolvedMethodDispatch::Trait {
            trait_name,
            module_path,
            ..
        } = &dispatch;
        if incan_lang::lang::traits::from_str(trait_name) == Some(incan_lang::lang::traits::TraitId::Iterator)
            && standard_iterator_module_path(module_path.as_deref())
            && let Some(method_id) = incan_lang::lang::surface::iterator_methods::from_str(&method)
        {
            self.calls
                .resolved_builtin_method_calls
                .entry(span)
                .or_insert(BuiltinMethodId::Iterator(method_id));
        }
        self.calls
            .resolved_method_calls
            .insert(span, ResolvedMethodCall { method, dispatch });
    }

    /// Remove and return the facts method resolution recorded for the call at `span` (see [`CallSiteFacts`]).
    pub(crate) fn take_call_site_facts(&mut self, span: Span) -> CallSiteFacts {
        let key = span;
        CallSiteFacts {
            explicit_input_call: self.protocols.explicit_input_calls.remove(&key),
            checked_rust_call: self.rust.checked_calls.remove(&key),
            checked_rust_function_value: self.rust.checked_function_values.remove(&key),
            body_ir_reexport_call: self.calls.body_ir_reexport_calls.remove(&key),
            body_ir_string_slice_member: self.calls.body_ir_string_slice_members.remove(&key),
            inferred_constructor_type: self.calls.inferred_constructor_types.remove(&key),
            inferred_call_result_type: self.calls.inferred_call_result_types.remove(&key),
            method_call: self.calls.resolved_method_calls.remove(&key),
            identity: self.references.resolved_identities.remove(&key),
            callable_params: self.calls.call_site_callable_params.remove(&key),
            monomorph_type_args: self.calls.call_site_monomorph_type_args.remove(&key),
            body_ir_constructor_type: self.calls.body_ir_constructor_types.remove(&key),
            body_ir_call_type_arguments: self.calls.body_ir_call_type_arguments.remove(&key),
            body_ir_call_result_type: self.calls.body_ir_call_result_types.remove(&key),
            body_ir_factory_owner_type: self.calls.body_ir_factory_owner_types.remove(&key),
            body_ir_async_call_target: self.calls.body_ir_async_call_targets.remove(&key),
            body_ir_self_result_method: self.calls.body_ir_self_result_methods.remove(&key),
            body_ir_self_method_identity: self.traits.self_method_identities.remove(&key),
            body_ir_self_method_result_type: self.traits.self_method_result_types.remove(&key),
            body_ir_self_method_params: self.traits.self_method_params.remove(&key),
            body_ir_inferred_method_identity: self.traits.inferred_method_identities.remove(&key),
        }
    }

    /// Clear only direct-route hook facts between two synthetic resolutions, leaving legacy's probe state unchanged.
    pub(crate) fn clear_body_ir_call_site_facts(&mut self, span: Span) {
        let key = span;
        self.protocols.explicit_input_calls.remove(&key);
        self.rust.checked_calls.remove(&key);
        self.rust.checked_function_values.remove(&key);
        self.calls.body_ir_reexport_calls.remove(&key);
        self.calls.body_ir_string_slice_members.remove(&key);
        self.calls.body_ir_constructor_types.remove(&key);
        self.calls.body_ir_call_type_arguments.remove(&key);
        self.calls.body_ir_call_result_types.remove(&key);
        self.calls.body_ir_factory_owner_types.remove(&key);
        self.calls.body_ir_async_call_targets.remove(&key);
        self.calls.body_ir_self_result_methods.remove(&key);
        self.traits.self_method_identities.remove(&key);
        self.traits.self_method_result_types.remove(&key);
        self.traits.self_method_params.remove(&key);
        self.traits.inferred_method_identities.remove(&key);
    }

    /// Make `facts`, as [`Self::take_call_site_facts`] returned them, the facts of the call at `span`, dropping any a
    /// later resolution recorded there.
    pub(crate) fn restore_call_site_facts(&mut self, span: Span, facts: CallSiteFacts) {
        /// Record `value` under `key`, or remove the entry when there is none.
        fn put<T>(map: &mut HashMap<crate::ast::Span, T>, key: crate::ast::Span, value: Option<T>) {
            match value {
                Some(value) => {
                    map.insert(key, value);
                }
                None => {
                    map.remove(&key);
                }
            }
        }
        let key = span;
        self.protocols.explicit_input_calls.remove(&key);
        if let Some(call) = facts.explicit_input_call {
            self.protocols.explicit_input_calls.insert(key, call);
        }
        put(&mut self.rust.checked_calls, key, facts.checked_rust_call);
        put(
            &mut self.rust.checked_function_values,
            key,
            facts.checked_rust_function_value,
        );
        put(&mut self.calls.body_ir_reexport_calls, key, facts.body_ir_reexport_call);
        put(
            &mut self.calls.body_ir_string_slice_members,
            key,
            facts.body_ir_string_slice_member,
        );
        put(
            &mut self.calls.body_ir_self_result_methods,
            key,
            facts.body_ir_self_result_method,
        );
        put(
            &mut self.calls.inferred_constructor_types,
            key,
            facts.inferred_constructor_type,
        );
        put(
            &mut self.calls.inferred_call_result_types,
            key,
            facts.inferred_call_result_type,
        );
        put(&mut self.calls.resolved_method_calls, key, facts.method_call);
        put(&mut self.references.resolved_identities, key, facts.identity);
        put(&mut self.calls.call_site_callable_params, key, facts.callable_params);
        put(
            &mut self.calls.call_site_monomorph_type_args,
            key,
            facts.monomorph_type_args,
        );
        put(
            &mut self.calls.body_ir_constructor_types,
            key,
            facts.body_ir_constructor_type,
        );
        put(
            &mut self.calls.body_ir_call_type_arguments,
            key,
            facts.body_ir_call_type_arguments,
        );
        put(
            &mut self.calls.body_ir_call_result_types,
            key,
            facts.body_ir_call_result_type,
        );
        put(
            &mut self.calls.body_ir_factory_owner_types,
            key,
            facts.body_ir_factory_owner_type,
        );
        put(
            &mut self.calls.body_ir_async_call_targets,
            key,
            facts.body_ir_async_call_target,
        );
        put(
            &mut self.traits.self_method_identities,
            key,
            facts.body_ir_self_method_identity,
        );
        put(
            &mut self.traits.self_method_result_types,
            key,
            facts.body_ir_self_method_result_type,
        );
        put(
            &mut self.traits.self_method_params,
            key,
            facts.body_ir_self_method_params,
        );
        put(
            &mut self.traits.inferred_method_identities,
            key,
            facts.body_ir_inferred_method_identity,
        );
    }

    /// Preserve a resolved trait's defining module across an adapter chain that returns the same trait.
    ///
    /// Source types record the exact trait they adopted, but a method return such as `Stream[U, E]` intentionally keeps
    /// only its source spelling in [`ResolvedType`]. When the next call resolves that same trait, inherit the prior
    /// call's canonical module instead of degrading backend dispatch to an unqualified Rust trait method.
    pub fn inherit_same_trait_method_module(&mut self, receiver_span: Span, call_span: Span) {
        let Some(ResolvedMethodCall {
            dispatch:
                ResolvedMethodDispatch::Trait {
                    trait_name: receiver_trait,
                    module_path: Some(receiver_module),
                    ..
                },
            ..
        }) = self.resolved_method_call(receiver_span).cloned()
        else {
            return;
        };
        let Some(ResolvedMethodCall {
            dispatch:
                ResolvedMethodDispatch::Trait {
                    trait_name,
                    module_path,
                    ..
                },
            ..
        }) = self.calls.resolved_method_calls.get_mut(&call_span)
        else {
            return;
        };
        if module_path.is_none() && *trait_name == receiver_trait {
            *module_path = Some(receiver_module);
        }
    }

    /// Record a custom `for` iteration protocol route.
    pub fn record_protocol_iteration(&mut self, span: Span, info: ProtocolIterationInfo) {
        self.protocols.iterations.insert(span, info);
    }

    /// Record that the `for` loop over the list at `iter_span` takes the list's items (#1844).
    pub fn record_for_loop_takes_items(&mut self, iter_span: Span) {
        self.protocols.item_taking_iterations.insert(iter_span);
    }

    /// Return whether the `for` loop over the list at `iter_span` takes the list's items (#1844).
    pub fn for_loop_takes_items(&self, iter_span: Span) -> bool {
        self.protocols.item_taking_iterations.contains(&iter_span)
    }

    /// Record whether a checked comprehension consumes its direct collection source (#1983).
    pub fn record_comprehension_source_consumption(&mut self, iter_span: Span, consumed: bool) {
        self.protocols
            .comprehension_source_consumption
            .insert(iter_span, consumed);
    }

    /// Return the checker's collection-source consumption decision for a comprehension, when one applies (#1983).
    pub fn comprehension_source_is_consumed(&self, iter_span: Span) -> Option<bool> {
        self.protocols.comprehension_source_consumption.get(&iter_span).copied()
    }

    /// Record that an arm of the `match`, `if let` or `while let` over the scrutinee at `span` changes a caller-visible
    /// `mut` parameter through a name its pattern binds (#1561).
    pub fn record_match_scrutinee_changed_in_place(&mut self, span: Span) {
        self.protocols.in_place_match_scrutinees.insert(span);
    }

    /// Return whether the scrutinee at `span` is matched in place: an arm changes a caller-visible `mut` parameter
    /// through a name the pattern binds, so each name binds the part of the parameter it names (#1561).
    pub fn match_scrutinee_is_changed_in_place(&self, span: Span) -> bool {
        self.protocols.in_place_match_scrutinees.contains(&span)
    }

    /// Record that the value of the `match`, `if let` or `while let` scrutinee at `span` holds no loan.
    pub fn record_loan_free_scrutinee(&mut self, span: Span) {
        self.protocols.loan_free_scrutinees.insert(span);
    }

    /// Return whether the scrutinee at `span` holds no loan, so the temporaries its evaluation made can end before
    /// the arms run.
    pub fn scrutinee_holds_no_loan(&self, span: Span) -> bool {
        self.protocols.loan_free_scrutinees.contains(&span)
    }
}

/// Authenticate either canonical origin shape used by the standard Iterator declaration.
fn standard_iterator_module_path(module_path: Option<&[String]>) -> bool {
    let Some(module_path) = module_path else {
        return false;
    };
    let Some(source_module) = incan_lang::lang::traits::source_module(incan_lang::lang::traits::TraitId::Iterator)
    else {
        return false;
    };
    let canonical = source_module.split('.').map(str::to_owned).collect::<Vec<_>>();
    module_path == canonical || (canonical.first().is_some_and(|root| root == "std") && module_path == &canonical[1..])
}
