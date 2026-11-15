//! Property and method declaration preparation and checking.

use super::*;

impl TypeChecker {
    /// Select the collected owner's exact generic surface for a method's `Self`. Each owner parameter is the body's
    /// own fixed type, preserving the existing field and constructor checking rule (#1561).
    pub(super) fn concrete_method_self_type(&self, owner: &str) -> ResolvedType {
        let parameters = self
            .lookup_type_info(owner)
            .map(|info| match info {
                TypeInfo::Model(model) => model.type_params.as_slice(),
                TypeInfo::Class(class) => class.type_params.as_slice(),
                TypeInfo::Newtype(newtype) => newtype.type_params.as_slice(),
                TypeInfo::Enum(enum_info) => enum_info.type_params.as_slice(),
                TypeInfo::Builtin | TypeInfo::TypeAlias => &[],
            })
            .unwrap_or(&[]);
        if parameters.is_empty() {
            ResolvedType::Named(owner.to_string())
        } else {
            ResolvedType::Generic(
                owner.to_string(),
                parameters
                    .iter()
                    .map(|parameter| ResolvedType::Named(parameter.clone()))
                    .collect(),
            )
        }
    }

    /// Validate a model or class computed property body using the concrete nominal owner as immutable `self`.
    pub fn check_property(&mut self, property: &Spanned<PropertyDecl>, owner: &str, owner_params: &[TypeParam]) {
        let owner_type_params = self
            .lookup_type_info(owner)
            .map(|info| match info {
                TypeInfo::Model(model) => model.type_params.clone(),
                TypeInfo::Class(class) => class.type_params.clone(),
                TypeInfo::Builtin | TypeInfo::TypeAlias | TypeInfo::Newtype(_) | TypeInfo::Enum(_) => Vec::new(),
            })
            .unwrap_or_default();
        let previous_owner = self.current_method_owner.replace(owner.to_string());
        let owner_self_ty = if owner_type_params.is_empty() {
            ResolvedType::Named(owner.to_string())
        } else {
            ResolvedType::Generic(
                owner.to_string(),
                owner_type_params
                    .iter()
                    .map(|type_param| ResolvedType::TypeVar(type_param.clone()))
                    .collect(),
            )
        };
        let prepared = self.check_declaration_owner(property.span, |checker| {
            checker.prepare_property_declaration(&property.node, property.span, owner_self_ty, owner_params)
        });
        self.symbols.resume_scope(prepared.scope);
        self.current_return_error_type = prepared.return_error_type;
        if let Some(body) = &property.node.body {
            self.check_property_body(body, property.span);
        }
        self.current_return_error_type = None;
        self.symbols.exit_scope();
        #[cfg(test)]
        if property.node.body.is_some() {
            self.observe_body_local(property.span);
        }
        self.current_method_owner = previous_owner;
    }

    /// Validate a computed property receiver and return annotation without entering its statement producer.
    pub(super) fn prepare_property_declaration(
        &mut self,
        property: &PropertyDecl,
        property_span: Span,
        self_ty: ResolvedType,
        owner_params: &[TypeParam],
    ) -> PreparedPropertyCheck {
        self.check_work.property_declarations += 1;

        // ---- Receiver scope ----
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: Some(Receiver::Immutable),
        });

        for type_param in owner_params {
            self.symbols.define_refined_binding(Symbol {
                name: type_param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin), // Type-var placeholder
                span: type_param.span,
                scope: 0,
            });
        }

        self.symbols.define_with_target_kind(
            Symbol {
                name: "self".to_string(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: self_ty.clone(),
                    is_mutable: false,
                    is_used: true,
                }),
                // Properties have an implicit `self`, so the property declaration is its source provenance.
                span: property_span,
                scope: 0,
            },
            SemanticSourceTargetKind::Receiver,
        );
        // As for a method's receiver, Body IR binds the getter's `self` local to this identity, so the body's reads of
        // `self` resolve to it.
        self.record_write_target_identity(property_span, "self");

        // ---- Checked receiver identity and return ----
        let receiver_identity = self
            .symbols
            .lookup("self")
            .and_then(|id| self.symbols.identity_of(id))
            .cloned();
        if let Some(binding) = self
            .type_info
            .declarations
            .body_ir_property_bindings
            .get_mut(&property_span)
        {
            binding.receiver_identity = receiver_identity;
        }
        let return_type = self.resolve_type_checked(&property.return_type);
        let effective_return_type = Self::concretize_self_type_in_annotation(&return_type, &self_ty);
        self.symbols.set_return_type(effective_return_type);
        self.current_return_error_type = return_type.result_err_type().cloned();

        PreparedPropertyCheck {
            scope: self.symbols.suspend_scope(),
            return_error_type: self.current_return_error_type.take(),
        }
    }

    /// Validate a method declaration and prepare its concrete `Self` frame without checking its statements.
    ///
    /// Generic owners pass `Owner[T, ...]` here so return checking and `cls(...)` constructor calls see the same
    /// generic surface that call sites use. Trait default methods may still pass bare `Self` because their eventual
    /// adopter is resolved later during trait conformance and method-call substitution.
    ///
    /// `method_span` is the declaration span of `method` itself (the `Spanned<MethodDecl>` this `&MethodDecl` was
    /// unwrapped from). It keys [`DeclarationArtifacts::method_bindings_by_span`](
    /// super::type_info::DeclarationArtifacts::method_bindings_by_span), recorded below once ordinary parameters are
    /// resolved, so Body IR lowering (#1121) can retrieve the checked callable signature for this exact declaration
    /// instead of re-resolving raw AST annotations. This revision-local span distinguishes same-name methods on
    /// different owners and same-owner overloads; persistent identity is supplied separately by the record adapter.
    pub(in crate::typechecker) fn prepare_method_declaration(
        &mut self,
        method: &MethodDecl,
        method_span: Span,
        self_ty: ResolvedType,
        owner_params: &[TypeParam],
    ) -> PreparedMethodCheck {
        self.check_work.method_declarations += 1;

        // ---- Declaration and receiver inputs ----
        self.validate_protected_type_param_bindings(&method.type_params, method_span);
        self.refuse_receiver_on_static_method(method, method_span);
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: method.receiver,
        });
        self.validate_callable_rest_params(&method.params);
        self.validate_surface_modifier_typecheck_actions(&method.surface_modifiers, &method.return_type);

        self.define_method_type_parameters(method, owner_params);
        let mut active_bounds = self.type_param_bound_details_from_type_params(owner_params);
        active_bounds.extend(self.type_param_bound_details_from_type_params(&method.type_params));
        self.current_type_param_bound_details.push(active_bounds);
        let previous_annotation_owner = self.enter_annotation_owner(&method.name, &method.type_params);
        let resolved_param_types = self.resolve_callable_parameter_types_and_check_defaults(
            &method.params,
            super::ownership_contracts::method_input_header(method),
        );
        let resolved_param_types =
            self.inherited_method_parameter_types(method, method_span, resolved_param_types, owner_params, &self_ty);
        let return_type = self.resolve_callable_return_annotation(&method.return_type, method.result_source.is_some());
        self.record_method_ownership(method, method_span, &resolved_param_types, &return_type, &self_ty);

        // Define the source receiver with its exact declaration token. Classmethod `cls` remains absent from the
        // legacy receiver enum because it emits as an associated function, but it is still a source binding with a
        // canonical receiver identity.
        let binds_self_receiver = method
            .receiver_binding
            .as_ref()
            .is_none_or(|binding| binding.node == "self");
        let previous_immutable_self_method = std::mem::replace(
            &mut self.current_immutable_self_method,
            (method.receiver == Some(Receiver::Immutable) && binds_self_receiver).then(|| method.name.clone()),
        );
        // The method's `mut` bindings, its receiver included, are its own: the set starts empty and the enclosing set
        // is restored afterwards.
        let previous_mutable_bindings = std::mem::take(&mut self.mutable_bindings);
        let is_classmethod = self.define_method_receiver_bindings(method, method_span, &self_ty, binds_self_receiver);
        let previous_classmethod_self_ty = self.current_classmethod_self_ty.take();
        if is_classmethod {
            self.current_classmethod_self_ty = Some(self_ty.clone());
        }

        // ---- Parameter bindings and return contract ----
        // Define parameters, capturing each checked callable-parameter fact once so it can be recorded for Body IR
        // lowering below without re-resolving the annotation (and re-emitting any of its diagnostics) a second time.
        let mut checked_params = Vec::with_capacity(method.params.len());
        let mut_param_body = self.enter_mut_param_body(
            &method.name,
            SemanticSourceTargetKind::Method,
            method_span,
            &method.params,
            &resolved_param_types,
        );
        for (slot, (param, resolved_ty)) in method.params.iter().zip(resolved_param_types).enumerate() {
            checked_params.push(CallableParam::named_with_default(
                param.node.name.clone(),
                resolved_ty.clone(),
                param.node.kind,
                param.node.default.is_some(),
            ));
            self.bind_checked_callable_parameter(param, resolved_ty, slot);
        }

        self.type_info.declarations.method_bindings_by_span.insert(
            method_span,
            FunctionBindingInfo {
                is_async: method.is_async(),
                params: checked_params,
                return_type: return_type.clone(),
                identity: Some(self.symbols.member_declaration_identity(
                    &method.name,
                    SemanticSourceTargetKind::Method,
                    method_span,
                )),
            },
        );
        let method_identity =
            self.symbols
                .member_declaration_identity(&method.name, SemanticSourceTargetKind::Method, method_span);
        let body_return_type = self.returned_value_type(&method_identity, &return_type);
        let yield_context = self.set_method_return_context(method, &body_return_type, &self_ty);

        // Set error type for ? checking: the value type, so a view carried in the error compares as its value.
        self.current_return_error_type = body_return_type.result_err_type().cloned();

        // ---- Prepared inputs ----
        PreparedMethodCheck {
            callable: self.suspend_prepared_callable(
                previous_annotation_owner,
                previous_mutable_bindings,
                mut_param_body,
                yield_context,
                method.body.is_some(),
            ),
            immutable_self_method: std::mem::replace(
                &mut self.current_immutable_self_method,
                previous_immutable_self_method,
            ),
            classmethod_self_ty: std::mem::replace(&mut self.current_classmethod_self_ty, previous_classmethod_self_ty),
        }
    }

    /// Consume a method's validated declaration frame without nesting statement checking inside validation.
    /// Abstract methods restore the same prepared context but never enter a statement producer.
    pub(in crate::typechecker) fn check_prepared_method(
        &mut self,
        method: &MethodDecl,
        span: Span,
        prepared: PreparedMethodCheck,
    ) {
        let previous_immutable_self =
            std::mem::replace(&mut self.current_immutable_self_method, prepared.immutable_self_method);
        let previous_classmethod =
            std::mem::replace(&mut self.current_classmethod_self_ty, prepared.classmethod_self_ty);
        let (restore, yield_context) = self.enter_prepared_callable(prepared.callable);
        if let Some(body) = &method.body {
            self.check_work.method_bodies += 1;
            let _ = self.check_callable_body(
                body,
                method.is_async(),
                span,
                yield_context,
                super::ownership_contracts::CallableSource::Method(method),
            );
        }
        self.current_classmethod_self_ty = previous_classmethod;
        self.current_immutable_self_method = previous_immutable_self;
        self.finish_prepared_callable(restore);
    }

    /// Refine owner binders before declaring the method's own generic binders and resolving an explicit trait
    /// target. Bound-stack and annotation-owner setup remains with the enclosing method frame.
    pub(super) fn define_method_type_parameters(&mut self, method: &MethodDecl, owner_params: &[TypeParam]) {
        for type_param in owner_params {
            self.symbols.define_refined_binding(Symbol {
                name: type_param.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: type_param.span,
                scope: 0,
            });
        }
        for type_param in &method.type_params {
            self.symbols.define_with_target_kind(
                Symbol {
                    name: type_param.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Builtin),
                    span: type_param.span,
                    scope: 0,
                },
                SemanticSourceTargetKind::GenericBinder,
            );
        }
        self.refuse_unsupported_callable_markers(&method.type_params, CallableMarkerOwner::Callable);
        if let Some(target) = &method.trait_target {
            let trait_name = target.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let _ = self.resolve_adopted_trait_info(&target.node, target.span);
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, target.span));
            }
        }
    }

    /// Bind the exact source receiver token after defaults have checked. A self receiver retains its mutability;
    /// a classmethod's cls token retains the owner's type token. Return whether classmethod context is required.
    pub(super) fn define_method_receiver_bindings(
        &mut self,
        method: &MethodDecl,
        method_span: Span,
        self_ty: &ResolvedType,
        binds_self_receiver: bool,
    ) -> bool {
        if let Some(receiver) = method.receiver
            && binds_self_receiver
        {
            let is_mutable = matches!(receiver, Receiver::Mutable);
            if is_mutable {
                self.mutable_bindings.insert("self".to_string());
            }
            self.symbols.define_with_target_kind(
                Symbol {
                    name: "self".to_string(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: self_ty.clone(),
                        is_mutable,
                        is_used: true,
                    }),
                    span: method
                        .receiver_binding
                        .as_ref()
                        .map_or(method_span, |binding| binding.span),
                    scope: 0,
                },
                SemanticSourceTargetKind::Receiver,
            );
            self.record_write_target_identity(
                method
                    .receiver_binding
                    .as_ref()
                    .map_or(method_span, |binding| binding.span),
                "self",
            );
        }
        let is_classmethod = Self::method_has_decorator(method, DecoratorId::ClassMethod);
        if is_classmethod
            && let Some(receiver_binding) = &method.receiver_binding
            && receiver_binding.node == "cls"
        {
            self.symbols.define_with_target_kind(
                Symbol {
                    name: "cls".to_string(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ResolvedType::TypeToken(Box::new(self_ty.clone())),
                        is_mutable: false,
                        is_used: true,
                    }),
                    span: receiver_binding.span,
                    scope: 0,
                },
                SemanticSourceTargetKind::Receiver,
            );
            self.record_write_target_identity(receiver_binding.span, "cls");
        }
        is_classmethod
    }

    /// Set the checked method return after substituting its receiver for Self and determine the yield context.
    /// Generator contracts require a reachable yield unless the body already returns a value; abstract methods
    /// retain their existing no-body behavior.
    pub(super) fn set_method_return_context(
        &mut self,
        method: &MethodDecl,
        return_type: &ResolvedType,
        self_ty: &ResolvedType,
    ) -> YieldContext {
        let effective_return_type = Self::concretize_self_type_in_annotation(return_type, self_ty);
        let body_has_yield = method
            .body
            .as_ref()
            .is_some_and(|body| any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_))));
        let body_has_reachable_yield = body_has_yield
            && method
                .body
                .as_deref()
                .is_some_and(super::reachability::body_has_reachable_yield);
        if effective_return_type.generator_element_type().is_some()
            && !body_has_reachable_yield
            && method.body.as_deref().is_some_and(|body| !body_has_return_value(body))
        {
            self.errors
                .push(errors::generator_requires_yield(&method.name, method.return_type.span));
        }
        let yield_context = if body_has_yield {
            match effective_return_type.generator_element_type() {
                Some(element_ty) => YieldContext::Generator {
                    element_ty: element_ty.clone(),
                },
                None => YieldContext::Disallowed,
            }
        } else {
            YieldContext::Disallowed
        };
        self.symbols.set_return_type(effective_return_type);
        yield_context
    }
}
