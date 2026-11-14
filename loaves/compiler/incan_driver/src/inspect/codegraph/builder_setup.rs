//! Codegraph builder construction, checked facts, and tolerant file and parsed module collection.

use super::*;

impl<'analysis> CodegraphBuilder<'analysis> {
    /// Create a record builder for one strict or tolerant export.
    pub(super) fn new(root_path: &Path, package: Option<CodegraphPackage>, allow_errors: bool) -> Self {
        Self {
            records: Vec::new(),
            diagnostics: Vec::new(),
            declaration_facts: BTreeMap::new(),
            stable_contexts_by_identity: BTreeMap::new(),
            file_ids: BTreeMap::new(),
            module_ids: BTreeSet::new(),
            namespace_ids: BTreeSet::new(),
            mode: if allow_errors {
                CodegraphMode::AllowErrors
            } else {
                CodegraphMode::Strict
            },
            root_path: path_string(root_path),
            root_path_buf: root_path.to_path_buf(),
            package,
            semantic_contexts: Vec::new(),
            next_body_fact_index: 0,
            semantic_snapshots_by_path: Cow::Owned(BTreeMap::new()),
            registry_metadata_by_path: BTreeMap::new(),
            capabilities_by_path: BTreeMap::new(),
            c_abi_by_path: BTreeMap::new(),
            canonical_target_ids: BTreeMap::new(),
            reference_sites: BTreeSet::new(),
            call_ids: BTreeMap::new(),
        }
    }

    /// Attach checker-owned source identities and independent executable digests to the graph projection.
    pub(super) fn set_checked_declaration_facts(
        &mut self,
        facts: BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts>,
    ) {
        self.declaration_facts = facts;
    }

    /// Project one checked identity into its exported edit-stable form, carrying a signature when one was proved.
    pub(super) fn stable_identity_for(
        &self,
        canonical: Option<&CanonicalSymbolId>,
    ) -> Option<CodegraphStableDeclarationId> {
        let canonical = canonical?;
        if let Some(identity) = self
            .declaration_facts
            .get(canonical)
            .and_then(|facts| facts.source_identity.as_ref())
        {
            return Some(codegraph_stable_identity_from_semantic(identity));
        }
        let context = self
            .stable_contexts_by_identity
            .get(canonical)
            .and_then(|(owner, binding_ordinal)| {
                let owner = self
                    .declaration_facts
                    .get(owner)
                    .and_then(|facts| facts.source_identity.clone())
                    .or_else(|| StableDeclarationId::from_canonical(owner, None, None))?;
                Some(StableDeclarationContext {
                    owner,
                    binding_ordinal: *binding_ordinal,
                })
            });
        codegraph_stable_identity(canonical, context)
    }

    /// Borrow the compilation's checked facts, or own the snapshots returned by a standalone inspection pass.
    /// Graph construction never mutates these facts or copies their module-wide identity tables.
    pub(super) fn set_semantic_snapshots(
        &mut self,
        semantic_snapshots_by_path: Cow<'analysis, BTreeMap<PathBuf, SemanticModuleSnapshot>>,
    ) {
        self.stable_contexts_by_identity.clear();
        for snapshot in semantic_snapshots_by_path.values() {
            for subject in snapshot.facts.subjects() {
                let Some(identity) = snapshot.facts.symbol_identities_for(subject).next() else {
                    continue;
                };
                let Some(context) = snapshot.facts.stable_declaration_contexts_for(subject).next() else {
                    continue;
                };
                self.stable_contexts_by_identity
                    .insert(identity.clone(), (context.owner.clone(), context.binding_ordinal));
            }
        }
        self.semantic_snapshots_by_path = semantic_snapshots_by_path;
    }

    /// Attach the checked registry projection produced from the same session-owned typechecking pass.
    pub(super) fn set_registry_metadata(
        &mut self,
        registry_metadata_by_path: BTreeMap<PathBuf, CheckedRegistryMetadataModule>,
    ) {
        self.registry_metadata_by_path = registry_metadata_by_path;
    }

    /// Attach the compilation's checked RFC 104 capability declarations alongside the other graph facts.
    pub(super) fn set_capabilities(&mut self, capabilities_by_path: BTreeMap<PathBuf, Vec<CapabilityDeclarationInfo>>) {
        self.capabilities_by_path = capabilities_by_path;
    }

    /// Attach successful checked C ABI artifacts from the same compilation session as the other graph facts.
    pub(super) fn set_c_abi_artifacts(&mut self, c_abi_by_path: BTreeMap<PathBuf, CAbiInteropArtifacts>) {
        self.c_abi_by_path = c_abi_by_path;
    }

    /// Precompute declaration record ids from compiler-owned canonical identities before body facts are emitted.
    ///
    /// A target id is only an export-local linkage projection. The canonical identity is the key and remains present
    /// on a reference even when its declaration lies outside this export.
    pub(super) fn seed_canonical_target_ids(&mut self, modules: &[ParsedModule]) {
        for module in modules {
            let Some(snapshot) = self.semantic_snapshots_by_path.get(&module.file_path) else {
                continue;
            };
            for (index, declaration) in module.ast.declarations.iter().enumerate() {
                let Some(summary) = declaration_summary(&declaration.node) else {
                    continue;
                };
                let declaration_id = declaration_id(module, declaration, index);
                let canonical = snapshot.hir.declarations.iter().find_map(|checked| {
                    (checked.span == declaration.span && checked.name.as_deref() == Some(summary.name.as_str()))
                        .then(|| checked.canonical.clone())
                        .flatten()
                });
                if let Some(canonical) = canonical
                    && canonical.declaration_span == declaration.span
                    && match &canonical.origin {
                        SymbolOrigin::Module(path) => path == &module.path_segments,
                        SymbolOrigin::Package { library, module_path } => {
                            module_path == &module.path_segments
                                && self.package.as_ref().and_then(|package| package.name.as_deref())
                                    == Some(library.as_str())
                        }
                        _ => false,
                    }
                {
                    self.canonical_target_ids.insert(canonical, declaration_id);
                }
            }
        }
    }

    /// Recover as much source structure as possible after the ordinary entrypoint collection path failed.
    pub(super) fn collect_tolerant_failure(
        &mut self,
        path: &Path,
        failure: CliDiagnosticFailure,
        session: &CompilationSession,
    ) -> CodegraphResult<()> {
        let before = self.diagnostics.len();
        if path.is_file() {
            self.collect_tolerant_file_with_session(path, session)?;
        }
        if self.diagnostics.len() == before {
            self.collect_diagnostics(stable_diagnostics(&failure));
        }
        Ok(())
    }

    /// Parse one file with a caller-provided project-aware session, avoiding repeated manifest/vocab discovery.
    pub(super) fn collect_tolerant_file_with_session(
        &mut self,
        path: &Path,
        session: &CompilationSession,
    ) -> CodegraphResult<()> {
        let source = read_source(&path.to_string_lossy())?;
        match session.parse_source_for_collection(path, &source) {
            Ok(ast) => {
                let file_id = self.ensure_file_record(path, &source, false);
                let source_positions = diagnostics::SourcePositionIndex::new(&source);
                let module = ParsedModule {
                    name: module_name_for_file(path),
                    path_segments: self.fallback_module_segments(path),
                    file_path: path.to_path_buf(),
                    source_positions,
                    source,
                    ast,
                    gated_exports: Vec::new(),
                };
                self.collect_module_records_with_degraded(&module, &file_id, true);
            }
            Err(errors) => {
                self.ensure_file_record(path, &source, true);
                let diagnostics = errors
                    .iter()
                    .map(|error| {
                        diagnostics::stable_diagnostic(
                            &path.to_string_lossy(),
                            &source,
                            error,
                            diagnostics::DiagnosticPhase::Parse,
                        )
                    })
                    .collect::<Vec<_>>();
                self.collect_diagnostics(diagnostics);
            }
        }
        Ok(())
    }

    /// Add graph records for one module that was already parsed by the canonical collection path.
    pub(super) fn collect_parsed_module(&mut self, module: &ParsedModule, diagnostics: Vec<StableDiagnostic>) {
        self.collect_parsed_module_with_degraded(module, diagnostics, false);
    }

    /// Add parsed module records, optionally marking the whole record set degraded because checked facts were lost.
    pub(super) fn collect_parsed_module_with_degraded(
        &mut self,
        module: &ParsedModule,
        diagnostics: Vec<StableDiagnostic>,
        force_degraded: bool,
    ) {
        let degraded = force_degraded || !diagnostics.is_empty();
        let file_id = self.ensure_file_record(&module.file_path, &module.source, degraded);
        self.collect_module_records_with_degraded(module, &file_id, degraded);
    }

    /// Add file, module, and module-containment facts before descending into declarations.
    pub(super) fn collect_module_records_with_degraded(
        &mut self,
        module: &ParsedModule,
        file_id: &str,
        degraded: bool,
    ) {
        let module_id = module_id(module);
        if self.module_ids.insert(module_id.clone()) {
            let module_span = source_span(module, Span::new(0, module.source.len()));
            self.records.push(CodegraphRecord::Module(CodegraphModuleRecord {
                id: module_id.clone(),
                language: CodegraphLanguage::Incan,
                file_id: file_id.to_string(),
                module_path: module.path_segments.clone(),
                namespace_path: enclosing_namespace(&module.path_segments).to_vec(),
                internal: !is_namespace_root(&module.path_segments),
                name: module.name.clone(),
                span: Some(module_span),
                provenance: CodegraphProvenance::Syntax,
                degraded,
            }));
            // RFC 106: the namespace is the graph's unit of reachable surface, and a module is a detail of it
            // unless it is one itself. Emitted once per namespace, with a `contains` edge per module beneath it,
            // so a consumer can read the surface without reconstructing it from `namespace_path`.
            let namespace_path = enclosing_namespace(&module.path_segments).to_vec();
            let namespace_id = namespace_id(&namespace_path);
            if self.namespace_ids.insert(namespace_id.clone()) {
                self.records.push(CodegraphRecord::Namespace(CodegraphNamespaceRecord {
                    id: namespace_id.clone(),
                    language: CodegraphLanguage::Incan,
                    name: namespace_path.last().cloned().unwrap_or_else(|| "crate".to_string()),
                    namespace_path,
                    provenance: CodegraphProvenance::Syntax,
                    degraded,
                }));
            }
            self.records
                .push(CodegraphRecord::Containment(CodegraphContainmentRecord {
                    id: format!("contains:{namespace_id}:{module_id}"),
                    language: CodegraphLanguage::Incan,
                    parent_id: namespace_id,
                    child_id: module_id.clone(),
                    kind: "namespace_contains_module".to_string(),
                    span: None,
                    provenance: CodegraphProvenance::Source,
                    degraded,
                }));
            self.records
                .push(CodegraphRecord::Containment(CodegraphContainmentRecord {
                    id: format!("contains:{file_id}:{module_id}"),
                    language: CodegraphLanguage::Incan,
                    parent_id: file_id.to_string(),
                    child_id: module_id.clone(),
                    kind: "file_contains_module".to_string(),
                    span: None,
                    provenance: CodegraphProvenance::Source,
                    degraded,
                }));
        }
        self.collect_program_records(module, &module_id, degraded);
        if !degraded {
            self.collect_remaining_checked_references(module, &module_id);
            self.collect_checked_registry_records(module, &module_id);
            self.collect_checked_capability_records(module, &module_id);
            self.collect_checked_c_binding_records(module, &module_id);
        }
    }
}
