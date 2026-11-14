//! Rust item references, checked capability, registry and C binding records, and program records.

use super::*;

impl<'analysis> CodegraphBuilder<'analysis> {
    /// Record each Rust item an import brings into scope as a reference fact of its own.
    ///
    /// Before this, a `from rust::… import …` produced one import record and nothing else, so the graph could say
    /// a module imported *from* a crate but not *what* it reached. RFC 106 makes that question answerable with the
    /// vocabulary already here rather than with a second one: a Rust declaration is a `Declaration` with
    /// `language: rust`, a reference to it is a `Reference`, and the reserved `rust_item` and `uses_rust_item`
    /// kinds are retired because they encode the distinction the `language` field exists to dissolve.
    ///
    /// # Why the target is `None` and that is not a gap
    ///
    /// The declaration these point at is not emitted here. It cannot be: naming a Rust item is not the same as
    /// locating one, and inventing a module and file node for a crate whose source this pass never opens would put
    /// a placeholder where a fact belongs. Rust declarations arrive through the native driver's rustc metadata
    /// export; until that export supplies the declaration, `target_id` says unresolved, which RFC 106 binds
    /// consumers to read as affected rather than as absent.
    ///
    /// The identity is not guessed either. Where the typechecker resolved the binding, the import's own checked
    /// projection carries it and the reference is `Checked`; where it did not, the reference is `Syntax` and the
    /// name is all the graph claims.
    ///
    /// Every argument is one import declaration's own fact, so bundling them into a struct for a single call site
    /// would move the parameter list rather than shorten it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn collect_rust_item_references(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        import_id: &str,
        import: &ImportDecl,
        span: Span,
        bindings: &[CodegraphImportBinding],
        degraded: bool,
    ) {
        let reached = match &import.kind {
            ImportKind::RustCrate { crate_name, path, .. } => {
                vec![(
                    rust_path_display(crate_name, path),
                    "imported_module",
                    import.alias.clone(),
                )]
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } => items
                .iter()
                .map(|item| {
                    let mut segments = path.clone();
                    segments.push(item.name.clone());
                    (
                        rust_path_display(crate_name, &segments),
                        "imported_item",
                        item.alias.clone(),
                    )
                })
                .collect(),
            ImportKind::Module(_)
            | ImportKind::From { .. }
            | ImportKind::PubLibrary { .. }
            | ImportKind::PubFrom { .. }
            | ImportKind::Python(_) => Vec::new(),
        };

        for (path, kind, alias) in reached {
            // The binding carries the checked identity under the name the importing module sees, which is the
            // alias where one was written and the item's own name otherwise.
            let local_name = alias.unwrap_or_else(|| {
                path.rsplit("::")
                    .next()
                    .map(str::to_string)
                    .unwrap_or_else(|| path.clone())
            });
            let binding = bindings.iter().find(|binding| binding.local_name == local_name);
            let canonical_identity = binding.and_then(|binding| binding.canonical_identity.clone());
            let stable_identity = binding.and_then(|binding| binding.stable_identity.clone());
            let provenance = if canonical_identity.is_some() {
                CodegraphProvenance::Checked
            } else {
                CodegraphProvenance::Syntax
            };
            let reference_id = self.next_body_fact_id("rust_reference", module, span, &path);
            self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
                id: reference_id.clone(),
                language: CodegraphLanguage::Rust,
                module_id: module_id.to_string(),
                owner_id: None,
                name: path,
                kind: kind.to_string(),
                target_id: None,
                canonical_identity,
                canonical_owner: None,
                stable_identity,
                span: Some(source_span(module, span)),
                provenance,
                degraded,
            }));
            // The import statement is what reaches the item, so the edge hangs off the import rather than off the
            // module. `owner_id` is left unset above because it names a declaration a navigator can open, and an
            // import record is not one.
            self.records
                .push(CodegraphRecord::Containment(CodegraphContainmentRecord {
                    id: format!("contains:{import_id}:{reference_id}"),
                    language: CodegraphLanguage::Rust,
                    parent_id: import_id.to_string(),
                    child_id: reference_id,
                    kind: "import_contains_reference".to_string(),
                    span: None,
                    provenance: CodegraphProvenance::Source,
                    degraded,
                }));
        }
    }

    /// Include checked type, layout and default sites that the source expression walker does not visit.
    ///
    /// The snapshot supplies both anchors and identities. These records do not infer a dependency from type spelling
    /// or reconstruct a source owner; they project the same relationship query used by executable requirements.
    pub(super) fn collect_remaining_checked_references(&mut self, module: &ParsedModule, module_id: &str) {
        let sites = self
            .semantic_snapshots_by_path
            .get(&module.file_path)
            .into_iter()
            .flat_map(|snapshot| snapshot.facts.checked_reference_sites())
            .map(|(span, reference)| (span, reference.target.clone(), reference.owner.cloned()))
            .collect::<Vec<_>>();
        for (span, target, owner) in sites {
            if self
                .reference_sites
                .contains(&(module_id.to_owned(), span, Some(target.clone())))
            {
                continue;
            }
            let name = target.declaration_name.clone();
            // `kind` is the syntactic form a reference took -- identifier, field, `self`, surface path -- and the
            // separate `provenance` field already records that a fact came from checked analysis. Naming this one
            // "checked" put a provenance answer in the form axis, so a consumer filtering by form saw a value that
            // is not one. These are references to a type at an expression's span, so they are typed as such.
            self.push_reference_with_checked(
                module,
                module_id,
                None,
                &name,
                "type",
                span,
                false,
                Some((target, owner)),
            );
        }
    }

    /// Project RFC 104 capability declarations from the session's checked facts.
    ///
    /// RFC 104 requires its authority contract to be inspectable without executing source. Codegraph never re-reads a
    /// `capability` block or re-resolves a `requires` reference to reconstruct that contract; it publishes what the
    /// typechecker already proved at the declaration site.
    pub(super) fn collect_checked_capability_records(&mut self, module: &ParsedModule, module_id: &str) {
        let Some(declarations) = self.capabilities_by_path.get(&module.file_path).cloned() else {
            return;
        };
        for declaration in declarations {
            let declaration_span = declaration.capability.declaration_span;
            let span = Some(source_span(
                module,
                Span::new(declaration_span.start, declaration_span.end),
            ));
            self.records
                .push(CodegraphRecord::Capability(CodegraphCapabilityRecord {
                    id: capability_record_id(&declaration.capability, declaration_span.start),
                    language: CodegraphLanguage::Incan,
                    module_id: module_id.to_string(),
                    capability_identity: codegraph_canonical_identity(&declaration.capability),
                    name: declaration.capability.declaration_name.clone(),
                    public: declaration.is_public,
                    description: declaration.description.as_deref().map(visible_source_text),
                    scope: declaration
                        .scope
                        .iter()
                        .map(|(name, ty)| CodegraphCapabilityScopeDimension {
                            name: name.clone(),
                            ty: ty.clone(),
                        })
                        .collect(),
                    requires: declaration.requires.iter().map(codegraph_canonical_identity).collect(),
                    span,
                    provenance: CodegraphProvenance::Checked,
                    degraded: false,
                }));
        }
    }

    /// Project RFC 113 registry facts from the session-owned checked metadata artifact.
    ///
    /// Inspection, package publication, and codegraph all consume this same portable projection. Codegraph never
    /// reparses decorators or performs a command-local typecheck to reconstruct registry meaning.
    pub(super) fn collect_checked_registry_records(&mut self, module: &ParsedModule, module_id: &str) {
        let Some(metadata) = self.registry_metadata_by_path.get(&module.file_path) else {
            return;
        };
        for entry in &metadata.entries {
            self.records.push(CodegraphRecord::Registry(CodegraphRegistryRecord {
                id: registry_record_id(
                    &entry.registry_identity,
                    &entry.subject_identity,
                    entry.registration_anchor.start,
                ),
                language: CodegraphLanguage::Incan,
                module_id: module_id.to_string(),
                registry_identity: entry.registry_identity.clone(),
                registry_public: entry.registry_public,
                key: checked_registry_value_json(&entry.key),
                descriptor: checked_registry_value_json(&entry.descriptor),
                subject_kind: checked_registry_subject_kind(entry.subject_kind).to_string(),
                subject_identity: entry.subject_identity.clone(),
                registration_span: source_span(
                    module,
                    Span::new(entry.registration_anchor.start, entry.registration_anchor.end),
                ),
                subject_span: source_span(module, Span::new(entry.subject_anchor.start, entry.subject_anchor.end)),
                provenance: CodegraphProvenance::Checked,
                reexport_paths: Vec::new(),
                degraded: false,
            }));
        }
    }

    /// Project checked C declaration contracts and explicit-unsafe raw calls from the shared typecheck artifacts.
    ///
    /// The generic codegraph walker still emits ordinary syntax-level `call` records. These records attach the
    /// compiler-owned ABI facts to those source spans rather than asking consumers to infer C interop from a spelling.
    /// Temporarily move the owned snapshot out of the builder while projecting records, then restore it for any
    /// repeated module collection; borrowing the builder's map across its record mutations would alias `self`.
    pub(super) fn collect_checked_c_binding_records(&mut self, module: &ParsedModule, module_id: &str) {
        let Some((path, c_abi)) = self.c_abi_by_path.remove_entry(&module.file_path) else {
            return;
        };
        self.collect_c_binding_declarations(module, module_id, &c_abi);
        self.collect_c_binding_raw_calls(module, module_id, &c_abi);
        self.collect_c_binding_facades(module, module_id, &c_abi);
        self.c_abi_by_path.insert(path, c_abi);
    }

    /// Emit checked C binding declarations and their containment records in the existing span/name order.
    /// The shared artifact snapshot remains owned by the caller across all three C-record phases.
    pub(super) fn collect_c_binding_declarations(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        c_abi: &CAbiInteropArtifacts,
    ) {
        let mut bindings = c_abi.bindings.values().collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            left.span
                .start
                .cmp(&right.span.start)
                .then_with(|| left.class_name.cmp(&right.class_name))
        });
        for descriptor in bindings {
            let binding_id = c_binding_record_id(module_id, &descriptor.class_name);
            let binding_identity = c_binding_descriptor_identity(&module.path_segments, descriptor);
            let Some(declaration_id) = c_binding_declaration_id(module, &descriptor.class_name) else {
                continue;
            };
            self.records.push(CodegraphRecord::CBinding(CodegraphCBindingRecord {
                id: binding_id.clone(),
                language: CodegraphLanguage::Incan,
                module_id: module_id.to_string(),
                declaration_id,
                name: descriptor.class_name.clone(),
                binding_identity: binding_identity.clone(),
                header: descriptor.header.clone(),
                system_library: descriptor.system_library.clone(),
                // A Clove link is carried by its configuration until the kernel's capability set names it.
                link_capability: if descriptor.configuration.is_empty() {
                    link_capability_as_str(descriptor.link_capability).to_string()
                } else {
                    "loaf_library".to_string()
                },
                resources: descriptor.resources.iter().map(c_binding_resource_record).collect(),
                symbols: descriptor.symbols.iter().map(c_binding_symbol_record).collect(),
                enums: descriptor.enums.iter().map(c_binding_enum_record).collect(),
                structs: descriptor.structs.iter().map(c_binding_struct_record).collect(),
                span: source_span(module, descriptor.span),
                provenance: CodegraphProvenance::Checked,
                degraded: false,
            }));
            self.records.push(CodegraphRecord::Containment(containment_record(
                module_id,
                &binding_id,
                "module_contains_c_binding",
                module,
                descriptor.span,
                false,
            )));
        }
    }

    /// Attach checked raw-call authority to already collected source call spans, preserving span/binding/symbol
    /// ordering and the refusal to invent a descriptor when the shared checked artifact has none.
    pub(super) fn collect_c_binding_raw_calls(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        c_abi: &CAbiInteropArtifacts,
    ) {
        let mut raw_calls = c_abi.raw_calls.iter().collect::<Vec<_>>();
        raw_calls.sort_by(|left, right| {
            left.span
                .start
                .cmp(&right.span.start)
                .then_with(|| left.span.end.cmp(&right.span.end))
                .then_with(|| left.binding.cmp(&right.binding))
                .then_with(|| left.symbol.cmp(&right.symbol))
        });
        for raw_call in raw_calls {
            let Some(descriptor) = c_abi.bindings.get(&raw_call.binding) else {
                continue;
            };
            let id = c_binding_call_record_id(module_id, &raw_call.binding, &raw_call.symbol, raw_call.span);
            self.records
                .push(CodegraphRecord::CBindingCall(CodegraphCBindingCallRecord {
                    id,
                    language: CodegraphLanguage::Incan,
                    module_id: module_id.to_string(),
                    call_id: self.call_record_id_at_span(module_id, raw_call.span),
                    binding_id: c_binding_record_id(module_id, &raw_call.binding),
                    binding_identity: c_binding_descriptor_identity(&module.path_segments, descriptor),
                    owner_declaration_id: raw_call
                        .owner
                        .as_ref()
                        .and_then(|owner| c_binding_raw_call_owner_declaration_id(module, owner)),
                    owner_visibility: raw_call
                        .owner
                        .as_ref()
                        .map(|owner| visibility_spelling(owner.visibility).to_string()),
                    binding: raw_call.binding.clone(),
                    symbol: raw_call.symbol.clone(),
                    unsafe_acknowledged: true,
                    span: source_span(module, raw_call.span),
                    provenance: CodegraphProvenance::Checked,
                    degraded: false,
                }));
        }
    }

    /// Emit facade-to-bridge records only when both declarations and checked raw-call coverage exist. Coverage IDs
    /// retain their existing sorting and deduplication before the facade record is published.
    pub(super) fn collect_c_binding_facades(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        c_abi: &CAbiInteropArtifacts,
    ) {
        let mut facades = c_abi.facades.iter().collect::<Vec<_>>();
        facades.sort_by(|left, right| {
            left.facade
                .declaration_span
                .start
                .cmp(&right.facade.declaration_span.start)
                .then_with(|| {
                    left.bridge
                        .declaration_span
                        .start
                        .cmp(&right.bridge.declaration_span.start)
                })
                .then_with(|| left.call_span.start.cmp(&right.call_span.start))
        });
        for facade in facades {
            let Some(facade_declaration_id) = c_binding_raw_call_owner_declaration_id(module, &facade.facade) else {
                continue;
            };
            let Some(bridge_declaration_id) = c_binding_raw_call_owner_declaration_id(module, &facade.bridge) else {
                continue;
            };
            let mut raw_call_ids = c_abi
                .raw_calls
                .iter()
                .filter(|raw_call| {
                    raw_call.owner.as_ref() == Some(&facade.bridge) && c_abi.bindings.contains_key(&raw_call.binding)
                })
                .map(|raw_call| c_binding_call_record_id(module_id, &raw_call.binding, &raw_call.symbol, raw_call.span))
                .collect::<Vec<_>>();
            raw_call_ids.sort();
            raw_call_ids.dedup();
            if raw_call_ids.is_empty() {
                continue;
            }
            self.records
                .push(CodegraphRecord::CBindingFacade(CodegraphCBindingFacadeRecord {
                    id: c_binding_facade_record_id(module_id, &facade.facade, &facade.bridge, facade.call_span),
                    language: CodegraphLanguage::Incan,
                    module_id: module_id.to_string(),
                    facade_declaration_id,
                    bridge_declaration_id,
                    call_id: self.call_record_id_at_span(module_id, facade.call_span),
                    raw_call_ids,
                    span: source_span(module, facade.call_span),
                    provenance: CodegraphProvenance::Checked,
                    degraded: false,
                }));
        }
    }

    /// Return the generic source-level call fact emitted for one checked raw-call span.
    pub(super) fn call_record_id_at_span(&self, module_id: &str, span: Span) -> Option<String> {
        self.call_ids.get(&(module_id.to_owned(), span)).cloned()
    }

    /// Add declaration, import, export, and containment records for a parsed module body.
    pub(super) fn collect_program_records(&mut self, module: &ParsedModule, module_id: &str, degraded: bool) {
        for (index, declaration) in module.ast.declarations.iter().enumerate() {
            match &declaration.node {
                Declaration::Import(import) => {
                    self.collect_import_records(module, module_id, index, import, declaration.span, degraded);
                }
                Declaration::Docstring(_) => {}
                _ => {
                    let declaration_id = declaration_id(module, declaration, index);
                    let Some(summary) = declaration_summary(&declaration.node) else {
                        continue;
                    };
                    let canonical_identity = self
                        .declaration_canonical_identity(module, declaration.span, &summary.name)
                        .cloned();
                    let wire_identity = canonical_identity.as_ref().map(codegraph_canonical_identity);
                    let wire_stable_identity = self.stable_identity_for(canonical_identity.as_ref());
                    let lowered = canonical_identity
                        .as_ref()
                        .and_then(|identity| self.declaration_facts.get(identity));
                    self.records
                        .push(CodegraphRecord::Declaration(CodegraphDeclarationRecord {
                            id: declaration_id.clone(),
                            language: CodegraphLanguage::Incan,
                            module_id: module_id.to_string(),
                            kind: summary.kind.clone(),
                            name: summary.name.clone(),
                            visibility: visibility_spelling(summary.visibility).to_string(),
                            type_params: summary.type_params,
                            signature: summary.signature,
                            canonical_identity: wire_identity.clone(),
                            stable_identity: wire_stable_identity.clone(),
                            semantic_digest: lowered.and_then(|facts| facts.semantic_digest.clone()),
                            doc_digest: lowered.and_then(|facts| facts.doc_digest.clone()),
                            span: Some(source_span(module, declaration.span)),
                            provenance: provenance_for_identity(canonical_identity.as_ref()),
                            degraded,
                        }));
                    self.records.push(CodegraphRecord::Containment(containment_record(
                        module_id,
                        &declaration_id,
                        "module_contains_declaration",
                        module,
                        declaration.span,
                        degraded,
                    )));
                    if summary.visibility == Visibility::Public {
                        self.records.push(CodegraphRecord::Export(export_record(
                            module,
                            module_id,
                            &declaration_id,
                            &summary.name,
                            "declaration",
                            declaration.span,
                            wire_identity,
                            wire_stable_identity,
                            degraded,
                        )));
                    }
                    self.collect_declaration_body_records(module, module_id, &declaration_id, declaration, degraded);
                }
            }
        }
    }
}
