//! Import collection: the one dispatch from an import declaration to its home. A module of a compiled Loaf (`pub::<key>`
//! or a `std.*` namespace of a standard package) is bound from the Loaf's manifest (`loaf_imports`), a `rust::` path
//! from Rust metadata (`rust_imports`), and anything else from the program's own source modules (here).

use std::collections::HashSet;

use crate::ast::*;
use crate::diagnostics::errors;
use crate::module::{ExportedSymbol, canonicalize_source_module_segments};
use crate::symbols::*;
use crate::typechecker::collect::loaf_imports::LoafImport;
use crate::typechecker::{ImportedRegistryDefinitionInfo, PartialProjectionInfo, TypeChecker};
use incan_lang::lang::stdlib::{self, is_typechecker_only_stdlib};
use incan_semantics_core::CanonicalSymbolId;

impl TypeChecker {
    /// Reject source names that shadow reserved root namespaces or protected builtin bindings.
    pub(super) fn validate_root_namespace(&mut self, name: &str, span: Span) {
        self.validate_protected_builtin_binding(name, span);
        if name == stdlib::STDLIB_ROOT || name == "rust" {
            self.errors.push(errors::reserved_root_namespace(name, span));
        }
    }

    /// Register an import declaration in the symbol table.
    pub(super) fn collect_import(&mut self, import: &ImportDecl, span: Span) {
        self.validate_import_visibility(import, span);
        match &import.kind {
            ImportKind::Module(path) => {
                self.collect_module_import(path, import.alias.as_ref(), span);
            }
            ImportKind::From { module, items } => {
                self.collect_from_imports(module, items, span);
            }
            ImportKind::PubLibrary { library, path } => {
                let target = LoafImport {
                    key: library.clone(),
                    module: path.clone(),
                };
                self.collect_loaf_module_import(&target, import.alias.as_ref(), span);
            }
            ImportKind::PubFrom { library, path, items } => {
                let target = LoafImport {
                    key: library.clone(),
                    module: path.clone(),
                };
                self.collect_loaf_from_imports(&target, items, span);
            }
            ImportKind::Python(pkg) => {
                // The grammar reserves the form, but nothing provides a Python module (#1561). The name is still bound
                // so its uses do not add unknown-symbol errors to the refusal.
                self.errors.push(errors::python_import_unsupported(pkg, span));
                let name = import.alias.clone().unwrap_or_else(|| pkg.clone());
                self.validate_root_namespace(&name, span);
                self.define_import_symbol(name, vec![pkg.clone()], true, None, span);
            }
            ImportKind::RustCrate { crate_name, path, .. } => {
                self.collect_rust_crate_import(crate_name, path, import.alias.as_ref(), span);
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..  // version, features: not used here
            } => {
                self.collect_rust_from_imports(crate_name, path, items, span);
            }
        }
    }

    /// Return whether `path` is spelled from the `std` root: a standard-library path rather than a source module.
    pub(super) fn is_standard_path(path: &ImportPath) -> bool {
        path.parent_levels == 0
            && !path.is_absolute
            && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
    }

    /// The Loaf import a `std.*` path reaches: a standard package other than the program's own owns its namespace root.
    pub(super) fn std_loaf_import(&self, path: &ImportPath) -> Option<LoafImport> {
        if !Self::is_standard_path(path) || self.owns_standard_module(&path.segments) {
            return None;
        }
        self.standard_loaf_import(&path.segments)
    }

    /// Return whether `path` is a `std.*` module nothing provides: no standard package owns it, it is not one of the
    /// program's own standard modules, and the compiler does not declare it.
    pub(super) fn is_unknown_standard_module(&self, path: &ImportPath) -> bool {
        Self::is_standard_path(path)
            && path.segments.len() > 1
            && !self.owns_standard_module(&path.segments)
            && !is_typechecker_only_stdlib(&path.segments)
            && self.loaves.owner_of(&path.segments).is_none()
    }

    /// Collect a plain module import.
    ///
    /// The import binds the last segment it spells (or its alias); a `std.<namespace>.prelude` path resolves as the
    /// namespace's module.
    pub(super) fn collect_module_import(&mut self, written_path: &ImportPath, alias: Option<&Ident>, span: Span) {
        let prelude_namespace = Self::stdlib_prelude_import_namespace(written_path);
        let path = prelude_namespace.as_ref().unwrap_or(written_path);
        let name = alias.cloned().unwrap_or_else(|| {
            written_path
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| "module".to_string())
        });
        if let Some(target) = self.std_loaf_import(path) {
            self.collect_loaf_module_import(&target, Some(&name), span);
            return;
        }
        // Reject `import std.f64.consts` - unknown stdlib module; suggest `import rust::std::f64::consts`.
        if self.is_unknown_standard_module(path) {
            self.errors
                .push(errors::unknown_stdlib_module(&path.segments.join("."), span));
        }

        // Allow `import std.web as std` (alias matches source root), but reject `import std.web as rust` (alias is a
        // different reserved root).
        let same_root = path.segments.first().map(|segment| segment.as_str()) == Some(&name);
        if !same_root {
            self.validate_root_namespace(&name, span);
        } else {
            self.validate_protected_builtin_binding(&name, span);
        }
        // `import a::b::C` is ambiguous: it can name a module, or an item inside its parent module. Module
        // resolution already tries the full path and then the parent (see `module::source_import_candidates`), and
        // symbol collection has to follow the same order or the two disagree about what was imported.
        if !self.full_path_names_a_module(path) && self.collect_module_import_as_item(path, alias, span) {
            return;
        }

        let normalized_path = canonicalize_source_module_segments(&path.segments);
        let resolved_path = self.resolved_source_module_path(path);
        let target_identity = resolved_path.as_deref().and_then(SymbolTable::module_path_identity);
        let canonical_path = resolved_path.unwrap_or(normalized_path);
        self.define_import_symbol(name, canonical_path, false, target_identity, span);
    }

    /// Whether the whole written path names a source module, rather than an item inside its parent.
    ///
    /// Checked against the full path only, deliberately: the shared candidate helper also offers the parent, which
    /// is the reading this is trying to distinguish from.
    pub(super) fn full_path_names_a_module(&self, path: &ImportPath) -> bool {
        let key = canonicalize_source_module_segments(&path.segments).join("_");
        self.dependency_exports.contains_key(&key)
            || self.dependency_member_symbols.contains_key(&key)
            || self.dependency_module_path_segments.contains_key(&key)
    }

    /// Return the import path of the namespace a `std.<namespace>.prelude` import path names, or `None` for any other
    /// path.
    ///
    /// A namespace's `prelude` is the namespace's own module (see [`stdlib::stdlib_prelude_module_namespace`]), so the
    /// import resolves through the namespace, the path its provider claims and lowering emits (#1561).
    pub(super) fn stdlib_prelude_import_namespace(path: &ImportPath) -> Option<ImportPath> {
        if path.parent_levels != 0 || path.is_absolute {
            return None;
        }
        stdlib::stdlib_prelude_module_namespace(&path.segments).map(|segments| ImportPath {
            segments: segments.to_vec(),
            is_absolute: false,
            parent_levels: 0,
        })
    }

    /// Bind `import module::item` as the item it names, with the declaration's own symbol kind.
    ///
    /// Without this the last segment was bound as if it were a module: the name resolved, so nothing reported an
    /// error, but the binding carried no signature and every call through it inferred `Unknown`. A local assigned
    /// from such a call then had no type, and arithmetic on it failed with `expected 'numeric', found '? + ?'` —
    /// pointing at the arithmetic rather than at the import that caused it (#1407).
    ///
    /// Routes through the same symbol definition `from module import item` uses, so the two import spellings cannot
    /// disagree about what a given item is.
    pub(super) fn collect_module_import_as_item(&mut self, path: &ImportPath, alias: Option<&Ident>, span: Span) -> bool {
        if path.segments.len() < 2 {
            return false;
        }
        let Some((item_name, parent_segments)) = path.segments.split_last() else {
            return false;
        };
        let parent = ImportPath {
            segments: parent_segments.to_vec(),
            is_absolute: path.is_absolute,
            parent_levels: path.parent_levels,
        };
        let item = ImportItem {
            name: item_name.clone(),
            alias: alias.cloned(),
        };
        let Some(kind) = self.imported_source_dependency_symbol_kind(&parent, &item) else {
            return false;
        };
        let projection = self.imported_source_dependency_partial_projection(&parent, &item);
        self.define_resolved_source_import_symbol(&parent, &parent, &item, kind, projection, span);
        true
    }

    /// Resolve a source import to the exact module graph node that accepted it.
    ///
    /// This mirrors member-resolution candidate order and refuses ambiguous suffix recovery. The path written by the
    /// user is not necessarily the declaration owner for a sibling-relative import, so it cannot itself be identity
    /// evidence.
    pub(super) fn resolved_source_module_path(&self, path: &ImportPath) -> Option<Vec<String>> {
        let base = self.current_module_path.as_deref().unwrap_or_default();
        for candidate in self.dependency_source_import_candidates(base, path) {
            let key = candidate.join("_");
            if self.dependency_exports.contains_key(&key)
                || self.dependency_member_symbols.contains_key(&key)
                || self.dependency_module_path_segments.contains_key(&key)
            {
                return Some(
                    self.dependency_module_path_segments
                        .get(&key)
                        .cloned()
                        .unwrap_or(candidate),
                );
            }
        }
        None
    }

    /// Collect a `from module import item, ...` declaration.
    ///
    /// A `std.<namespace>.prelude` path resolves as the namespace's module.
    pub(super) fn collect_from_imports(&mut self, written_module: &ImportPath, items: &[ImportItem], span: Span) {
        let prelude_namespace = Self::stdlib_prelude_import_namespace(written_module);
        let module = prelude_namespace.as_ref().unwrap_or(written_module);
        if let Some(target) = self.std_loaf_import(module) {
            self.collect_loaf_from_imports(&target, items, span);
            return;
        }
        if Self::is_standard_path(module) && module.segments.len() == 1 {
            for item in items {
                self.collect_std_root_import(item, span);
            }
            return;
        }
        if Self::is_standard_path(module) && is_typechecker_only_stdlib(&module.segments) {
            for item in items {
                self.collect_capability_bound_import(module, item, span);
            }
            return;
        }
        if self.is_unknown_standard_module(module) {
            self.errors
                .push(errors::unknown_stdlib_module(&module.segments.join("."), span));
        }
        for item in items {
            if self.materialize_own_standard_import(module, item, span) {
                continue;
            }
            if self.materialize_source_dependency_import(module, item, span) {
                continue;
            }
            if self.preserve_existing_from_import_symbol(module, item, span) {
                continue;
            }
            self.define_from_import_placeholder(module, item, span);
        }
    }

    /// Bind one item of `from std import …`: the standard-library root exports its namespaces and nothing else, so the
    /// item is the `std.<item>` module, owned by a standard package or by the program's own standard source.
    pub(super) fn collect_std_root_import(&mut self, item: &ImportItem, span: Span) {
        let module = vec![stdlib::STDLIB_ROOT.to_string(), item.name.clone()];
        let local_name = Self::import_item_local_name(item);
        if self.owns_standard_module(&module) {
            self.validate_root_namespace(&local_name, span);
            let target_identity = SymbolTable::module_path_identity(&module);
            self.define_import_symbol(local_name, module, false, target_identity, span);
            return;
        }
        if let Some(target) = self.standard_loaf_import(&module) {
            self.collect_loaf_module_import(&target, Some(&local_name), span);
            return;
        }
        self.refuse_std_root_member_import(item, span);
    }

    /// Materialize one imported source dependency, preserving package-private implementation facts.
    pub(super) fn materialize_source_dependency_import(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        let Some(kind) = self.imported_source_dependency_symbol_kind(module, item) else {
            return false;
        };
        let projection = self.imported_source_dependency_partial_projection(module, item);
        self.define_resolved_source_import_symbol(module, module, item, kind, projection, span);
        true
    }

    /// Bind an item of one of the program's own standard modules while it is a standard package.
    ///
    /// The package's source keeps physical paths such as `registry`, while its public surface is spelled
    /// `std.registry`; resolve the physical path and require that the checked module graph contains the member.
    pub(super) fn materialize_own_standard_import(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        if !Self::is_standard_path(module) || !self.owns_standard_module(&module.segments) {
            return false;
        }
        let source_module = ImportPath::absolute(module.segments[1..].to_vec());
        let Some(kind) = self.imported_source_dependency_symbol_kind(&source_module, item) else {
            return false;
        };
        let projection = self.imported_source_dependency_partial_projection(&source_module, item);
        self.define_resolved_source_import_symbol(module, &source_module, item, kind, projection, span);
        true
    }

    /// Return an import item's local binding name after applying `as alias`.
    pub(super) fn import_item_local_name(item: &ImportItem) -> Ident {
        item.alias.clone().unwrap_or_else(|| item.name.clone())
    }

    /// Preserve an imported item that has already been materialized as a concrete symbol in this collection pass.
    ///
    /// This keeps dependency metadata imports, especially statics, from being rewritten as module path proxies.
    /// Returns `true` when the caller should skip fallback placeholder materialization.
    pub(super) fn preserve_existing_from_import_symbol(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        let Some(mut imported_kind) = self.existing_from_import_symbol_kind(&item.name) else {
            return false;
        };

        if let SymbolKind::Static(info) = &mut imported_kind {
            info.is_imported = true;
        }
        if let Some(alias) = &item.alias {
            if self.symbols.lookup(alias).is_none() {
                self.validate_root_namespace(alias, span);
                self.record_source_import_target(module, item, alias, &imported_kind);
                if matches!(imported_kind, SymbolKind::Static(_)) {
                    self.type_info.declarations.static_bindings.insert(
                        alias.clone(),
                        crate::typechecker::StaticBindingInfo { is_imported: true },
                    );
                }
                // RFC 120: the alias is a second binding to the already-materialized symbol, so it carries that
                // symbol's identity rather than minting one of its own.
                let target_identity = self
                    .symbols
                    .lookup(&item.name)
                    .and_then(|id| self.symbols.identity_of(id).cloned());
                let mut binding_path = canonicalize_source_module_segments(&module.segments);
                binding_path.push(item.name.clone());
                self.symbols.define_import_binding_at_path(
                    Symbol {
                        name: alias.clone(),
                        kind: imported_kind,
                        span,
                        scope: 0,
                    },
                    target_identity,
                    binding_path,
                );
                self.mark_static_binding_imported(&item.name);
                return true;
            }
        } else {
            self.record_source_import_target(module, item, &item.name, &imported_kind);
            self.mark_static_binding_imported(&item.name);
            return true;
        }
        false
    }

    /// Retain the canonical identity proven for an imported binding, keyed by the local name the import introduced.
    ///
    /// Recorded only when import resolution proves the declaration, so a consumer gets a correct identity or none at
    /// all. It is deliberately separate from [`crate::typechecker::SourceTargetInfo::module_path`], which keeps its
    /// existing meaning of the path as written at the import.
    pub(super) fn record_resolved_import_owner(&mut self, module: &ImportPath, item: &ImportItem, local_name: &str) {
        let identity = self
            .dependency_member_identity(module, &item.name);
        if let Some(identity) = identity {
            // A binding materialized before this proof starts identity-less; attach the proof to that binding —
            // and only to an import binding — so reference-side recording never has to reach past the symbol
            // table to a name-keyed map that a shadowing definition may have made stale.
            self.symbols.backfill_import_identity(local_name, &identity);
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name.to_string(), identity);
        }
    }

    /// Record the codegraph source target an import makes visible under `local_name`.
    ///
    /// The recorded `module_path` is the import path as *written*, which is the codegraph's existing contract. The
    /// module that resolution actually selected is recorded separately by [`Self::record_resolved_import_owner`],
    /// because the two are not always the same and only the latter may back a declaration identity.
    pub(super) fn record_source_import_target(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        local_name: &str,
        kind: &SymbolKind,
    ) {
        if let Some(target_kind) = Self::source_target_kind(kind) {
            self.source_import_targets.insert(
                local_name.to_string(),
                crate::typechecker::SourceTargetInfo {
                    module_path: canonicalize_source_module_segments(&module.segments),
                    name: item.name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
        self.record_resolved_import_owner(module, item, local_name);
    }

    /// Define a fallback module placeholder for one `from module import item` binding.
    pub(super) fn define_from_import_placeholder(&mut self, module: &ImportPath, item: &ImportItem, span: Span) {
        let name = Self::import_item_local_name(item);
        self.validate_root_namespace(&name, span);
        let mut path = canonicalize_source_module_segments(&module.segments);
        path.push(item.name.clone());
        self.define_import_symbol(name, path, false, None, span);
    }

    /// Define one imported item under its local alias after root namespace validation.
    ///
    /// The proven declaring identity is attached to the binding and recorded as a checked fact: lowering keys a
    /// stdlib trait's protocol on that identity, and a spelling alone cannot tell `std.serde.json.Serialize` from an
    /// unrelated trait of the same name (#1431).
    pub(super) fn define_from_import_symbol(&mut self, module: &ImportPath, item: &ImportItem, kind: SymbolKind, span: Span) {
        let local_name = Self::import_item_local_name(item);
        let target_identity = self
            .dependency_member_identity(module, &item.name);
        let symbol_id =
            self.define_named_import_symbol(module, item, local_name.clone(), kind, target_identity.clone(), span);
        if let Some(identity) = target_identity
            && self.symbols.is_active_lookup_binding(symbol_id)
        {
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name, identity);
        }
    }

    /// Return the exact source dependency member targeted by a `from module import item` declaration.
    pub(super) fn imported_source_dependency_symbol_kind(&self, module: &ImportPath, item: &ImportItem) -> Option<SymbolKind> {
        self.dependency_member_symbol_for_path(module, &item.name)
    }

    /// Return the exact partial projection metadata targeted by a source import.
    pub(super) fn imported_source_dependency_partial_projection(
        &self,
        module: &ImportPath,
        item: &ImportItem,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_member_partial_projection_for_path(module, &item.name)
    }

    /// Return the codegraph declaration kind for source targets this importer can preserve.
    pub(super) fn source_target_kind(kind: &SymbolKind) -> Option<&'static str> {
        Self::source_target_kind_for_symbol(kind)
    }

    /// Define a source-imported dependency symbol under its local import name.
    pub(super) fn define_resolved_source_import_symbol(
        &mut self,
        written_module: &ImportPath,
        resolved_module: &ImportPath,
        item: &ImportItem,
        mut kind: SymbolKind,
        projection: Option<PartialProjectionInfo>,
        span: Span,
    ) {
        let local_name = Self::import_item_local_name(item);
        if let SymbolKind::Static(info) = &mut kind {
            info.is_imported = true;
        }
        self.validate_root_namespace(&local_name, span);
        let target = self.dependency_member_resolution(resolved_module, &item.name);
        let mut binding_path = canonicalize_source_module_segments(&written_module.segments);
        binding_path.push(item.name.clone());
        let symbol_id = self.symbols.define_import_binding_at_path(
            Symbol {
                name: local_name.clone(),
                kind: kind.clone(),
                span,
                scope: 0,
            },
            target.as_ref().map(|member| member.identity.clone()),
            binding_path,
        );
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }

        if let Some(member) = target {
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name.clone(), member.identity);
            self.type_info
                .declarations
                .resolved_import_declared_names
                .insert(local_name.clone(), member.declared_name);
        }

        if matches!(kind, SymbolKind::Type(TypeInfo::TypeAlias))
            && let Some(target) = self.dependency_member_type_alias_for_path(resolved_module, &item.name)
        {
            self.record_dependency_import_type_alias_before_change(&local_name);
            self.type_aliases.insert(local_name.clone(), target);
        }
        if let Some(target_kind) = Self::source_target_kind(&kind) {
            self.source_import_targets.insert(
                local_name.clone(),
                crate::typechecker::SourceTargetInfo {
                    module_path: canonicalize_source_module_segments(&written_module.segments),
                    name: item.name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
        self.record_imported_function_binding(&local_name, &kind);
        if matches!(kind, SymbolKind::Static(_)) {
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::typechecker::StaticBindingInfo { is_imported: true },
            );
            if let Some((definition, owner_module_path)) =
                self.dependency_registry_definition_for_path(resolved_module, &item.name)
            {
                self.type_info.registry.imported_definitions.insert(
                    local_name.clone(),
                    ImportedRegistryDefinitionInfo {
                        definition,
                        owner_module_path,
                        owner_binding: item.name.clone(),
                    },
                );
            }
        }
        if let Some(mut projection) = projection {
            projection.name.clone_from(&local_name);
            self.type_info.record_partial_projection(projection);
        }
    }

    /// Define one already named imported symbol after root namespace validation.
    ///
    /// `target_identity` belongs only to this import target. It must never be recovered from a name-keyed map that may
    /// describe an earlier colliding binding; an unavailable provider identity remains explicitly unproven.
    pub(super) fn define_named_import_symbol(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        name: Ident,
        kind: SymbolKind,
        target_identity: Option<CanonicalSymbolId>,
        span: Span,
    ) -> SymbolId {
        self.validate_root_namespace(&name, span);
        let mut binding_path = canonicalize_source_module_segments(&module.segments);
        binding_path.push(item.name.clone());
        self.symbols.define_import_binding_at_path(
            Symbol {
                name,
                kind,
                span,
                scope: 0,
            },
            target_identity,
            binding_path,
        )
    }

    /// Return a stable diagnostic label for a symbol that already exists in the current local scope.
    pub(super) fn existing_local_symbol_kind(&self, name: &str) -> Option<&'static str> {
        let symbol_id = self.symbols.lookup_local(name)?;
        let symbol = self.symbols.get(symbol_id)?;
        let kind = match &symbol.kind {
            SymbolKind::Variable(_) => "const/variable",
            SymbolKind::Static(_) => "static",
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) => "function",
            SymbolKind::Type(_) => "type",
            SymbolKind::Trait(_) => "trait",
            SymbolKind::Module(_) => "imported module",
            SymbolKind::Variant(_) => "enum variant",
            SymbolKind::Field(_) => "field",
            SymbolKind::Property(_) => "property",
            SymbolKind::RustItem(_) => "rust import",
            SymbolKind::Capability(_) => "capability",
        };
        Some(kind)
    }

    /// Ensure imported items are public in the dependency module.
    ///
    /// `from M import X` and `import M::X` of a declaration `X` (a path that names no module) bind the same item, so
    /// both require `M` to export it (#1561).
    pub(super) fn validate_import_visibility(&mut self, import: &ImportDecl, span: Span) {
        let (module, item_names) = match &import.kind {
            ImportKind::From { module, items } => (
                module.clone(),
                items.iter().map(|item| item.name.clone()).collect::<Vec<_>>(),
            ),
            ImportKind::Module(path) if !self.full_path_names_a_module(path) => {
                let Some((item_name, parent_segments)) = path.segments.split_last() else {
                    return;
                };
                if parent_segments.is_empty() {
                    return;
                }
                let parent = ImportPath {
                    segments: parent_segments.to_vec(),
                    is_absolute: path.is_absolute,
                    parent_levels: path.parent_levels,
                };
                (parent, vec![item_name.clone()])
            }
            _ => return,
        };
        let module = &module;

        // Only check modules that were pre-imported; skip std and unresolved ones.
        let module_name = canonicalize_source_module_segments(&module.segments).join("_");
        let Some(exports) = self.dependency_exports.get(&module_name) else {
            return;
        };

        let mut exported_names: HashSet<String> = HashSet::new();
        for sym in exports {
            match sym {
                ExportedSymbol::Const(name)
                | ExportedSymbol::Static(name)
                | ExportedSymbol::Type(name)
                | ExportedSymbol::Trait(name)
                | ExportedSymbol::Function(name)
                | ExportedSymbol::Capability(name)
                | ExportedSymbol::Reexported(name) => {
                    exported_names.insert(name.clone());
                }
                ExportedSymbol::Variant { variant_name, .. } => {
                    exported_names.insert(variant_name.clone());
                }
            }
        }

        let exported_list: Vec<String> = exported_names.iter().cloned().collect();

        for item_name in &item_names {
            if !exported_names.contains(item_name)
                && self.dependency_member_symbol_for_path(module, item_name).is_none()
            {
                self.errors.push(errors::import_not_exported(
                    item_name,
                    &module.to_rust_path(),
                    &exported_list,
                    span,
                ));
            }
        }
    }

    /// Define a symbol for a module import through the shared binding registry.
    pub(super) fn define_import_symbol(
        &mut self,
        name: Ident,
        path: Vec<Ident>,
        is_python: bool,
        target_identity: Option<CanonicalSymbolId>,
        span: Span,
    ) {
        let symbol = Symbol {
            name,
            kind: SymbolKind::Module(ModuleInfo {
                path: path.clone(),
                is_python,
            }),
            span,
            scope: 0,
        };
        if is_python {
            self.symbols.define_import_binding(symbol, None);
        } else {
            self.symbols
                .define_import_binding_at_path(symbol, target_identity, path);
        }
    }

    /// Returns the existing symbol kind for a `from ... import ...` item when it resolves to a concrete, non-implicit
    /// symbol in the current compilation context.
    pub(super) fn existing_from_import_symbol_kind(&self, name: &str) -> Option<SymbolKind> {
        let id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(id)?;
        if Self::is_implicit_builtin_symbol(sym) {
            return None;
        }
        Some(sym.kind.clone())
    }

    /// Mark a symbol as an imported static binding when it resolves to `SymbolKind::Static`.
    ///
    /// This keeps assignment diagnostics aligned with RFC 052 (`from ... import STATIC` may read/mutate contents but
    /// must reject rebinding the imported name).
    pub(super) fn mark_static_binding_imported(&mut self, name: &str) {
        let Some(id) = self.symbols.lookup(name) else {
            return;
        };
        let mut touched_static = false;
        if let Some(sym) = self.symbols.get_mut(id)
            && let SymbolKind::Static(info) = &mut sym.kind
        {
            info.is_imported = true;
            touched_static = true;
        }
        if touched_static {
            self.type_info.declarations.static_bindings.insert(
                name.to_string(),
                crate::typechecker::StaticBindingInfo { is_imported: true },
            );
        }
    }
}
