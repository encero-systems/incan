//! Dependency module imports, interface predeclaration, and import-aware checking entry points.

use super::*;

impl TypeChecker {
    /// Import symbols from another module's AST into the symbol table.
    ///
    /// Call this before [`check_program`](Self::check_program) so the main module can reference types and functions
    /// defined in dependencies.
    ///
    /// ## Parameters
    ///
    /// - `module_ast`: parsed AST of the dependency module.
    /// - `module_name`: dependency cache key whose registered path owns the collected declarations.
    pub fn import_module(&mut self, module_ast: &Program, module_name: &str) {
        let previous_surface_context = self.surface_context.clone();
        let previous_module_function_symbols = std::mem::take(&mut self.current_module_function_symbols);
        let previous_module_path = self.current_module_path.clone();
        let previous_type_aliases = self.type_aliases.clone();
        let dependency_module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
        self.set_current_module_path(Some(dependency_module_path));
        let previous_nominal_declarations = self.enter_nominal_declaring_module(module_ast);
        self.surface_context = SurfaceContext::from_program(module_ast);
        self.symbols.begin_dependency_interface_bindings();
        self.seed_dependency_interface_bindings(module_ast, true);

        // Imports are lexical rather than exported, but public declaration signatures may depend on their exact type
        // metadata. Collect them temporarily, then restore only the names they introduced after the public surface has
        // been cached.
        self.begin_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Import(_)) {
                self.collect_declaration(decl);
            }
        }
        let (imported_symbol_bindings, imported_type_alias_bindings) =
            self.finish_dependency_import_binding_transaction();
        // Collect only public declarations from the imported module.
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.resolve_forward_type_references(&module_ast.declarations);
        self.record_local_nominal_type_param_bounds(&module_ast.declarations);
        self.resolve_pending_trait_supertraits();
        self.settle_method_scopes(&module_ast.declarations);
        self.infer_hash_key_type_params(&module_ast.declarations, false);
        self.collect_imported_method_ownership_headers(module_ast);
        self.register_dependency_derivable_metadata(module_name, module_ast);
        self.cache_dependency_direct_member_symbols(module_name, module_ast, true);
        self.cache_dependency_registry_definitions(module_name, module_ast, true);
        self.cache_dependency_registry_consts(module_ast);
        self.cache_dependency_member_symbols(module_name, module_ast, true);
        let owned_names = module_ast
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(declaration_name)
            .collect::<HashSet<_>>();
        let owned_type_alias_names = module_ast
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(|decl| match &decl.node {
                Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        self.restore_dependency_import_bindings(
            imported_symbol_bindings,
            imported_type_alias_bindings,
            &owned_names,
            &owned_type_alias_names,
        );
        self.dependency_semantics_pending = true;
        self.surface_context = previous_surface_context;
        self.current_module_function_symbols = previous_module_function_symbols;
        self.symbols.finish_dependency_interface_bindings();
        self.type_aliases = previous_type_aliases;
        self.restore_nominal_declaring_module(previous_nominal_declarations);
        self.set_current_module_path(previous_module_path);
    }

    /// Import all symbols from another module's AST into the symbol table.
    ///
    /// This is used for internal compiler passes that need type information across modules without enforcing `pub`
    /// visibility (e.g. codegen-only validation for dependencies).
    pub fn import_module_all(&mut self, module_ast: &Program, module_name: &str) {
        let previous_surface_context = self.surface_context.clone();
        let previous_module_function_symbols = std::mem::take(&mut self.current_module_function_symbols);
        let previous_module_path = self.current_module_path.clone();
        let previous_type_aliases = self.type_aliases.clone();
        let dependency_module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
        self.set_current_module_path(Some(dependency_module_path));
        let previous_nominal_declarations = self.enter_nominal_declaring_module(module_ast);
        self.surface_context = SurfaceContext::from_program(module_ast);
        self.symbols.begin_dependency_interface_bindings();
        self.seed_dependency_interface_bindings(module_ast, false);

        // Dependency imports must be available while its declarations are collected so public signatures retain exact
        // provider-owned type metadata. They still belong only to the dependency's lexical scope, so remember which
        // root bindings they introduce and restore those names before the consumer program is collected.
        self.begin_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Import(_)) {
                self.collect_declaration(decl);
            }
        }
        let (imported_symbol_bindings, imported_type_alias_bindings) =
            self.finish_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if !matches!(
                decl.node,
                Declaration::Import(_) | Declaration::Alias(_) | Declaration::Partial(_)
            ) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.resolve_forward_type_references(&module_ast.declarations);
        self.record_local_nominal_type_param_bounds(&module_ast.declarations);
        self.resolve_pending_trait_supertraits();
        self.settle_method_scopes(&module_ast.declarations);
        self.infer_hash_key_type_params(&module_ast.declarations, false);
        self.collect_imported_method_ownership_headers(module_ast);
        self.register_dependency_derivable_metadata(module_name, module_ast);
        self.cache_dependency_direct_member_symbols(module_name, module_ast, false);
        self.cache_dependency_registry_definitions(module_name, module_ast, false);
        self.cache_dependency_registry_consts(module_ast);
        self.cache_dependency_member_symbols(module_name, module_ast, false);
        let owned_names = module_ast
            .declarations
            .iter()
            .filter_map(declaration_name)
            .collect::<HashSet<_>>();
        let owned_type_alias_names = module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        self.restore_dependency_import_bindings(
            imported_symbol_bindings,
            imported_type_alias_bindings,
            &owned_names,
            &owned_type_alias_names,
        );
        self.dependency_semantics_pending = true;
        self.surface_context = previous_surface_context;
        self.current_module_function_symbols = previous_module_function_symbols;
        self.symbols.finish_dependency_interface_bindings();
        self.type_aliases = previous_type_aliases;
        self.restore_nominal_declaring_module(previous_nominal_declarations);
        self.set_current_module_path(previous_module_path);
    }

    /// Start a narrow transaction for source-visible bindings mutated while dependency imports are collected.
    pub(super) fn begin_dependency_import_binding_transaction(&mut self) {
        self.symbols.begin_current_scope_binding_transaction();
        debug_assert!(self.dependency_import_type_alias_transaction.is_none());
        self.dependency_import_type_alias_transaction = Some(HashMap::new());
    }

    /// Finish the dependency import transaction and return the previous values of only the bindings it changed.
    pub(super) fn finish_dependency_import_binding_transaction(
        &mut self,
    ) -> (
        HashMap<String, Option<SymbolId>>,
        HashMap<String, Option<TypeAliasTarget>>,
    ) {
        (
            self.symbols.finish_current_scope_binding_transaction(),
            self.dependency_import_type_alias_transaction.take().unwrap_or_default(),
        )
    }

    /// Record one type-alias binding before a dependency import changes it.
    pub(super) fn record_dependency_import_type_alias_before_change(&mut self, name: &str) {
        let previous = self.type_aliases.get(name).cloned();
        if let Some(transaction) = &mut self.dependency_import_type_alias_transaction {
            transaction.entry(name.to_string()).or_insert(previous);
        }
    }

    /// Remove dependency-lexical imports from the consumer scope while retaining dependency-owned declarations.
    pub(super) fn restore_dependency_import_bindings(
        &mut self,
        imported_symbol_bindings: HashMap<String, Option<SymbolId>>,
        imported_type_alias_bindings: HashMap<String, Option<TypeAliasTarget>>,
        owned_names: &HashSet<&str>,
        owned_type_alias_names: &HashSet<&str>,
    ) {
        for (name, previous_binding) in imported_symbol_bindings {
            if owned_names.contains(name.as_str()) {
                continue;
            }
            self.symbols.restore_current_scope_binding(&name, previous_binding);
        }
        for (name, previous_alias) in imported_type_alias_bindings {
            if owned_type_alias_names.contains(name.as_str()) {
                continue;
            }
            if let Some(previous_alias) = previous_alias {
                self.type_aliases.insert(name, previous_alias);
            } else {
                self.type_aliases.remove(&name);
            }
        }
    }

    /// Rebuild exact dependency-member caches after all dependency modules have been collected.
    ///
    /// Public facade modules may re-export items from sibling modules. Importing a facade before the underlying
    /// sibling has been fully collected can leave the facade cache with a placeholder type shell. A final cache pass
    /// after dependency collection lets re-exports resolve through the now-complete target module metadata.
    pub(super) fn refresh_dependency_member_symbols(&mut self, dependencies: &[(&str, &Program)], public_only: bool) {
        for (name, dep_ast) in dependencies {
            self.cache_dependency_member_symbols(name, dep_ast, public_only);
        }
        self.cache_dependency_adoption_traits();
    }

    /// Predeclare exact per-module dependency interface members before collecting imported declarations.
    ///
    /// Cross-module public signatures may reference types and traits from modules collected later, including cyclic
    /// interfaces such as `session -> dataset -> session`. Shells live in the module-keyed import-resolution cache,
    /// never in the consumer's lexical symbol table, so sibling modules may export the same spelling safely.
    pub(super) fn predeclare_dependency_interfaces(&mut self, dependencies: &[(&str, &Program)], public_only: bool) {
        for (module_name, dep_ast) in dependencies {
            let members = self
                .dependency_member_symbols
                .entry((*module_name).to_string())
                .or_default();
            for decl in &dep_ast.declarations {
                if public_only && !is_public_decl(decl) {
                    continue;
                }
                if let Some(symbol) = Self::dependency_interface_shell(decl) {
                    members.entry(symbol.name).or_insert(symbol.kind);
                }
            }
        }
    }

    /// Seed one dependency's temporary interface lookup view from its own declaration shells.
    pub(super) fn seed_dependency_interface_bindings(&mut self, module_ast: &Program, public_only: bool) {
        for decl in &module_ast.declarations {
            if public_only && !is_public_decl(decl) {
                continue;
            }
            if let Some(symbol) = Self::dependency_interface_shell(decl) {
                self.symbols.define_import_binding(symbol, None);
            }
        }
    }

    /// Build a minimal placeholder for one type-like dependency declaration.
    ///
    /// The subsequent `import_module*` pass replaces the temporary lookup shell with full collected metadata. The
    /// module-keyed copy remains available to imports from modules that have not yet been collected.
    pub(super) fn dependency_interface_shell(decl: &Spanned<Declaration>) -> Option<Symbol> {
        match &decl.node {
            Declaration::Model(model) => Some(Symbol {
                name: model.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                    type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: HashMap::new(),
                    field_order: Vec::new(),
                    properties: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                    method_aliases: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Class(class) => Some(Symbol {
                name: class.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                    type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    extends: class.extends.clone(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: HashMap::new(),
                    field_defaults: Box::new(HashMap::new()),
                    field_default_metadata: Box::new(HashMap::new()),
                    field_provider_libraries: Box::new(HashMap::new()),
                    field_order: Vec::new(),
                    properties: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                    method_aliases: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Trait(tr) => Some(Symbol {
                name: tr.name.clone(),
                kind: SymbolKind::Trait(TraitInfo {
                    type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    methods: HashMap::new(),
                    method_aliases: HashMap::new(),
                    properties: HashMap::new(),
                    requires: Vec::new(),
                    supertraits: Vec::new(),
                }),
                span: decl.span,
                scope: 0,
            }),
            Declaration::TypeAlias(alias) => Some(Symbol {
                name: alias.name.clone(),
                kind: SymbolKind::Type(TypeInfo::TypeAlias),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Newtype(nt) => Some(Symbol {
                name: nt.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                    type_params: nt.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    is_rusttype: nt.is_rusttype,
                    has_interop: !nt.interop_edges.is_empty(),
                    underlying: ResolvedType::Unknown,
                    constraints: Vec::new(),
                    implicit_coercion_enabled: true,
                    method_rebindings: HashMap::new(),
                    traits: nt.traits.iter().map(|trait_ref| trait_ref.node.name.clone()).collect(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    method_aliases: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Enum(en) => Some(Symbol {
                name: en.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                    type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                    trait_adoptions: Vec::new(),
                    variants: en.variants.iter().map(|v| v.node.name.clone()).collect(),
                    variant_identities: HashMap::new(),
                    variant_fields: HashMap::new(),
                    variant_aliases: en
                        .variant_aliases
                        .iter()
                        .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
                        .collect(),
                    value_enum: None,
                    derives: Vec::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            _ => None,
        }
    }

    /// Check a program that may have dependencies on other modules.
    ///
    /// Convenience wrapper that calls [`import_module`](Self::import_module) for each dependency, then
    /// [`check_program`](Self::check_program).
    ///
    /// ## Parameters
    ///
    /// - `program`: parsed AST of the main module.
    /// - `dependencies`: list of `(module_name, ast)` pairs to import first.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len(), dep_count = dependencies.len()))]
    pub fn check_with_imports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        let dependency_exports = dependencies
            .iter()
            .map(|(name, dep_ast)| {
                (
                    (*name).to_string(),
                    Arc::<[ExportedSymbol]>::from(exported_symbols(dep_ast)),
                )
            })
            .collect();
        self.check_with_imports_and_exports(program, dependencies, dependency_exports)
    }

    /// Check a module against dependency ASTs and their precomputed immutable public-export projections.
    ///
    /// `dependency_exports` must be keyed exactly like `dependencies`. The projection changes only where the export
    /// list is built: every visibility lookup still uses this checker's dependency closure and all remaining
    /// dependency-interface state is collected from the same ASTs in the same order.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len(), dep_count = dependencies.len()))]
    pub fn check_with_imports_and_exports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
        dependency_exports: DependencyExportInterfaces,
    ) -> Result<(), Vec<CompileError>> {
        let prepare = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::prepare_dependency_interfaces",
            declarations = program.declarations.len(),
            dependencies = dependencies.len(),
            public_only = true,
        )
        .entered();
        self.dependency_semantics_pending = false;
        self.dependency_method_input_contracts.clear();
        self.hash_key_inference.type_params.clear();
        self.transitive_pub_types.clear();
        self.public_library_type_identities.clear();
        self.transitive_pub_traits.clear();
        self.cached_pub_libraries.clear();
        self.foreign_pub_type_remappings.clear();
        self.pub_library_declaration_scopes.clear();
        self.dependency_exports = dependency_exports;
        self.dependency_member_symbols.clear();
        self.dependency_member_type_aliases.clear();
        self.dependency_registry_definitions.clear();
        self.dependency_registry_consts.clear();
        self.dependency_member_partial_projections.clear();
        self.dependency_direct_member_symbols.clear();
        self.dependency_direct_member_type_aliases.clear();
        self.dependency_direct_member_identities.clear();
        self.dependency_member_reexports.clear();
        self.body_ir_dependency_pub_reexports.clear();
        self.dependency_direct_member_partial_projections.clear();
        self.dependency_derivable_modules.clear();
        self.dependency_derivable_trait_identities.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.record_nominal_declaring_modules(program, dependencies);
        if let Some(interfaces) = self.take_exact_dependency_interfaces(dependencies) {
            self.install_dependency_interfaces_directly(&interfaces);
            drop(prepare);
        } else {
            self.predeclare_dependency_interfaces(dependencies, true);
            drop(prepare);
            // First: import all dependencies
            {
                let _trace = tracing::trace_span!(
                    target: "incan::invocation",
                    "incan_frontend::import_dependency_interfaces",
                    dependencies = dependencies.len(),
                    public_only = true,
                )
                .entered();
                for (name, dep_ast) in dependencies {
                    self.import_module(dep_ast, name);
                }
            }
            {
                let _trace = tracing::trace_span!(
                    target: "incan::invocation",
                    "incan_frontend::refresh_dependency_interfaces",
                    dependencies = dependencies.len(),
                    public_only = true,
                )
                .entered();
                self.refresh_dependency_member_symbols(dependencies, true);
            }
        }

        // Then check the main program
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::check_program",
            declarations = program.declarations.len(),
        )
        .entered();
        self.check_program_with_dependency_semantics(program)
    }

    /// Check one module of a standard package against its sibling modules' complete interfaces.
    ///
    /// A standard package checks its complete source before publishing its public checked facade, so the private
    /// declarations of the modules it imports stay in scope for their signatures. Its imports still bind only what the
    /// source module exports: a missing or private name is refused, the same import check every other module gets
    /// (#2595).
    pub fn check_standard_package_with_imports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        let dependency_exports = dependencies
            .iter()
            .map(|(name, dep_ast)| {
                (
                    (*name).to_string(),
                    Arc::<[ExportedSymbol]>::from(exported_symbols(dep_ast)),
                )
            })
            .collect();
        self.check_standard_package_with_imports_and_exports(program, dependencies, dependency_exports)
    }

    /// Check one standard-package module against sibling ASTs and their precomputed immutable public exports.
    ///
    /// Complete sibling interfaces still include private declarations. The shared projection is used only by the
    /// existing source-import visibility check, which has always admitted public exports alone.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len(), dep_count = dependencies.len()))]
    pub fn check_standard_package_with_imports_and_exports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
        dependency_exports: DependencyExportInterfaces,
    ) -> Result<(), Vec<CompileError>> {
        let prepare = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::prepare_dependency_interfaces",
            declarations = program.declarations.len(),
            dependencies = dependencies.len(),
            public_only = false,
        )
        .entered();
        self.dependency_semantics_pending = false;
        self.dependency_method_input_contracts.clear();
        self.hash_key_inference.type_params.clear();
        self.transitive_pub_types.clear();
        self.public_library_type_identities.clear();
        self.transitive_pub_traits.clear();
        self.cached_pub_libraries.clear();
        self.foreign_pub_type_remappings.clear();
        self.pub_library_declaration_scopes.clear();
        self.dependency_exports = dependency_exports;
        self.dependency_member_symbols.clear();
        self.dependency_member_type_aliases.clear();
        self.dependency_registry_definitions.clear();
        self.dependency_registry_consts.clear();
        self.dependency_member_partial_projections.clear();
        self.dependency_direct_member_symbols.clear();
        self.dependency_direct_member_type_aliases.clear();
        self.dependency_direct_member_identities.clear();
        self.dependency_member_reexports.clear();
        self.body_ir_dependency_pub_reexports.clear();
        self.dependency_direct_member_partial_projections.clear();
        self.dependency_derivable_modules.clear();
        self.dependency_derivable_trait_identities.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.record_nominal_declaring_modules(program, dependencies);
        if let Some(interfaces) = self.take_exact_dependency_interfaces(dependencies) {
            self.install_dependency_interfaces_directly(&interfaces);
            drop(prepare);
        } else {
            self.predeclare_dependency_interfaces(dependencies, false);
            drop(prepare);
            {
                let _trace = tracing::trace_span!(
                    target: "incan::invocation",
                    "incan_frontend::import_dependency_interfaces",
                    dependencies = dependencies.len(),
                    public_only = false,
                )
                .entered();
                for (name, dep_ast) in dependencies {
                    self.import_module_all(dep_ast, name);
                }
            }
            {
                let _trace = tracing::trace_span!(
                    target: "incan::invocation",
                    "incan_frontend::refresh_dependency_interfaces",
                    dependencies = dependencies.len(),
                    public_only = false,
                )
                .entered();
                self.refresh_dependency_member_symbols(dependencies, false);
            }
        }
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::check_program",
            declarations = program.declarations.len(),
        )
        .entered();
        self.check_program_with_dependency_semantics(program)
    }
}
