//! Reference and call records, file records, diagnostics, and finishing the record set.

use super::*;

impl<'analysis> CodegraphBuilder<'analysis> {
    /// Push one source-level reference record and its owner containment edge.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_reference(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        name: &str,
        kind: &str,
        span: Span,
        degraded: bool,
    ) {
        let checked = self
            .source_checked_reference(module, span)
            .map(|reference| (reference.target.clone(), reference.owner.cloned()));
        self.push_reference_with_checked(module, module_id, owner_id, name, kind, span, degraded, checked);
    }

    /// Choose the owner id a navigation consumer should follow for one record.
    ///
    /// A proven checked owner supersedes the syntactic one; syntax ownership is a fallback for the case where
    /// nothing was proven, not a second opinion to be merged with it. Reference and call records both make this
    /// choice, and they made it with two spellings of the same condition, so it is decided here once.
    ///
    /// Returns an owned id because every caller pushes a record immediately afterwards, so a borrow of `self`
    /// cannot outlive the call.
    pub(super) fn navigable_owner_id(
        &self,
        canonical_owner: Option<&CanonicalSymbolId>,
        syntactic_owner_id: Option<&str>,
        has_checked_target: bool,
    ) -> Option<String> {
        if !has_checked_target {
            return syntactic_owner_id.map(str::to_string);
        }
        canonical_owner
            .and_then(|identity| self.canonical_target_ids.get(identity))
            .cloned()
    }

    /// Emit one checked reference projection, including a type component sharing an expression's source span.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_reference_with_checked(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        name: &str,
        kind: &str,
        span: Span,
        degraded: bool,
        checked: Option<(CanonicalSymbolId, Option<CanonicalSymbolId>)>,
    ) {
        let id = self.next_body_fact_id("reference", module, span, name);
        let has_checked_target = checked.is_some();
        let (canonical_identity, canonical_owner) = match checked {
            Some((target, owner)) => (Some(target), owner),
            None => (None, None),
        };
        self.reference_sites
            .insert((module_id.to_owned(), span, canonical_identity.clone()));
        let owner_id = self.navigable_owner_id(canonical_owner.as_ref(), owner_id, has_checked_target);
        let target_id = canonical_identity
            .as_ref()
            .and_then(|identity| self.canonical_target_ids.get(identity))
            .cloned();
        let provenance = provenance_for_identity(canonical_identity.as_ref());
        let stable_identity = self.stable_identity_for(canonical_identity.as_ref());
        self.records.push(CodegraphRecord::Reference(CodegraphReferenceRecord {
            id: id.clone(),
            language: reference_language(canonical_identity.as_ref()),
            module_id: module_id.to_string(),
            owner_id: owner_id.clone(),
            name: name.to_string(),
            kind: kind.to_string(),
            target_id,
            canonical_identity: canonical_identity.as_ref().map(codegraph_canonical_identity),
            stable_identity,
            canonical_owner: canonical_owner.as_ref().map(codegraph_canonical_identity),
            span: Some(source_span(&module.file_path, &module.source, span)),
            provenance,
            degraded,
        }));
        if let Some(owner_id) = owner_id.as_deref() {
            self.records.push(CodegraphRecord::Containment(containment_record(
                owner_id,
                &id,
                "declaration_contains_reference",
                &module.file_path,
                &module.source,
                span,
                degraded,
            )));
        }
    }

    /// Return the leftmost identifier of a dotted expression chain, when it is one.
    pub(super) fn expr_root_ident(expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Ident(name) => Some(name.as_str()),
            Expr::Field(base, _) => Self::expr_root_ident(&base.node),
            _ => None,
        }
    }

    /// Return whether a method-call receiver names a type this module declares.
    ///
    /// `Signal.Ready()` and `Widget.build()` parse as method calls on a bare identifier, but the identifier is a type,
    /// not a value. Labeling those by the bare member name loses which type answers the call and makes a variant
    /// indistinguishable from any other `Ready` in the file, so record the receiver the source wrote. A receiver that
    /// is an ordinary value keeps the bare member name: `user.save()` is a method on a value, not a qualified path.
    pub(super) fn receiver_names_a_declared_type(module: &ParsedModule, receiver: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &receiver.node else {
            return false;
        };
        module.ast.declarations.iter().any(|declaration| {
            let declared = match &declaration.node {
                Declaration::Model(decl) => decl.name.as_str(),
                Declaration::Class(decl) => decl.name.as_str(),
                Declaration::Trait(decl) => decl.name.as_str(),
                Declaration::TypeAlias(decl) => decl.name.as_str(),
                Declaration::Newtype(decl) => decl.name.as_str(),
                Declaration::Enum(decl) => decl.name.as_str(),
                _ => return false,
            };
            declared == name.as_str()
        })
    }

    /// Return whether a method-call receiver names a module this file imported as a whole.
    ///
    /// Only a plain `import <module>` binding qualifies. An item import (`from m import f`) binds the item, not the
    /// module, and a value named after a module is still a value.
    pub(super) fn receiver_names_an_imported_module(module: &ParsedModule, receiver: &Spanned<Expr>) -> bool {
        // `std.builtins.len(..)` reaches a module through a dotted path rather than a single binding. Its root is the
        // stdlib namespace, which is always a module, so the chain names one wherever it stops. A receiver rooted at
        // an ordinary name is not: `Signal.Ready()` reads the same way but names a type, which
        // `receiver_names_a_declared_type` answers instead.
        if let Expr::Field(base, _) = &receiver.node {
            return Self::expr_root_ident(&base.node) == Some(incan_lang::lang::stdlib::STDLIB_ROOT);
        }
        let Expr::Ident(name) = &receiver.node else {
            return false;
        };
        module.ast.declarations.iter().any(|declaration| {
            let Declaration::Import(import) = &declaration.node else {
                return false;
            };
            let ImportKind::Module(path) = &import.kind else {
                return false;
            };
            let binding = import
                .alias
                .as_deref()
                .or_else(|| path.segments.last().map(String::as_str));
            binding == Some(name.as_str())
        })
    }

    /// Push one source-level call record and its owner containment edge.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_call(
        &mut self,
        module: &ParsedModule,
        module_id: &str,
        owner_id: Option<&str>,
        callee: &str,
        kind: &str,
        argument_count: usize,
        type_argument_count: usize,
        span: Span,
        target_span: Span,
        degraded: bool,
    ) {
        let id = self.next_body_fact_id("call", module, span, callee);
        self.call_ids.insert((module_id.to_owned(), span), id.clone());
        let checked = self.source_checked_reference(module, target_span);
        let canonical_identity = checked.map(|reference| reference.target.clone());
        let canonical_owner = checked.and_then(|reference| reference.owner.cloned());
        let owner_id = self.navigable_owner_id(canonical_owner.as_ref(), owner_id, checked.is_some());
        let target_id = canonical_identity
            .as_ref()
            .and_then(|identity| self.canonical_target_ids.get(identity))
            .cloned();
        let provenance = provenance_for_identity(canonical_identity.as_ref());
        let stable_identity = self.stable_identity_for(canonical_identity.as_ref());
        self.records.push(CodegraphRecord::Call(CodegraphCallRecord {
            id: id.clone(),
            language: CodegraphLanguage::Incan,
            module_id: module_id.to_string(),
            owner_id: owner_id.clone(),
            callee: callee.to_string(),
            kind: kind.to_string(),
            argument_count,
            type_argument_count,
            target_id,
            canonical_identity: canonical_identity.as_ref().map(codegraph_canonical_identity),
            stable_identity,
            canonical_owner: canonical_owner.as_ref().map(codegraph_canonical_identity),
            span: Some(source_span(&module.file_path, &module.source, span)),
            provenance,
            degraded,
        }));
        if let Some(owner_id) = owner_id.as_deref() {
            self.records.push(CodegraphRecord::Containment(containment_record(
                owner_id,
                &id,
                "declaration_contains_call",
                &module.file_path,
                &module.source,
                span,
                degraded,
            )));
        }
    }

    /// Return the next deterministic body-fact id for one export.
    pub(super) fn next_body_fact_id(&mut self, kind: &str, module: &ParsedModule, span: Span, label: &str) -> String {
        let index = self.next_body_fact_index;
        self.next_body_fact_index += 1;
        format!(
            "{kind}:{}:{}:{index}:{}",
            module.file_path.to_string_lossy(),
            span.start,
            sanitize_record_label(label)
        )
    }

    /// Insert one file record if it has not already been seen, returning the stable file id.
    pub(super) fn ensure_file_record(&mut self, path: &Path, source: &str, degraded: bool) -> String {
        let path = path_string(path);
        if let Some(id) = self.file_ids.get(&path) {
            return id.clone();
        }
        let id = format!("file:{path}");
        self.records.push(CodegraphRecord::File(CodegraphFileRecord {
            id: id.clone(),
            language: CodegraphLanguage::Incan,
            path: path.clone(),
            size_bytes: source.len(),
            provenance: CodegraphProvenance::Source,
            degraded,
        }));
        self.file_ids.insert(path, id.clone());
        id
    }

    /// Buffer diagnostics until `finish` appends them after syntax facts.
    pub(super) fn collect_diagnostics(&mut self, diagnostics: Vec<StableDiagnostic>) {
        dedup_diagnostics(&mut self.diagnostics, diagnostics);
    }

    /// Return whether the export has diagnostics and should be considered degraded.
    pub(super) fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    /// Return buffered diagnostics for strict-mode failure rendering.
    pub(super) fn diagnostics(&self) -> &[StableDiagnostic] {
        &self.diagnostics
    }

    /// Infer module path segments for independently parsed files using the inspected root as the stable base.
    pub(super) fn fallback_module_segments(&self, path: &Path) -> Vec<String> {
        let base = if self.root_path_buf.is_dir() {
            self.root_path_buf.as_path()
        } else {
            self.root_path_buf.parent().unwrap_or_else(|| Path::new("."))
        };
        module_segments_for_file(path, base)
    }

    /// Assemble the final header, syntax records, and diagnostic records in stable JSONL order.
    pub(super) fn finish(mut self) -> Vec<CodegraphRecord> {
        self.materialize_registry_reexport_projections();
        let degraded = self.records.iter().any(record_degraded) || !self.diagnostics.is_empty();
        let languages = represented_languages(&self.records);
        let mut records = vec![CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: INCAN_VERSION.to_string(),
            mode: self.mode,
            root_path: self.root_path,
            languages,
            package: self.package,
            semantic_contexts: self.semantic_contexts,
            degraded,
        })];
        records.append(&mut self.records);
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            records.push(CodegraphRecord::Diagnostic(diagnostic_record(index, diagnostic)));
        }
        records
    }

    /// Attach public facade paths to the one checked source-owned registry fact they expose.
    ///
    /// Registry entries are emitted only after successful typechecking. This pass reuses the already-exported public
    /// import graph to resolve facade aliases; it does not inspect decorators, execute registry code, or emit a second
    /// semantic entry. An import chain that cannot be resolved to a source path is omitted rather than guessed.
    pub(super) fn materialize_registry_reexport_projections(&mut self) {
        let module_paths = self
            .records
            .iter()
            .filter_map(|record| match record {
                CodegraphRecord::Module(module) => Some((module.id.clone(), module.module_path.clone())),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut aliases = BTreeMap::new();

        for record in &self.records {
            let CodegraphRecord::Import(import) = record else {
                continue;
            };
            if import.degraded || import.visibility != "public" || import.kind != "from" {
                continue;
            }
            let Some(facade_module) = module_paths.get(&import.module_id) else {
                continue;
            };
            let Some(imported_module) = codegraph_source_import_path(&import.path) else {
                continue;
            };
            let Some(span) = import.span.clone() else {
                continue;
            };
            for item in &import.items {
                let Some((source_name, local_name)) = codegraph_import_item_names(item) else {
                    continue;
                };
                let mut facade_path = facade_module.clone();
                facade_path.push(local_name);
                let mut target_path = imported_module.clone();
                target_path.push(source_name);
                aliases.insert(facade_path, (target_path, span.clone()));
            }
        }

        let projections = aliases
            .keys()
            .filter_map(|path| {
                let (_, span) = aliases.get(path)?;
                let target = resolve_codegraph_alias_target(path, &aliases)?;
                Some((
                    target,
                    CodegraphRegistryReexportProjection {
                        path: path.clone(),
                        span: span.clone(),
                    },
                ))
            })
            .collect::<Vec<_>>();

        for record in &mut self.records {
            let CodegraphRecord::Registry(registry) = record else {
                continue;
            };
            if !registry.registry_public {
                continue;
            }
            let mut canonical_paths = vec![registry_identity_path(&registry.registry_identity)];
            if let Some(subject_path) = registry_subject_path(&registry.subject_identity) {
                canonical_paths.push(subject_path);
            }
            let mut paths = projections
                .iter()
                .filter(|(target, _)| canonical_paths.iter().any(|canonical| canonical == target))
                .map(|(_, projection)| projection.clone())
                .collect::<Vec<_>>();
            paths.sort_by(|left, right| {
                (&left.path, &left.span.file, left.span.start, left.span.end).cmp(&(
                    &right.path,
                    &right.span.file,
                    right.span.start,
                    right.span.end,
                ))
            });
            paths.dedup_by(|left, right| left.path == right.path && left.span == right.span);
            registry.reexport_paths = paths;
        }
    }

    /// Query the shared checked target and closest declaration owner; syntax-only records carry no authority.
    pub(super) fn source_checked_reference(
        &self,
        module: &ParsedModule,
        span: Span,
    ) -> Option<incan_semantics_core::dependencies::CheckedReference<'_>> {
        let module_identity = incan_semantics_core::module_identity_for_path(&module.path_segments);
        let subject = CompilerNodeId::expression_at(&module_identity, span);
        self.semantic_snapshots_by_path
            .get(&module.file_path)?
            .facts
            .checked_reference(&subject)
    }

    /// Return the canonical identity minted for one emitted top-level declaration.
    pub(super) fn declaration_canonical_identity(
        &self,
        module: &ParsedModule,
        span: Span,
        name: &str,
    ) -> Option<&CanonicalSymbolId> {
        self.semantic_snapshots_by_path
            .get(&module.file_path)?
            .hir
            .declarations
            .iter()
            .find(|declaration| {
                declaration.span.start == span.start
                    && declaration.span.end == span.end
                    && declaration.name.as_deref() == Some(name)
            })?
            .canonical
            .as_ref()
    }

    /// Return every checked binding introduced by one import declaration.
    pub(super) fn import_canonical_bindings(&self, module: &ParsedModule, span: Span) -> Vec<CodegraphImportBinding> {
        self.semantic_snapshots_by_path
            .get(&module.file_path)
            .into_iter()
            .flat_map(|snapshot| snapshot.hir.declarations.iter())
            .filter(|declaration| {
                declaration.kind == incan_semantics_core::HirDeclarationKind::Import
                    && declaration.span.start == span.start
                    && declaration.span.end == span.end
            })
            .filter_map(|declaration| {
                Some(CodegraphImportBinding {
                    local_name: declaration.name.clone()?,
                    canonical_identity: declaration.canonical.as_ref().map(codegraph_canonical_identity),
                    stable_identity: self.stable_identity_for(declaration.canonical.as_ref()),
                })
            })
            .collect()
    }
}
