//! Checker construction, the default checker, and module, package and dependency configuration setters.

use super::*;

impl TypeChecker {
    /// Create an empty typechecker with fresh symbol, diagnostic, import, and lowering-metadata state.
    ///
    /// Native queries, statement-block tracking and hash-key inference each initialize their own empty state.
    pub fn new() -> Self {
        Self {
            record_package_identity: None,
            #[cfg(test)]
            legacy_body_checking: false,
            #[cfg(test)]
            active_declaration_checks: Vec::new(),
            #[cfg(test)]
            body_local_observations: Vec::new(),
            #[cfg(test)]
            body_enclosing_observations: Vec::new(),
            check_work: CheckWork::default(),
            body_checks: Vec::new(),
            checked_bodies: Vec::new(),
            checked_declarations: Vec::new(),
            produced_declaration_records: std::collections::BTreeMap::new(),
            produced_local_records: std::collections::BTreeMap::new(),
            selected_local_records: std::collections::BTreeMap::new(),
            produced_final_records: std::collections::BTreeMap::new(),
            final_record_refusals: std::collections::BTreeMap::new(),
            callable_declaration_reuse: std::collections::BTreeMap::new(),
            lazy_result_bodies: HashSet::new(),
            iteration_replacement_bodies: HashSet::new(),
            dictionary_lookup_bodies: HashSet::new(),
            loop_item_bodies: HashSet::new(),
            loop_item_candidates: std::collections::BTreeMap::new(),
            active_loop_items: std::collections::BTreeMap::new(),
            refused_ownership_authorities: HashSet::new(),
            diagnostic_owner_ranges: Vec::new(),
            body_open_diagnostics: Vec::new(),
            symbols: SymbolTable::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            mutable_bindings: HashSet::new(),
            mutation: ownership_contracts::mutation::MutationFacts::default(),
            current_immutable_self_method: None,
            consumed_iterator_bindings: HashMap::new(),
            pending_uncopyable_dict_lookups: Vec::new(),
            static_alias_bindings: HashMap::new(),
            read_only_binding_spans: HashSet::new(),
            closure_depth: 0,
            scoped_c_string_view_receivers: HashSet::new(),
            c_abi_raw_argument_spans: HashSet::new(),
            unbound_scoped_c_string_views: HashSet::new(),
            scoped_c_string_view_bindings: HashMap::new(),
            transferred_c_resource_bindings: HashMap::new(),
            comprehension_sources: comprehension_sources::ComprehensionSources::default(),
            capturing_callables: capturing_callables::CapturingCallables::default(),
            own_standard_package: None,
            unbound_c_abi_span_constructors: HashMap::new(),
            c_abi_span_bindings: HashMap::new(),
            consumed_c_abi_span_bindings: HashMap::new(),
            pending_c_abi_output_slots: HashMap::new(),
            unbound_c_abi_output_slot_constructors: HashMap::new(),
            unbound_c_abi_raw_call_results: HashMap::new(),
            open_rust_generic_calls: HashMap::new(),
            open_rust_generic_bindings: Vec::new(),
            tied_rust_trait_callees: HashMap::new(),
            type_param_declaration_spans: HashMap::new(),
            unannotated_binding_value: None,
            statement_blocks: StatementBlockState::default(),
            c_abi_raw_call_results: Vec::new(),
            available_c_abi_output_slots: HashSet::new(),
            consumed_c_abi_output_slots: HashMap::new(),
            current_return_error_type: None,
            current_yield_context: YieldContext::Disallowed,
            in_async_body: false,
            await_operand_span: None,
            call_argument_depth: 0,
            unsafe_depth: 0,
            type_receiver_spans: Vec::new(),
            type_token_value_spans: Vec::new(),
            generic_partial_target_span: None,
            decorator_callable_span: None,
            generic_partial_target: None,
            loop_stack: Vec::new(),
            current_trait_requires: None,
            current_trait_properties: None,
            current_trait_name: None,
            current_method_owner: None,
            current_classmethod_self_ty: None,
            current_c_abi_raw_call_owner: None,
            warned_public_c_abi_raw_call_owners: HashSet::new(),
            current_type_param_bound_details: Vec::new(),
            nominal_type_param_bounds: HashMap::new(),
            current_trait_missing_requires_emitted: None,
            const_decls: HashMap::new(),
            static_decls: Vec::new(),
            local_function_decls: HashMap::new(),
            callable_body_reuse: None,
            current_module_public_declarations: HashSet::new(),
            receiver_plan_inputs: Default::default(),
            body_ir_decorator_factory_span: None,
            decorator_callable_type_params: Vec::new(),
            current_module_function_symbols: HashMap::new(),
            type_aliases: NominalInventory::default(),
            rejected_member_bindings: HashSet::new(),
            dependency_import_type_alias_transaction: None,
            validate_source_type_names: false,
            unknown_source_type_names_emitted: HashSet::new(),
            source_type_declaration_names: HashSet::new(),
            source_type_alias_names: HashSet::new(),
            annotation_owner: None,
            static_decl_positions: HashMap::new(),
            checking_registry_entry_static_initializer: false,
            checking_callable_default: false,
            const_eval_state: HashMap::new(),
            const_eval_cache: HashMap::new(),
            type_info: TypeCheckInfo::default(),
            view_type_cache: RefCell::new(HashMap::new()),
            visibility_point: VisibilityPoint::default(),
            dependency_exports: HashMap::new(),
            selected_dependency_interfaces: Vec::new(),
            dependency_interface_preparation_cache: Default::default(),
            dependency_member_symbols: HashMap::new(),
            dependency_member_type_aliases: HashMap::new(),
            dependency_registry_definitions: HashMap::new(),
            dependency_registry_consts: HashMap::new(),
            dependency_member_partial_projections: HashMap::new(),
            dependency_direct_member_symbols: HashMap::new(),
            dependency_direct_member_type_aliases: HashMap::new(),
            dependency_direct_member_identities: HashMap::new(),
            declaration_nominal_scopes: HashMap::new(),
            retained_nominal_carriers: std::collections::BTreeMap::new(),
            checking_nominal_scope: RefCell::new(None),
            declaration_callables: HashMap::new(),
            dependency_member_reexports: HashMap::new(),
            body_ir_dependency_pub_reexports: HashMap::new(),
            dependency_module_path_segments: HashMap::new(),
            nominal_declarations: union_member_identity::NominalDeclarationContext::default(),
            dependency_direct_member_partial_projections: HashMap::new(),
            dependency_derivable_modules: HashMap::new(),
            dependency_derivable_trait_identities: HashMap::new(),
            dependency_module_traits: HashMap::new(),
            dependency_trait_rust_derive_paths: HashMap::new(),
            local_rust_derive_paths: HashMap::new(),
            local_derive_facts: HashMap::new(),
            hash_key_inference: HashKeyInferenceState::default(),
            loaves: Arc::new(LoafIndex::default()),
            not_prepared_loaves: HashSet::new(),
            standard_traits: HashMap::new(),
            transitive_pub_types: HashMap::new(),
            public_library_type_identities: NominalInventory::default(),
            transitive_pub_traits: HashMap::new(),
            cached_pub_libraries: HashSet::new(),
            foreign_pub_type_remappings: HashMap::new(),
            pub_library_declaration_scopes: HashMap::new(),
            dependency_semantics_pending: false,
            dependency_method_input_contracts: Default::default(),
            current_module_path: None,
            source_import_targets: HashMap::new(),
            declared_crate_names: None,
            testing_marker_import_bindings: HashSet::new(),
            surface_function_import_bindings: HashMap::new(),
            surface_type_import_bindings: HashMap::new(),
            module_declared_type_names: HashSet::new(),
            testing_fixture_names: HashSet::new(),
            testing_marker_semantics: None,
            surface_context: SurfaceContext::default(),
            supertrait_closure: HashMap::new(),
            pending_trait_supertraits: Vec::new(),
            native_queries: NativeQueryState::default(),
        }
    }

    /// Install producer-validated bodies for one exact program. Declaration collection and final publication still
    /// run; only admitted scalar bodies with complete source-call witnesses skip checking and feed Body IR directly.
    pub fn set_callable_body_reuse(&mut self, products: crate::declaration_interface::ValidatedCallableBodies) {
        self.callable_body_reuse = Some(products);
    }

    /// Set the declared Rust crate names from `loaf.toml [rust-dependencies]` for checked direct imports and derives.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the compiled Loaves this program imports from: its dependencies and the standard packages (DD-0007).
    pub fn set_loaves(&mut self, loaves: Arc<LoafIndex>) {
        self.loaves = loaves;
    }

    /// Name dependency bindings whose manifests are unavailable because unattended preparation stopped before
    /// untrusted compile-time code. Imports from these bindings remain opaque and report one stable informational
    /// diagnostic instead of being misclassified as unknown packages.
    pub fn set_not_prepared_loaves(&mut self, names: HashSet<String>) {
        self.not_prepared_loaves = names;
    }

    /// Mark the program being checked as the standard package `policy` names.
    pub fn set_own_standard_package(&mut self, policy: &'static StandardPackageNamespacePolicy) {
        self.own_standard_package = Some(policy);
    }

    /// Record the module path owning this checker's declarations.
    ///
    /// Also forwarded to the symbol table, whose RFC 120 identity minting uses it as the [`SymbolOrigin`] of every
    /// locally-declared symbol; `None` resets to the anonymous-module origin.
    pub fn set_current_module_path(&mut self, path: Option<Vec<String>>) {
        self.symbols.set_module_path(path.clone().unwrap_or_default());
        self.current_module_path = path;
    }

    /// Set the defining package for declarations checked for a compiled-library artifact.
    pub fn set_current_package_identity(&mut self, package_identity: Option<String>) {
        self.symbols.set_package_identity(package_identity);
    }

    /// Install the already admitted owning package fact without changing symbol ownership or asking a registry.
    pub fn set_record_package_identity(
        &mut self,
        identity: Option<incan_semantics_core::record_identity::RecordPackageIdentity>,
    ) {
        self.record_package_identity = identity;
    }

    /// Return accumulated non-fatal diagnostics (warnings/lints) from the last check.
    pub fn warnings(&self) -> &[CompileError] {
        &self.warnings
    }

    /// Return accumulated type information for reuse by later stages (lowering/codegen).
    pub fn type_info(&self) -> &TypeCheckInfo {
        &self.type_info
    }

    /// Hand the accumulated type information to a later stage once checking is over. The opt-in Body IR baseline also
    /// retains indexed immutable owner references; checking and settlement are never repeated.
    pub fn into_type_info(mut self) -> TypeCheckInfo {
        if std::env::var_os("INCAN_BODY_IR_FREE_BASELINE").is_some() {
            self.type_info.body_ir_baseline_inputs = Some(self.body_ir_baseline_inputs());
        }
        self.type_info
    }

    /// Return the checked source path for one active import-derived binding.
    pub fn import_binding_path(&self, local_name: &str) -> Option<&[String]> {
        self.symbols.import_binding_path(local_name)
    }
}

impl Default for TypeChecker {
    /// Initialize the same builtin scope and empty field groups as [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}
