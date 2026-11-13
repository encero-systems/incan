//! Testing fixtures, callable parameter and return rules, function preparation, and callable check frames.

use super::*;

impl TypeChecker {
    /// Collect fixture names before function bodies are checked so dependency metadata is independent of declaration
    /// order.
    pub fn collect_testing_fixture_names(&mut self, program: &Program) {
        self.testing_fixture_names.clear();
        let Some(span) = declarations_may_contain_testing_fixture(&program.declarations, &self.symbols) else {
            return;
        };
        let Some(semantics) = self.testing_marker_semantics.clone() else {
            self.errors.push(errors::invalid_std_testing_marker_metadata(
                "the active std.testing provider did not supply checked marker metadata",
                span,
            ));
            return;
        };
        self.collect_testing_fixture_names_from_decls(&program.declarations, &semantics);
    }

    /// Recursively collect fixture names from top-level and inline test-module declarations.
    pub(super) fn collect_testing_fixture_names_from_decls(
        &mut self,
        declarations: &[Spanned<Declaration>],
        semantics: &TestingMarkerSemantics,
    ) {
        for decl in declarations {
            match &decl.node {
                Declaration::Function(func)
                    if resolve_testing_fixture_marker_args(&func.decorators, &self.symbols, semantics).is_some() =>
                {
                    self.testing_fixture_names.insert(func.name.clone());
                }
                Declaration::TestModule(test_module) => {
                    self.collect_testing_fixture_names_from_decls(&test_module.body, semantics);
                }
                _ => {}
            }
        }
    }

    /// Resolve fixture marker arguments for a declaration, reporting stdlib metadata failures through type diagnostics.
    pub(super) fn testing_fixture_marker_args(
        &mut self,
        decorators: &[Spanned<Decorator>],
        span: Span,
    ) -> Option<TestingFixtureMarkerArgs> {
        if !decorators
            .iter()
            .any(|decorator| is_possible_testing_fixture_decorator(&decorator.node, &self.symbols))
        {
            return None;
        }

        let Some(semantics) = self.testing_marker_semantics.as_ref() else {
            self.errors.push(errors::invalid_std_testing_marker_metadata(
                "the active std.testing provider did not supply checked marker metadata",
                span,
            ));
            return None;
        };
        resolve_testing_fixture_marker_args(decorators, &self.symbols, semantics)
    }

    /// Enforce source-language ordering and default rules for `*args` / `**kwargs` declarations.
    pub(super) fn validate_callable_rest_params(&mut self, params: &[Spanned<Param>]) {
        let mut saw_rest_positional = false;
        let mut saw_rest_keyword = false;
        let mut saw_rest = false;

        for param in params {
            match param.node.kind {
                ParamKind::Normal => {
                    if saw_rest_keyword {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "Normal parameters cannot appear after a `**kwargs` rest parameter",
                            param.span,
                        ));
                    } else if saw_rest {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "Normal parameters cannot appear after a rest parameter",
                            param.span,
                        ));
                    }
                }
                ParamKind::RestPositional => {
                    if saw_rest_positional {
                        self.errors.push(errors::duplicate_rest_parameter("*args", param.span));
                    }
                    if saw_rest_keyword {
                        self.errors.push(errors::invalid_rest_parameter_order(
                            "`*args` must appear before `**kwargs`",
                            param.span,
                        ));
                    }
                    if param.node.default.is_some() {
                        self.errors
                            .push(errors::rest_parameter_default_not_allowed(&param.node.name, param.span));
                    }
                    saw_rest_positional = true;
                    saw_rest = true;
                }
                ParamKind::RestKeyword => {
                    if saw_rest_keyword {
                        self.errors
                            .push(errors::duplicate_rest_parameter("**kwargs", param.span));
                    }
                    if param.node.default.is_some() {
                        self.errors
                            .push(errors::rest_parameter_default_not_allowed(&param.node.name, param.span));
                    }
                    saw_rest_keyword = true;
                    saw_rest = true;
                }
            }
        }
    }

    /// Run declaration-level typecheck actions selected by the surface semantics registry.
    pub(super) fn validate_surface_modifier_typecheck_actions(
        &mut self,
        modifiers: &[SurfaceModifier],
        return_type: &Spanned<Type>,
    ) {
        use crate::semantics_registry::semantics_registry;

        for modifier in modifiers {
            let Some(action) = semantics_registry().typecheck_surface_modifier_action(&modifier.key) else {
                continue;
            };
            match action {
                SurfaceModifierTypeCheck::AsyncCallable => self.validate_async_callable_return_type(return_type),
            }
        }
    }

    /// Validate return annotations for an async callable.
    ///
    /// Incan async declarations spell the callable's logical output type, not an explicit `Future[T]` wrapper, so no
    /// additional return-type rejection is currently applicable. Keeping this routed through the semantics registry
    /// prevents `async` from becoming another hardcoded declaration special case.
    pub(super) fn validate_async_callable_return_type(&mut self, return_type: &Spanned<Type>) {
        let _ = return_type;
    }

    /// Return whether a method carries a resolved builtin decorator.
    pub(super) fn method_has_decorator(method: &MethodDecl, id: DecoratorId) -> bool {
        method
            .decorators
            .iter()
            .any(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(id))
    }

    /// Refuse a `self` or `mut self` receiver on a `@staticmethod` (#1882).
    ///
    /// A static method is called on the type, `TypeName.method(...)`, so there is no instance to receive; the parser
    /// reads a leading `self` as the receiver whatever the decorator, and a method that keeps it is an instance method
    /// no call on the type reaches.
    pub(super) fn refuse_receiver_on_static_method(&mut self, method: &MethodDecl, method_span: Span) {
        let Some(receiver) = method.receiver else {
            return;
        };
        let decorator = DecoratorId::StaticMethod;
        if !Self::method_has_decorator(method, decorator) {
            return;
        }
        let receiver_text = match receiver {
            Receiver::Mutable => "mut self",
            Receiver::Immutable => "self",
        };
        let span = method
            .receiver_binding
            .as_ref()
            .map_or(method_span, |binding| binding.span);
        self.errors.push(errors::receiver_on_static_method(
            &method.name,
            decorators::as_str(decorator),
            receiver_text,
            span,
        ));
    }

    /// Produce function validation, defaults and prepared lexical inputs without checking any statement.
    pub(in crate::typechecker) fn prepare_function_declaration(
        &mut self,
        func: &FunctionDecl,
        decl_span: Span,
    ) -> PreparedFunctionCheck {
        self.check_work.function_declarations += 1;
        self.symbols.enter_scope(ScopeKind::Function);

        // ---- Declaration metadata and generic bounds ----
        self.validate_decorators_allowing_user_defined(&func.decorators);
        self.record_native_attribute_applications(&func.decorators, decl_span);
        self.check_registry_description_decorators(
            &func.decorators,
            &func.name,
            decl_span,
            SemanticRegistrySubjectKind::Function,
        );
        self.check_provider_operation_decorators(func, decl_span);
        self.validate_callable_rest_params(&func.params);
        let fixture_args = self.check_function_fixture_marker(func);
        self.validate_surface_modifier_typecheck_actions(&func.surface_modifiers, &func.return_type);

        let previous_annotation_owner = self.prepare_function_generic_context(func);

        // ---- Signature defaults and frame bindings ----
        let resolved_param_types = self.resolve_callable_parameter_types_and_check_defaults(
            &func.params,
            super::ownership_contracts::direct_function_boundary(func),
        );
        let return_type = self.resolve_callable_return_annotation(&func.return_type, func.result_source.is_some());
        self.record_function_ownership(func, decl_span, &resolved_param_types, &return_type);
        self.check_route_handler_signature(func, &return_type, &resolved_param_types);

        // Define parameters after checking defaults so a declaration-owned default cannot resolve a callable-frame
        // binding. The function body still receives its ordinary parameter locals below. Its `mut` bindings are its
        // own: the set starts empty and the enclosing set is restored afterwards.
        let previous_mutable_bindings = std::mem::take(&mut self.mutable_bindings);
        let mut_param_body = self.enter_mut_param_body(
            &func.name,
            SemanticSourceTargetKind::Function,
            decl_span,
            &func.params,
            &resolved_param_types,
        );
        for (slot, (param, resolved_ty)) in func.params.iter().zip(resolved_param_types).enumerate() {
            self.bind_checked_callable_parameter(param, resolved_ty, slot);
        }

        // ---- Yield and native-call contexts ----
        let has_yield = any_expr_in_body(&func.body, |expr| matches!(expr, Expr::Yield(_)));
        // RFC 006 requires a reachable `yield`: one after an unconditional `return` in its block never runs.
        let has_reachable_yield = has_yield && super::reachability::body_has_reachable_yield(&func.body);
        if return_type.generator_element_type().is_some() && !has_reachable_yield && !body_has_return_value(&func.body)
        {
            self.errors
                .push(errors::generator_requires_yield(&func.name, func.return_type.span));
        }
        let yield_context = if fixture_args.is_some() {
            YieldContext::Fixture
        } else if has_yield {
            match return_type.generator_element_type() {
                Some(element_ty) => YieldContext::Generator {
                    element_ty: element_ty.clone(),
                },
                None => YieldContext::Disallowed,
            }
        } else {
            YieldContext::Disallowed
        };
        let function_identity =
            self.symbols
                .module_declaration_identity(&func.name, SemanticSourceTargetKind::Function, decl_span);
        let body_return_type = self.returned_value_type(&function_identity, &return_type);
        self.symbols.set_return_type(body_return_type.clone());

        // Set error type for ? checking: the value type, so a view carried in the error compares as its value.
        self.current_return_error_type = body_return_type.result_err_type().cloned();

        let previous_c_abi_raw_call_owner =
            self.current_c_abi_raw_call_owner
                .replace(crate::typechecker::CBindingRawCallOwner {
                    name: func.name.clone(),
                    visibility: func.visibility,
                    declaration_span: decl_span,
                });
        let previous_function = self.receiver_plan_inputs.current_function.replace(decl_span);
        PreparedFunctionCheck {
            callable: self.suspend_prepared_callable(
                previous_annotation_owner,
                previous_mutable_bindings,
                mut_param_body,
                yield_context,
                true,
            ),
            function: std::mem::replace(&mut self.receiver_plan_inputs.current_function, previous_function),
            c_abi_raw_call_owner: std::mem::replace(
                &mut self.current_c_abi_raw_call_owner,
                previous_c_abi_raw_call_owner,
            ),
        }
    }

    /// Reconstruct only the lexical frame consumed by a changed function body from its already collected and
    /// producer-validated declaration value. This performs no annotation, default, decorator or ownership checking.
    pub(in crate::typechecker) fn prepare_reused_function_body(
        &mut self,
        func: &FunctionDecl,
        decl_span: Span,
        binding: &FunctionBindingInfo,
    ) -> PreparedFunctionCheck {
        self.symbols.enter_scope(ScopeKind::Function);
        let previous_annotation_owner = self.enter_annotation_owner(&func.name, &[]);
        self.current_type_param_bound_details.push(HashMap::new());

        // ---- Parameter frame: bind the accepted resolved types without revisiting their annotations or defaults ----
        let resolved_param_types = binding
            .params
            .iter()
            .map(|parameter| parameter.ty.clone())
            .collect::<Vec<_>>();
        let previous_mutable_bindings = std::mem::take(&mut self.mutable_bindings);
        let mut_param_body = self.enter_mut_param_body(
            &func.name,
            SemanticSourceTargetKind::Function,
            decl_span,
            &func.params,
            &resolved_param_types,
        );
        for (slot, (param, resolved_ty)) in func.params.iter().zip(resolved_param_types).enumerate() {
            self.bind_checked_callable_parameter(param, resolved_ty, slot);
        }

        // ---- Body context: recover the accepted return and native-call owners without declaration validation ----
        let body_return_type = binding.identity.as_ref().map_or_else(
            || binding.return_type.clone(),
            |identity| self.returned_value_type(identity, &binding.return_type),
        );
        self.symbols.set_return_type(body_return_type.clone());
        self.current_return_error_type = body_return_type.result_err_type().cloned();
        let previous_c_abi_raw_call_owner =
            self.current_c_abi_raw_call_owner
                .replace(crate::typechecker::CBindingRawCallOwner {
                    name: func.name.clone(),
                    visibility: func.visibility,
                    declaration_span: decl_span,
                });
        let previous_function = self.receiver_plan_inputs.current_function.replace(decl_span);
        PreparedFunctionCheck {
            callable: self.suspend_prepared_callable(
                previous_annotation_owner,
                previous_mutable_bindings,
                mut_param_body,
                YieldContext::Disallowed,
                true,
            ),
            function: std::mem::replace(&mut self.receiver_plan_inputs.current_function, previous_function),
            c_abi_raw_call_owner: std::mem::replace(
                &mut self.current_c_abi_raw_call_owner,
                previous_c_abi_raw_call_owner,
            ),
        }
    }

    /// Bind a function's declared generic parameters and bounds before checking its defaults or frame.
    pub(super) fn prepare_function_generic_context(&mut self, func: &FunctionDecl) -> Option<super::AnnotationOwner> {
        // Define type parameters so explicit generic bounds are visible in function-level type resolution.
        for param in &func.type_params {
            self.symbols.define_with_target_kind(
                Symbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Builtin), // Type-var placeholder
                    span: param.span,
                    scope: 0,
                },
                SemanticSourceTargetKind::GenericBinder,
            );
        }
        self.refuse_unsupported_callable_markers(&func.type_params, CallableMarkerOwner::Callable);
        self.validate_type_param_bound_type_names(&func.type_params);
        let active_bounds = self.type_param_bound_details_from_type_params(&func.type_params);
        self.current_type_param_bound_details.push(active_bounds);
        self.enter_annotation_owner(&func.name, &func.type_params)
    }

    /// Consume a completed function declaration's lexical frame exactly once and trace its stable owner until
    /// checker-record publication classifies it. Decoration is separate post-body work; no declaration validation or
    /// default expression is repeated by this consumer.
    pub(in crate::typechecker) fn check_prepared_function(
        &mut self,
        func: &FunctionDecl,
        span: Span,
        prepared: PreparedFunctionCheck,
        record_program: Option<&Program>,
    ) {
        let body_trace = self.owner_body_trace(span);
        let _trace = body_trace.enter();
        let previous_function = std::mem::replace(&mut self.receiver_plan_inputs.current_function, prepared.function);
        let previous_raw_owner =
            std::mem::replace(&mut self.current_c_abi_raw_call_owner, prepared.c_abi_raw_call_owner);
        let (restore, yield_context) = self.enter_prepared_callable(prepared.callable);
        let reused = record_program.is_some_and(|program| match self.reuse_graph_body_record(program, span) {
            Ok(reused) => reused,
            Err(error) => {
                tracing::debug!(target: "incan::invocation", ?span, ?error, "graph body selection refused");
                false
            }
        });
        let local_index = if reused {
            None
        } else {
            self.check_work.function_bodies += 1;
            self.check_callable_body(
                &func.body,
                func.is_async(),
                span,
                yield_context,
                super::ownership_contracts::CallableSource::Function(func),
            )
        };
        self.receiver_plan_inputs.current_function = previous_function;
        self.current_c_abi_raw_call_owner = previous_raw_owner;
        self.finish_prepared_callable(restore);
        if let (Some(program), Some(index)) = (record_program, local_index) {
            self.produce_local_record(program, span, index);
        }
    }

    /// Enter one physical callable owner in checking scope whether its statements are restored or evaluated freshly.
    pub(in crate::typechecker) fn owner_body_trace(&mut self, span: Span) -> tracing::Span {
        let trace_identity = self
            .declaration_view()
            .function(span)
            .and_then(|binding| binding.identity.as_ref())
            .and_then(|canonical| self.type_info.declarations.source_identities.get(canonical))
            .map(incan_semantics_core::stable_identity::StableDeclarationId::render_compact)
            .unwrap_or_else(|| "<unavailable>".into());
        let body_trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::check_owner_body",
            identity = %trace_identity,
            outcome = tracing::field::Empty,
        );
        self.body_check_traces.insert(span, body_trace.clone());
        body_trace
    }

    /// Preserve function decorator application after statement checking while retaining its facts and diagnostics
    /// with the declaration output. Builtin-only decorators need no post-body expression producer.
    pub(in crate::typechecker) fn complete_function_declaration(
        &mut self,
        function: &FunctionDecl,
        span: Span,
        index: usize,
    ) {
        if function
            .decorators
            .iter()
            .any(|decorator| self.is_user_defined_decorator_candidate(&decorator.node))
        {
            self.continue_declaration_owner(index, |checker| {
                checker.apply_user_defined_function_decorators(function, span)
            });
        }
    }

    /// Detach the completed declaration's exact lexical frame and checked contexts. Defaults and parameter bindings
    /// have already been checked; the next producer receives values, not a callback into declaration validation.
    pub(super) fn suspend_prepared_callable(
        &mut self,
        previous_annotation_owner: Option<super::AnnotationOwner>,
        previous_mutable_bindings: HashSet<String>,
        previous_mut_param_body: super::mutation::MutParamBodyState,
        yield_context: YieldContext,
        has_statement_body: bool,
    ) -> PreparedCallableCheck {
        let Some(bound_details) = self.current_type_param_bound_details.pop() else {
            unreachable!("callable preparation requires its installed generic-bound frame");
        };
        PreparedCallableCheck {
            scope: self.symbols.suspend_scope(),
            annotation_owner: std::mem::replace(&mut self.annotation_owner, previous_annotation_owner),
            bound_details,
            mutable_bindings: std::mem::replace(&mut self.mutable_bindings, previous_mutable_bindings),
            mut_param_body: self.suspend_mut_param_body(previous_mut_param_body, has_statement_body),
            return_error_type: self.current_return_error_type.take(),
            yield_context,
            has_statement_body,
        }
    }

    /// Enter a completed callable declaration's prepared frame without resolving annotations, defaults or bindings
    /// again. The symbol table resumes the exact scope, preserving identity and shadowing semantics.
    pub(super) fn enter_prepared_callable(
        &mut self,
        prepared: PreparedCallableCheck,
    ) -> (CallableCheckRestore, YieldContext) {
        self.symbols.resume_scope(prepared.scope);
        self.current_type_param_bound_details.push(prepared.bound_details);
        self.current_return_error_type = prepared.return_error_type;
        let restore = CallableCheckRestore {
            annotation_owner: std::mem::replace(&mut self.annotation_owner, prepared.annotation_owner),
            mutable_bindings: std::mem::replace(&mut self.mutable_bindings, prepared.mutable_bindings),
            mut_param_body: self.resume_mut_param_body(prepared.mut_param_body, prepared.has_statement_body),
        };
        (restore, prepared.yield_context)
    }

    /// Restore the enclosing scratch after consuming its prepared statement inputs. The declaration scope and its
    /// generic identities were created once by preparation, never rebound by the statement producer.
    pub(super) fn finish_prepared_callable(&mut self, restore: CallableCheckRestore) {
        self.current_return_error_type = None;
        self.current_type_param_bound_details.pop();
        self.annotation_owner = restore.annotation_owner;
        self.exit_mut_param_body(restore.mut_param_body);
        self.mutable_bindings = restore.mutable_bindings;
        self.symbols.exit_scope();
    }

    /// Check one callable's statements inside its already bound declaration frame. Defaults and declaration validation
    /// remain with the caller and are not repeated here. Iterator and C-span flow state belongs to this body and is
    /// restored before another body enters; checked output and pending decisions stay in the active body's writer.
    pub(in crate::typechecker) fn check_callable_body_in_frame(
        &mut self,
        body: &[Spanned<Statement>],
        is_async: bool,
        yield_context: YieldContext,
    ) {
        let previous_async = std::mem::replace(&mut self.in_async_body, is_async);
        let previous_yield = std::mem::replace(&mut self.current_yield_context, yield_context);
        let previous_iterators = std::mem::take(&mut self.consumed_iterator_bindings);
        let previous_resources = std::mem::take(&mut self.transferred_c_resource_bindings);
        let previous_comprehensions = self.enter_comprehension_source_scope();
        let previous_constructors = std::mem::take(&mut self.unbound_c_abi_span_constructors);
        let previous_spans = std::mem::take(&mut self.c_abi_span_bindings);
        let previous_consumed_spans = std::mem::take(&mut self.consumed_c_abi_span_bindings);

        self.check_statement_block(body);
        self.finalize_comprehension_collection_sources();

        self.consumed_iterator_bindings = previous_iterators;
        self.transferred_c_resource_bindings = previous_resources;
        self.exit_comprehension_source_scope(previous_comprehensions);
        self.unbound_c_abi_span_constructors = previous_constructors;
        self.c_abi_span_bindings = previous_spans;
        self.consumed_c_abi_span_bindings = previous_consumed_spans;
        self.in_async_body = previous_async;
        self.current_yield_context = previous_yield;
    }

    /// Validate a function's fixture marker before checking its signature. Async fixture yield errors retain the
    /// marker's source span and distinguish missing yields, invalid placement and missing yielded values.
    pub(super) fn check_function_fixture_marker(&mut self, func: &FunctionDecl) -> Option<TestingFixtureMarkerArgs> {
        let fixture_span = fixture_function_span(func);
        let fixture_args = self.testing_fixture_marker_args(&func.decorators, fixture_span);
        if let Some(args) = &fixture_args {
            if let Some(span) = args.unsupported_timeout_span {
                self.errors
                    .push(errors::fixture_timeout_config_not_supported(&func.name, span));
            }
            if func.is_async() {
                match validate_async_fixture_yield_shape(&func.body) {
                    AsyncFixtureYieldShape::Valid { .. } => {}
                    AsyncFixtureYieldShape::Missing => {
                        self.errors
                            .push(errors::async_fixture_requires_yield(&func.name, fixture_span));
                    }
                    AsyncFixtureYieldShape::Invalid(span) => {
                        self.errors
                            .push(errors::async_fixture_invalid_yield_shape(&func.name, span));
                    }
                    AsyncFixtureYieldShape::MissingValue(span) => {
                        self.errors
                            .push(errors::async_fixture_yield_requires_value(&func.name, span));
                    }
                }
            }
        }
        fixture_args
    }

    /// Publish one execution-frame parameter after all declaration-owned defaults have checked. Explicit input
    /// obligations retain their declaration metadata while the body sees the underlying value. A `mut` binding
    /// cannot upgrade shared access; executable storage remains separately gated until lowering honors the mode.
    pub(super) fn bind_checked_callable_parameter(
        &mut self,
        param: &Spanned<Param>,
        resolved_ty: ResolvedType,
        slot: usize,
    ) {
        let input = self
            .current_body_identity()
            .and_then(|identity| self.checked_input_contract(identity, slot))
            .cloned();
        if param.node.is_mut && param.node.kind == ParamKind::Normal {
            if let Some(contract) = &input {
                self.type_info.declarations.mut_param_markers.insert(
                    (param.span, param.node.name.clone()),
                    contract.mode == incan_lang::lang::types::ownership::OwnershipMode::BorrowedMut,
                );
            } else {
                self.record_mut_param_marker(&param.node, param.span, &resolved_ty);
            }
        }
        let writable = param.node.is_mut
            && !input
                .as_ref()
                .is_some_and(|contract| contract.mode == incan_lang::lang::types::ownership::OwnershipMode::Borrowed);
        let value_type = input.map_or(resolved_ty, |contract| contract.underlying);
        let ty = local_type_for_param(param.node.kind, value_type);
        self.validate_protected_builtin_binding(&param.node.name, param.span);
        if writable {
            self.bind_mut_param_as_mutable(&param.node);
        }
        self.symbols.define_with_target_kind(
            Symbol {
                name: param.node.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: writable,
                    is_used: false,
                }),
                span: param.span,
                scope: 0,
            },
            SemanticSourceTargetKind::Parameter,
        );
        self.record_read_only_binding(writable, param.span);
        self.record_write_target_identity(param.span, &param.node.name);
    }

    /// Resolve a callable return annotation and apply its nominal, nested-trait and Rust lifetime restrictions in
    /// order. Signature owners retain responsibility for route-specific checks and recording their final binding.
    /// A result with a `from` source defers its outer ownership mode, or the mode of a carrier's first payload, to the
    /// declaration's returned-view check. A top-level `Owned` result has no source dependency to resolve, but its
    /// independence still belongs to that boundary check rather than the general annotation resolver.
    pub(super) fn resolve_callable_return_annotation(
        &mut self,
        annotation: &Spanned<Type>,
        has_source: bool,
    ) -> ResolvedType {
        let ty = self.resolve_type_checked_deferring(
            annotation,
            |mode| has_source || mode == incan_lang::lang::types::ownership::OwnershipMode::Owned,
            has_source,
        );
        self.refuse_unbounded_nominal_type_arguments(&ty, annotation.span);
        self.refuse_trait_type_nested_in_annotation(&ty, annotation.span);
        let retained_native_result = has_source
            && self
                .explicit_root_contract(&annotation.node, &ty)
                .is_some_and(|mode| mode.mode == incan_lang::lang::types::ownership::OwnershipMode::Owned);
        if !retained_native_result {
            self.refuse_lifetime_bearing_rust_type(&ty, annotation.span);
        }
        ty
    }

    /// Resolve callable parameter annotations and validate source defaults before their execution-frame bindings exist.
    ///
    /// A default may use module-visible declarations and the current function or method's active generic bounds, but
    /// it may not read `self` or another callable parameter. Keeping this check before the frame bindings makes the
    /// source typechecker agree with call-time default materialization and records canonical expression facts for
    /// compiler-owned consumers without requiring them to reconstruct source typing.
    /// Native inputs keep their checked borrow shape; their erased metadata regions receive independent input
    /// binders at the native signature boundary, without granting an output or field an escaping loan.
    pub(super) fn resolve_callable_parameter_types_and_check_defaults(
        &mut self,
        params: &[Spanned<Param>],
        defer_inputs: bool,
    ) -> Vec<ResolvedType> {
        params
            .iter()
            .map(|param| {
                let param_ty = self.resolve_type_checked_for_input(&param.node.ty, defer_inputs);
                self.refuse_unbounded_nominal_type_arguments(&param_ty, param.node.ty.span);
                self.refuse_trait_type_nested_in_annotation(&param_ty, param.node.ty.span);
                if param.node.kind != ParamKind::Normal {
                    return param_ty;
                }
                let Some(default) = &param.node.default else {
                    return param_ty;
                };

                let previous_callable_default_context = std::mem::replace(&mut self.checking_callable_default, true);
                self.check_work.callable_defaults += 1;
                let default_ty = self.check_expr_with_expected(default, Some(&param_ty));
                self.checking_callable_default = previous_callable_default_context;
                if !self.types_compatible(&default_ty, &param_ty)
                    && !self.record_validated_newtype_coercion_if_possible(&default_ty, &param_ty, default.span)
                {
                    self.errors.push(errors::type_mismatch(
                        &param_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
                param_ty
            })
            .collect()
    }

    /// Resolve generic type-parameter bounds while preserving trait type arguments for call-site checks.
    pub(super) fn type_param_bound_details_from_type_params(
        &mut self,
        type_params: &[TypeParam],
    ) -> HashMap<String, Vec<TypeBoundInfo>> {
        type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| {
                            let name = self.resolve_generic_bound_name(&bound.name, Span::default());
                            let module_path = self.trait_bound_module_path(&bound.name);
                            TypeBoundInfo {
                                name,
                                source_name: self.trait_bound_source_name(&bound.name),
                                type_args: bound
                                    .type_args
                                    .iter()
                                    .map(|type_arg| self.resolve_type_checked(type_arg))
                                    .collect(),
                                module_path,
                                implementation_type_params: Vec::new(),
                                inferred: false,
                            }
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Validate bound names and type arguments for declarations whose collected signatures otherwise have no semantic
    /// body pass.
    ///
    /// A bound name resolves as a builtin trait (`Eq`, `Ord`, `Awaitable`, ...), a builtin derive bound (`Copy`
    /// included, which names no builtin trait of its own; see [`Self::builtin_derive_bound`]) or a builtin type in
    /// either spelling (`int` or `Int`, #1033), none of which appears in the symbol table under every spelling, or as
    /// a trait or type the program declares or imports.
    pub(super) fn validate_type_param_bound_type_names(&mut self, type_params: &[TypeParam]) {
        for type_param in type_params {
            for bound in &type_param.bounds {
                let resolved_name = self.resolve_trait_bound_name(&bound.name, type_param.span);
                let is_builtin = builtin_traits::from_str(&resolved_name).is_some()
                    || self.builtin_derive_bound(&resolved_name).is_some()
                    || ResolvedType::builtin_simple(&resolved_name).is_some();
                if !is_builtin && self.lookup_symbol(&resolved_name).is_none() {
                    self.errors.push(errors::unknown_symbol(&bound.name, type_param.span));
                }
                for type_arg in &bound.type_args {
                    let _ = self.resolve_type_checked(type_arg);
                }
            }
        }
    }

    /// Return the first trait leaf stored inside a field annotation, including nested collection positions.
    pub(super) fn stored_trait_annotation(&self, ty: &ResolvedType) -> Option<String> {
        match ty {
            ResolvedType::Named(name) if self.lookup_semantic_trait_info(name).is_some() => Some(name.clone()),
            ResolvedType::Generic(name, args) => {
                if self.lookup_semantic_trait_info(name).is_some() {
                    return Some(name.clone());
                }
                args.iter().find_map(|arg| self.stored_trait_annotation(arg))
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.stored_trait_annotation(inner),
            ResolvedType::TypeToken(_) => None,
            ResolvedType::FrozenDict(key, value) => self
                .stored_trait_annotation(key)
                .or_else(|| self.stored_trait_annotation(value)),
            ResolvedType::Function(params, returned) => params
                .iter()
                .find_map(|param| self.stored_trait_annotation(&param.ty))
                .or_else(|| self.stored_trait_annotation(returned)),
            ResolvedType::Tuple(items) => items.iter().find_map(|item| self.stored_trait_annotation(item)),
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Named(_)
            | ResolvedType::Trait(_)
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => None,
        }
    }

    /// Refuse a field annotation that would store a bare trait value.
    pub(super) fn refuse_stored_trait_field(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        field: &FieldDecl,
        ty: &ResolvedType,
    ) {
        if let Some(trait_name) = self.stored_trait_annotation(ty) {
            self.errors.push(errors::trait_typed_field_annotation_unsupported(
                owner_kind,
                owner_name,
                &field.name,
                &field.ty.node.to_string(),
                &trait_name,
                field.ty.span,
            ));
        }
    }

    /// Validate a model, class, enum, or newtype method body using the concrete nominal owner as `self`.
    ///
    /// `method_span` selects the exact checked declaration binding consumed by Body IR, including same-name
    /// overloads and methods on different nominal owners (#1121).
    pub(super) fn check_method_with_owner_type_params(
        &mut self,
        method: &MethodDecl,
        method_span: Span,
        owner: &str,
        owner_params: &[TypeParam],
    ) {
        let index = self.body_checks.len();
        let prepared = self.check_declaration_owner(method_span, |checker| {
            checker.prepare_owned_method(method, method_span, owner, owner_params)
        });
        if let Some((prepared, previous_owner)) = prepared {
            self.check_prepared_method(method, method_span, prepared);
            #[cfg(test)]
            self.observe_body_local(method_span);
            self.current_method_owner = previous_owner;
            self.continue_declaration_owner(index, |checker| {
                checker.apply_user_defined_method_decorators(method, owner)
            });
        }
    }

    /// Validate concrete-method metadata before its signature and defaults. A missing concrete body retains the
    /// existing early refusal and decorator behavior; no statement producer is fabricated for that declaration.
    pub(super) fn prepare_owned_method(
        &mut self,
        method: &MethodDecl,
        span: Span,
        owner: &str,
        owner_params: &[TypeParam],
    ) -> Option<(PreparedMethodCheck, Option<String>)> {
        self.validate_decorators_allowing_user_defined(&method.decorators);
        self.validate_made_private_decorators(
            &method.decorators,
            method.visibility,
            &format!("method '{owner}.{}'", method.name),
        );
        let declaration_name = format!("{owner}.{}", method.name);
        self.check_registry_description_decorators(
            &method.decorators,
            &declaration_name,
            method.return_type.span,
            SemanticRegistrySubjectKind::Method,
        );
        if method.body.is_none() {
            self.errors.push(errors::concrete_method_requires_body(
                &method.name,
                method.return_type.span,
            ));
            self.apply_user_defined_method_decorators(method, owner);
            return None;
        }
        let self_ty = self.concrete_method_self_type(owner);
        let previous_owner = self.current_method_owner.replace(owner.to_string());
        Some((
            self.prepare_method_declaration(method, span, self_ty, owner_params),
            previous_owner,
        ))
    }
}
