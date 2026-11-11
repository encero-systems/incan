//! The program check pipeline from declaration collection to the finished result.

use super::*;

impl TypeChecker {
    /// Collect concrete declarations first, then resolve aliases and partials in dependency order.
    ///
    /// Aliases may target partials and partials may target aliases, so two fixed phases are insufficient. Retrying
    /// unresolved projections to a fixed point preserves forward references without guessing through source names.
    /// Anything still unresolved after a no-progress pass is collected once so the normal unknown-target diagnostics
    /// remain authoritative (and the declaration validator can additionally report cycles).
    pub(super) fn collect_declarations_for_check(&mut self, declarations: &[Spanned<Declaration>]) {
        for decl in declarations {
            if !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }

        let mut pending = declarations
            .iter()
            .filter(|decl| matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)))
            .collect::<Vec<_>>();
        while !pending.is_empty() {
            let mut deferred = Vec::new();
            let mut made_progress = false;
            for decl in pending {
                let target = match &decl.node {
                    Declaration::Alias(alias) => &alias.target.segments,
                    Declaration::Partial(partial) => &partial.target.segments,
                    _ => unreachable!("pending projection declarations contain only aliases and partials"),
                };
                if self.alias_target_symbol_kind(target).is_some() {
                    self.collect_declaration(decl);
                    made_progress = true;
                } else {
                    deferred.push(decl);
                }
            }
            if !made_progress {
                for decl in deferred {
                    self.collect_declaration(decl);
                }
                break;
            }
            pending = deferred;
        }
    }

    /// Return whether the program last checked declares `name` itself and marks that declaration `pub`.
    pub fn declares_public(&self, name: &str) -> bool {
        self.current_module_public_declarations.contains(name)
    }

    /// Check a program and return errors if any.
    ///
    /// Runs the two-pass type-checking algorithm:
    /// 1. **Collect**: register all type/function declarations in the symbol table.
    /// 2. **Check**: validate bodies, expressions, and cross-references.
    ///
    /// ## Parameters
    ///
    /// - `program`: the parsed AST to validate.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    ///
    /// ## Notes
    ///
    /// For multi-module projects, call [`import_module`](Self::import_module) first to populate dependency symbols,
    /// or use [`check_with_imports`](Self::check_with_imports) to import dependencies first.
    pub fn check_program(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        let preserve_dependency_semantics = std::mem::take(&mut self.dependency_semantics_pending);
        let result = self.check_program_internal(program, preserve_dependency_semantics);
        self.clear_nominal_identity_cache();
        result
    }

    /// Return the actual entries performed by the most recent program activation. The observation does not retain
    /// checker facts and never decides cache reuse, checking scope or publication readiness.
    pub fn check_work(&self) -> CheckWork {
        self.check_work
    }

    /// Check one program while retaining semantic-only type metadata collected from its dependency imports.
    pub(super) fn check_program_with_dependency_semantics(
        &mut self,
        program: &Program,
    ) -> Result<(), Vec<CompileError>> {
        self.dependency_semantics_pending = false;
        let result = self.check_program_internal(program, true);
        self.clear_nominal_identity_cache();
        result
    }

    /// Check one program in declaration, body and publication order, preserving dependency-only semantic caches
    /// only when the caller has collected them for this program. Accumulated fatal diagnostics prevent nominal
    /// snapshot publication.
    pub(super) fn check_program_internal(
        &mut self,
        program: &Program,
        preserve_dependency_semantics: bool,
    ) -> Result<(), Vec<CompileError>> {
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::reset_program_check",
            )
            .entered();
            self.reset_program_check(program, preserve_dependency_semantics);
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::collect_program_declarations",
                declarations = program.declarations.len(),
            )
            .entered();
            self.collect_program_check_declarations(program);
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::record_program_header_facts",
            )
            .entered();
            self.record_program_header_facts();
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::record_source_declaration_identities",
            )
            .entered();
            self.record_source_declaration_identities(program);
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::record_type_param_declaration_spans",
            )
            .entered();
            self.record_type_param_declaration_spans(program);
        }
        self.visibility_point = VisibilityPoint::Collected;
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::prepare_program_body_facts",
            )
            .entered();
            self.prepare_program_body_facts();
        }
        if let Some(reuse) = self.callable_body_reuse.take() {
            reuse.install(program, self);
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::check_program_declaration_bodies",
                declarations = program.declarations.len(),
            )
            .entered();
            self.check_program_declaration_bodies(program);
        }
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::record_program_lowering_facts",
            )
            .entered();
            self.record_program_lowering_facts(program);
        }
        let result = {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::finish_program_check",
            )
            .entered();
            self.finish_program_check(program)
        };
        self.visibility_point = VisibilityPoint::Finished;
        result
    }

    /// Clear per-program observations before collecting declarations. Dependency semantic inventories survive only
    /// for an explicitly prepared imported program; symbol registration and alias validation keep their prior order.
    pub(super) fn reset_program_check(&mut self, program: &Program, preserve_dependency_semantics: bool) {
        #[cfg(test)]
        self.body_local_observations.clear();
        #[cfg(test)]
        self.body_enclosing_observations.clear();
        self.check_work = CheckWork::default();
        self.body_checks.clear();
        self.checked_bodies.clear();
        self.checked_declarations.clear();
        self.produced_declaration_records.clear();
        self.produced_local_records.clear();
        self.selected_local_records.clear();
        self.selected_final_records.clear();
        self.produced_final_records.clear();
        self.final_record_refusals.clear();
        self.callable_declaration_reuse.clear();
        self.refused_ownership_authorities.clear();
        self.diagnostic_owner_ranges.clear();
        self.body_open_diagnostics.clear();
        self.visibility_point = VisibilityPoint::Collecting;
        self.checking_nominal_scope.get_mut().take();
        self.validate_source_type_names = false;
        self.unknown_source_type_names_emitted.clear();
        self.source_type_declaration_names = Self::collect_source_type_declaration_names(&program.declarations);
        self.source_type_alias_names = Self::collect_source_type_alias_names(&program.declarations);
        self.const_decls.clear();
        self.static_decls.clear();
        self.local_function_decls.clear();
        self.receiver_plan_inputs = Default::default();
        self.current_module_function_symbols.clear();
        self.rejected_member_bindings.clear();
        self.warned_public_c_abi_raw_call_owners.clear();
        self.static_decl_positions.clear();
        self.const_eval_state.clear();
        self.const_eval_cache.clear();
        self.type_info = TypeCheckInfo::default();
        self.view_type_cache.get_mut().clear();
        if preserve_dependency_semantics {
            self.type_info
                .declarations
                .ownership_contracts
                .clone_from(&self.dependency_method_input_contracts);
        } else {
            self.dependency_method_input_contracts.clear();
        }
        self.type_info.declarations.record_package_identity = self.record_package_identity.clone();
        self.warnings.clear();
        self.errors.clear();
        self.pending_uncopyable_dict_lookups.clear();
        self.mutable_bindings.clear();
        self.static_alias_bindings.clear();
        self.read_only_binding_spans.clear();
        self.closure_depth = 0;
        self.scoped_c_string_view_receivers.clear();
        self.unbound_scoped_c_string_views.clear();
        self.scoped_c_string_view_bindings.clear();
        self.testing_marker_import_bindings.clear();
        self.surface_function_import_bindings.clear();
        self.surface_type_import_bindings.clear();
        self.module_declared_type_names = Self::collect_module_declared_type_names(program);
        self.testing_fixture_names.clear();
        self.testing_marker_semantics = None;
        self.local_rust_derive_paths.clear();
        self.local_derive_facts.clear();
        self.hash_key_inference.local_requirements.clear();
        self.hash_key_inference.function_bounds.clear();
        self.source_import_targets.clear();
        self.surface_context = SurfaceContext::from_program(program);
        self.supertrait_closure.clear();
        self.enter_nominal_declaring_module(program);
        if !preserve_dependency_semantics {
            self.clear_nominal_declaring_modules();
            self.hash_key_inference.type_params.clear();
            self.transitive_pub_types.clear();
            self.public_library_type_identities.clear();
            self.transitive_pub_traits.clear();
            self.cached_pub_libraries.clear();
            self.foreign_pub_type_remappings.clear();
            self.pub_library_declaration_scopes.clear();
        }
    }

    /// Validate alias and reserved declaration spellings, then retain public names before signature collection.
    pub(super) fn prepare_program_declaration_names(&mut self, program: &Program) {
        self.validate_alias_declarations(program);
        self.report_source_name_diagnostics(program);
        self.current_module_public_declarations = program
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(declaration_name)
            .map(str::to_string)
            .collect();
    }

    /// Collect signatures and settle inherited trait requirements before checking any body or call. Dependency
    /// supertraits resolve first, and callable capture planning uses the complete current declaration inventory.
    pub(super) fn collect_program_check_declarations(&mut self, program: &Program) {
        self.prepare_program_declaration_names(program);
        // `check_with_imports` / `import_module` can queue supertrait bounds while collecting dependency ASTs.
        // Resolve those queued bounds into trait symbols before we collect and resolve the current program.
        if !self.pending_trait_supertraits.is_empty() {
            self.resolve_pending_trait_supertraits();
        }

        // First pass: collect concrete declarations, then aliases and partials after their possible targets are
        // available. Signatures that named a type declared further down the module now resolve to it (#1780).
        self.collect_declarations_for_check(&program.declarations);
        self.resolve_forward_type_references(&program.declarations);
        self.record_local_nominal_type_param_bounds(&program.declarations);
        // Every declaration is collected, so the signatures' hashed type parameters can be inferred before any body
        // or call is checked (#1758).
        self.infer_hash_key_type_params(&program.declarations, true);

        self.resolve_pending_trait_supertraits();
        self.finalize_supertrait_graph();
        self.merge_supertrait_requires_into_traits();
        self.settle_method_scopes(&program.declarations);
        self.validate_static_dependencies();
        self.collect_testing_fixture_names(program);
        // Every signature is collected, so the function-typed parameters and returns that hold capturing closures
        // can be decided before any call or body is checked (#1561).
        self.plan_closure_holding_callables(program);
        self.collect_function_ownership_headers(program);
    }

    /// Publish declaration-owned identity, import and alias bindings once collection has settled them. Body checking
    /// may add lexical identities, but it does not own or republish the module's header binding inventory.
    pub(super) fn record_program_header_facts(&mut self) {
        self.export_module_declaration_bindings();
        self.export_checked_import_bindings();
        self.export_type_alias_targets();
    }

    /// Retain private declaration-time native and nominal facts for body checking, reusing only matching mutation
    /// revisions. Finalization publishes the current inventory only after accumulated fatal diagnostics are absent.
    pub(super) fn prepare_program_body_facts(&mut self) {
        // Direct-route call and pattern facts need the already collected native owners and their public paths while
        // checking arguments and results. Keep this snapshot private; ordinary nominal tables retain their final
        // successful-check timing.
        self.type_info.declarations.body_ir_native_type_identities = self.checked_native_nominal_type_identities();
        self.type_info.rust.item_public_paths = self.checked_rust_item_public_paths();
        // Internal carrier identities must survive per-module fact resets before Result payloads are checked. Source
        // alias entries still publish only after successful checking.
        let nominal_scope = self.checked_nominal_type_identities();
        self.type_info.declarations.named_type_identities = nominal_scope
            .iter()
            .filter(|(name, identity)| {
                incan_semantics_core::body_ir::nominal_carrier_key(identity).as_deref() == Some(name.as_str())
            })
            .map(|(name, identity)| (name.clone(), identity.clone()))
            .collect();
    }

    /// Check constants before other declarations, then settle argument ownership and deferred dictionary refusals.
    /// Source-name validation is active across both passes and remains active until native declarations are checked.
    pub(super) fn check_program_declaration_bodies(&mut self, program: &Program) {
        // Second pass: check consts first so their resolved types are available to later checks.
        self.validate_source_type_names = true;
        for decl in &program.declarations {
            if matches!(decl.node, Declaration::Const(_)) {
                self.check_environment_declaration(decl);
            }
        }
        for decl in &program.declarations {
            if !matches!(decl.node, Declaration::Const(_)) {
                if self.has_declaration_record(decl.span) {
                    let Declaration::Function(function) = &decl.node else {
                        self.check_environment_declaration(decl);
                        continue;
                    };
                    self.visibility_point = VisibilityPoint::BeforeDeclaration(decl.span);
                    self.check_reused_function(program, function, decl.span);
                    self.visibility_point = VisibilityPoint::AfterDeclaration(decl.span);
                } else {
                    self.check_environment_record_declaration(program, decl);
                }
            }
        }
        #[cfg(test)]
        if self.legacy_body_checking {
            self.legacy_resolve_mut_arguments();
            self.refuse_copying_dict_lookups_of_uncopyable_values();
        } else {
            self.settle_body_checks(program);
        }
        #[cfg(not(test))]
        self.settle_body_checks(program);
    }

    /// Keep a retained scalar body only at its original physical lexical scope. Earlier unsupported bodies may
    /// introduce different scopes after an edit; that invalidates this product rather than renumbering identities.
    pub(in crate::typechecker) fn callable_body_matches_current_scope(&mut self, span: Span) -> bool {
        let Some(scope) = self.type_info.reused_callable_scopes.get(&span) else {
            return false;
        };
        if scope.is_none_or(|scope| scope == self.symbols.next_scope_index()) {
            return self.type_info.reused_callable_bodies.contains_key(&span);
        }
        self.type_info.reused_callable_scopes.remove(&span);
        if let Some(body) = self.type_info.reused_callable_bodies.remove(&span) {
            self.type_info
                .reused_callable_contexts
                .retain(|(_, owner, _)| Some(owner) != body.canonical.as_ref());
        }
        false
    }

    /// Retain layout and trait facts after body checking without publishing a nominal snapshot yet. Native module
    /// validation precedes inferred-bound and nested-identity export; module header bindings have already been
    /// published.
    pub(super) fn record_program_lowering_facts(&mut self, program: &Program) {
        self.type_info
            .c_abi
            .resolve_checked_facades(self.current_module_path.as_deref());
        self.record_trait_metadata_for_lowering();
        self.record_standard_registry_types();
        self.record_trait_adoption_arguments_for_lowering(program);
        self.record_body_ir_bare_adoption_arguments(program);
        self.record_model_field_visibilities_for_lowering(program);
        self.record_class_layouts_for_lowering(program);
        self.record_body_ir_class_inheritance(program);
        self.record_method_decorator_receiver_slots(program);

        self.validate_source_type_names = false;
        // The module is checked; its inferred hashed type parameters now join its exported signatures (#1758).
        self.export_inferred_hash_key_bounds();

        self.export_nested_declaration_identities();
        self.record_binding_collision_diagnostics();
    }

    /// Resolve final destination and artifact demands, retain warnings, and publish only after fatal diagnostics are
    /// absent. Publication's own demands are validated again before the checked result is accepted.
    pub(super) fn finish_program_check(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        if self.loaves.keys().next().is_some() {
            for (name, identity) in &self.public_library_type_identities {
                if identity.selected_provider.is_none()
                    && matches!(
                        identity.canonical.as_ref().map(|identity| &identity.origin),
                        Some(incan_semantics_core::SymbolOrigin::Package { .. })
                    )
                {
                    self.errors.push(CompileError::type_error(
                        format!("compiled nominal `{name}` has no unique admitted declaring artifact"),
                        Span::default(),
                    ));
                }
            }
        }
        self.settle_open_value_destinations();
        self.errors.append(&mut self.body_open_diagnostics);
        if let Err(error) = self.loaves.validate_demands() {
            self.errors
                .push(CompileError::type_error(error.to_string(), Span::default()));
        }
        // Split fatal errors from non-fatal diagnostics.
        let all = std::mem::take(&mut self.errors);
        let (fatal, non_fatal): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|e| !matches!(e.kind, ErrorKind::Warning | ErrorKind::Lint));
        self.warnings.extend(non_fatal);

        if fatal.is_empty() {
            self.publish_checked_nominal_scope();
            self.publish_rust_storage_types(program);
            self.type_info.body_ir_newtypes = native_newtype_facts::project(program, &self.type_info);
            self.publish_body_ir_empty_lists();
            self.checking_nominal_scope.get_mut().take();
            if let Err(error) = self.loaves.validate_demands() {
                return Err(vec![CompileError::type_error(error.to_string(), Span::default())]);
            }
            Ok(())
        } else {
            self.checking_nominal_scope.get_mut().take();
            Err(fatal)
        }
    }

    /// Publish the checked module-scope nominal and native identity tables Body IR reads, once the module is checked.
    pub(super) fn publish_checked_nominal_scope(&mut self) {
        self.type_info.declarations.named_type_identities = self.take_checked_nominal_type_identities();
        self.type_info.declarations.body_ir_native_type_identities = self.checked_native_nominal_type_identities();
        self.type_info.body_ir_runtime_types = self.checked_body_ir_runtime_types();
        self.type_info.rust.item_public_paths = self.checked_rust_item_public_paths();
        self.type_info.rust.owner_parameters = self.checked_rust_owner_parameters();
    }
}
