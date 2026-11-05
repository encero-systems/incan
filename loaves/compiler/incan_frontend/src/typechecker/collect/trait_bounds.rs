//! Trait bound names and paths, derive trait adoptions, and imported and hidden trait symbols.

use super::*;

impl TypeChecker {
    /// Return the source module that owns a trait bound, including direct imports and module-qualified spellings.
    ///
    /// A trait the name reaches through no import is declared in the current module, unless it is a builtin trait: the
    /// compiler's root-scope stub for `Display` or `Eq` is declared in no module of this compilation, so a bound on it
    /// has no owning module here and its methods resolve through the builtin's standard-library declaration, as they
    /// do for a module checked without a module path. Giving it the current module left `value.__str__()` through
    /// `T with Display` without a dispatch in every project module (#1561).
    pub fn trait_bound_module_path(&self, name: &str) -> Option<Vec<String>> {
        if let Some((owner, _)) = self.checked_trait_source_target(name) {
            return Some(owner);
        }
        if let Some(path) = self.import_binding_path(name) {
            return path
                .len()
                .checked_sub(1)
                .map(|end| path[..end].to_vec())
                .filter(|segments| !segments.is_empty());
        }
        if let Some((module_name, _trait_name)) = name.rsplit_once('.') {
            return self.module_path_for_imported_name(module_name);
        }
        if let Some(target) = self.source_import_targets.get(name) {
            return Some(target.module_path.clone());
        }
        let symbol_id = self.symbols.lookup(name)?;
        let symbol = self.symbols.get(symbol_id)?;
        if !matches!(&symbol.kind, SymbolKind::Trait(_)) {
            return None;
        }
        if self
            .symbols
            .identity_of(symbol_id)
            .is_some_and(|identity| identity.origin == incan_semantics_core::SymbolOrigin::Builtin)
        {
            return None;
        }
        self.current_module_path.clone()
    }

    /// Return the defining source trait name for imported or module-qualified trait bounds.
    pub fn trait_bound_source_name(&self, name: &str) -> Option<String> {
        if let Some((_, original)) = self.checked_trait_source_target(name) {
            return Some(original);
        }
        if let Some(path) = self.import_binding_path(name) {
            return path.last().cloned();
        }
        if let Some(target) = self.source_import_targets.get(name) {
            return Some(target.name.clone());
        }
        let (_module_name, trait_name) = name.rsplit_once('.')?;
        Some(trait_name.to_string())
    }

    /// Resolve a trait bound name, installing hidden symbols for module-qualified imported traits.
    ///
    /// A trait named through a `pub::` package module binding (`t.Tag` after `import pub::tags as t`) keeps the
    /// package declaration's identity, so a bound on it is met by the package's own adopters as it is under
    /// `from pub::tags import Tag` (#1561).
    pub fn resolve_trait_bound_name(&mut self, name: &str, span: Span) -> String {
        if name.contains('.')
            && let Some((canonical, info)) = self.resolve_qualified_trait(name)
        {
            let identity = self.package_module_trait_identity(name);
            self.define_hidden_trait_symbol_with_identity(&canonical, info, identity, span);
            return canonical;
        }
        name.to_string()
    }

    /// Return the checked identity of the trait a module-qualified name names through a `pub::` package module
    /// binding, or `None` for any other spelling.
    pub(super) fn package_module_trait_identity(&mut self, name: &str) -> Option<CanonicalSymbolId> {
        let (module_name, trait_name) = name.rsplit_once('.')?;
        let module_path = self.module_path_for_imported_name(module_name)?;
        let [root, library, rest @ ..] = module_path.as_slice() else {
            return None;
        };
        if root != super::PUBLIC_LIBRARY_NAMESPACE {
            return None;
        }
        let resolved = self
            .resolve_pub_library_module_symbol_member(library, rest, trait_name)
            .ok()
            .flatten()?;
        matches!(resolved.kind, SymbolKind::Trait(_))
            .then_some(resolved.canonical)
            .flatten()
    }

    /// Retain a generic bound's foreign identity before its declaring module's imports leave scope.
    ///
    /// Source imports and checked SDK signatures carry the absolute Rust path rather than a local alias. Known
    /// non-trait items keep their binding so ordinary bound validation can reject them instead of deferring them.
    /// Source-defined traits exported by the standard prelude are materialized only when no local or imported binding
    /// already owns the name, preserving ordinary lexical shadowing.
    pub fn resolve_generic_bound_name(&mut self, name: &str, span: Span) -> String {
        if let Some(path) = self.imported_generic_rust_bound_path(name) {
            return path;
        }
        self.materialize_implicit_prelude_trait_bound(name, span);
        self.resolve_trait_bound_name(name, span)
    }

    /// Materialize a bare source-defined standard-prelude trait used as a generic bound.
    ///
    /// Compiler-owned builtin traits are installed with the root symbol table, but traits such as `Mod` are authored
    /// in `std.traits` and re-exported by `std.prelude`. Generic bounds need the full source contract so operator and
    /// method lookup can observe its hooks. An existing binding wins, so a local trait with the same spelling never
    /// acquires standard-library semantics.
    pub(super) fn materialize_implicit_prelude_trait_bound(&mut self, name: &str, span: Span) {
        if name.contains('.') || self.lookup_symbol(name).is_some() {
            return;
        }
        let prelude_path = vec!["std".to_string(), "traits".to_string(), "prelude".to_string()];
        if let Some(info) = self.lookup_imported_module_trait(&prelude_path, name) {
            self.define_hidden_trait_symbol(name, info, span);
            self.source_import_targets.insert(
                name.to_string(),
                crate::typechecker::SourceTargetInfo {
                    module_path: prelude_path,
                    name: name.to_string(),
                    kind: SemanticSourceTargetKind::Trait.to_string(),
                },
            );
        }
    }

    /// Resolve a foreign generic bound for both declaration collection and checked public export metadata.
    pub fn imported_generic_rust_bound_path(&self, name: &str) -> Option<String> {
        if let Some(symbol) = self.lookup_symbol(name)
            && let SymbolKind::RustItem(info) = &symbol.kind
            && info
                .metadata
                .as_ref()
                .is_none_or(|metadata| matches!(metadata.kind, incan_lang::interop::RustItemKind::Trait(_)))
        {
            return Some(format!("::{}", info.path.trim_start_matches("::")));
        }
        None
    }

    /// Collect synthetic trait adoptions introduced by RFC 024 `@derive(...)` arguments.
    pub fn collect_derive_trait_adoption_infos(&mut self, derives: &[String]) -> Vec<TypeBoundInfo> {
        let mut out = Vec::new();
        for derive_name in derives {
            if incan_lang::lang::derives::from_str(derive_name).is_some() {
                continue;
            }
            if let Some(module_path) = self.module_path_for_imported_name(derive_name)
                && let Some(traits) = self.lookup_derivable_traits(&module_path)
            {
                for trait_name in traits {
                    let Some(info) = self.lookup_imported_module_trait(&module_path, &trait_name) else {
                        continue;
                    };
                    let identity = self.derivable_trait_identity(&module_path, &trait_name);
                    let (owner, source_name) = identity
                        .as_ref()
                        .and_then(|identity| self.canonical_trait_source_target(identity))
                        .unwrap_or_else(|| (module_path.clone(), trait_name.clone()));
                    let canonical = format!("{}.{}", derive_name, trait_name);
                    self.define_hidden_trait_symbol_with_identity(&canonical, info, identity, Span::default());
                    if !out.iter().any(|existing: &TypeBoundInfo| {
                        existing.module_path.as_ref() == Some(&owner)
                            && Self::type_bound_source_name(existing) == source_name
                    }) {
                        out.push(TypeBoundInfo {
                            name: canonical,
                            source_name: Some(source_name),
                            type_args: Vec::new(),
                            module_path: Some(owner),
                            implementation_type_params: Vec::new(),
                            inferred: false,
                        });
                    }
                }
                continue;
            }
            let resolved = self.derive_trait_path(derive_name);
            if resolved.len() >= 2 {
                let module_segments = &resolved[..resolved.len() - 1];
                let trait_name = &resolved[resolved.len() - 1];
                if self.imported_trait_is_derivable(module_segments, trait_name)
                    && let Some(info) = self.lookup_imported_module_trait(module_segments, trait_name)
                {
                    let identity = self.derivable_trait_identity(module_segments, trait_name);
                    let (owner, source_name) = identity
                        .as_ref()
                        .and_then(|identity| self.canonical_trait_source_target(identity))
                        .unwrap_or_else(|| (module_segments.to_vec(), trait_name.clone()));
                    let canonical = derive_name.clone();
                    self.define_hidden_trait_symbol_with_identity(&canonical, info, identity, Span::default());
                    if !out.iter().any(|existing: &TypeBoundInfo| {
                        existing.module_path.as_ref() == Some(&owner)
                            && Self::type_bound_source_name(existing) == source_name
                    }) {
                        out.push(TypeBoundInfo {
                            name: canonical,
                            source_name: Some(source_name),
                            type_args: Vec::new(),
                            module_path: Some(owner),
                            implementation_type_params: Vec::new(),
                            inferred: false,
                        });
                    }
                } else if self.lookup_trait_info(derive_name).is_some() {
                    out.push(TypeBoundInfo {
                        name: derive_name.clone(),
                        source_name: None,
                        type_args: Vec::new(),
                        module_path: None,
                        implementation_type_params: Vec::new(),
                        inferred: false,
                    });
                }
            }
        }
        out
    }

    /// Resolve a derive entry to the original checked trait rather than synthesizing a declaration in its facade.
    pub(super) fn derivable_trait_identity(&self, module: &[String], name: &str) -> Option<CanonicalSymbolId> {
        self.dependency_derivable_trait_identities
            .get(&format!("{}.{name}", module.join(".")))
            .cloned()
            .or_else(|| self.dependency_member_identity(&ImportPath::simple(module.to_vec()), name))
    }

    /// Retain the declaration selected by a visible trait binding before a lexical facade spelling is recorded in a
    /// bound.
    pub(super) fn checked_trait_source_target(&self, name: &str) -> Option<(Vec<String>, String)> {
        let symbol = self.symbols.lookup(name)?;
        let identity = self.symbols.identity_of(symbol)?;
        self.canonical_trait_source_target(identity)
    }

    /// Project a checked trait identity into the source-owner form consumed by existing adoption and native-derive
    /// relations. Standard namespace ownership is checked against the admitted provider; other packages keep their
    /// explicit public package root. Equal-spelled user modules cannot acquire standard trait semantics.
    pub(in crate::typechecker) fn canonical_trait_source_target(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Option<(Vec<String>, String)> {
        if identity.kind != SemanticSourceTargetKind::Trait {
            return None;
        }
        let path = match &identity.origin {
            incan_semantics_core::SymbolOrigin::Module(path) => path.clone(),
            incan_semantics_core::SymbolOrigin::Package { library, module_path } => {
                let standard = std::iter::once("std".to_owned())
                    .chain(module_path.iter().cloned())
                    .collect::<Vec<_>>();
                if self
                    .loaves
                    .owner_of(&standard)
                    .is_some_and(|owner| owner.artifact.manifest.name == *library)
                {
                    standard
                } else {
                    [
                        vec![super::PUBLIC_LIBRARY_NAMESPACE.to_string(), library.clone()],
                        module_path.clone(),
                    ]
                    .concat()
                }
            }
            _ => return None,
        };
        (!path.is_empty()).then(|| (path, identity.declaration_name.clone()))
    }

    /// Return the module path and trait name a derive argument names, last segment the trait.
    ///
    /// A name imported on its own resolves through its import binding (`Serialize` to `std.serde.json.Serialize`); a
    /// module-qualified name resolves its module through the module's import (`json.Serialize`,
    /// `serde.json.Serialize`), as the same spelling does after `with` (#1885). A checked standard trait identity
    /// takes precedence over a facade's import path. Anything else is the name alone.
    pub fn derive_trait_path(&self, derive_name: &str) -> Vec<String> {
        if let Some(path) = self.checked_stdlib_trait_path(derive_name) {
            return path;
        }
        if let Some(path) = self.import_binding_path(derive_name) {
            return path.to_vec();
        }
        if let Some((module_name, trait_name)) = derive_name.rsplit_once('.')
            && let Some(mut path) = self.module_path_for_imported_name(module_name)
        {
            path.push(trait_name.to_string());
            return path;
        }
        vec![derive_name.to_string()]
    }

    /// Recover a standard trait's declaring path from its selected identity, including reexports through a source
    /// facade. A package must be the admitted owner of that standard module; an equal-spelled user trait grants no
    /// protocol or derive. Source shadows are read from the active symbol binding rather than an earlier import.
    pub(in crate::typechecker) fn checked_stdlib_trait_path(&self, name: &str) -> Option<Vec<String>> {
        let symbol = self.symbols.lookup(name)?;
        let identity = self.symbols.identity_of(symbol)?;
        if identity.kind != SemanticSourceTargetKind::Trait || identity.scope_discriminant.is_some() {
            return None;
        }
        let (mut path, name) = self.canonical_trait_source_target(identity)?;
        if path.first().map(String::as_str) != Some("std") {
            return None;
        }
        path.push(name);
        Some(path)
    }

    /// Resolve a module-qualified trait name through the imported-module metadata table.
    pub fn resolve_qualified_trait(&mut self, name: &str) -> Option<(String, TraitInfo)> {
        let (module_name, trait_name) = name.rsplit_once('.')?;
        let module_path = self.module_path_for_imported_name(module_name)?;
        let info = self.lookup_imported_module_trait(&module_path, trait_name)?;
        Some((name.to_string(), info))
    }

    /// Look up a module's RFC 024 `__derives__` trait list from its source module or its Loaf.
    pub fn lookup_derivable_traits(&mut self, module_path: &[String]) -> Option<Vec<String>> {
        let traits = match self.dependency_derivable_modules.get(&module_path.join(".")) {
            Some(traits) => traits.clone(),
            None => self.loaf_module_derivable_traits(module_path)?,
        };
        (!traits.is_empty()).then_some(traits)
    }

    /// Look up a trait declared by an imported module, falling back to the current scope for direct imports.
    ///
    /// The module is a project module or a module of a Loaf (`std.*` or `pub::`), so `with t.Tag` after
    /// `import pub::tags as t` names the package's own trait, or the trait a package alias of it names. A Loaf owns
    /// its modules: a trait it does not publish is not answered from anywhere else.
    pub fn lookup_imported_module_trait(&mut self, module_path: &[String], trait_name: &str) -> Option<TraitInfo> {
        let module_key = module_path.join(".");
        if let Some(info) = self.dependency_module_traits.get(&format!("{module_key}.{trait_name}")) {
            return Some(info.clone());
        }
        if self.loaf_module(module_path).is_some() {
            return self.loaf_trait(module_path, trait_name);
        }
        if let Some(SymbolKind::Trait(info)) = self.source_dependency_member_symbol_kind(module_path, trait_name) {
            return Some(info);
        }
        self.lookup_trait_info(trait_name).cloned()
    }

    /// Return whether a module-qualified trait may be adopted through `@derive(...)`: the module lists it among its
    /// derivable traits, or the trait declares the Rust derives that implement it.
    pub fn imported_trait_is_derivable(&mut self, module_path: &[String], trait_name: &str) -> bool {
        if self.lookup_imported_module_trait(module_path, trait_name).is_none() {
            return false;
        }
        let key = format!("{}.{}", module_path.join("."), trait_name);
        self.lookup_derivable_traits(module_path)
            .is_some_and(|traits| traits.iter().any(|name| name == trait_name))
            || self
                .dependency_trait_rust_derive_paths
                .get(&key)
                .is_some_and(|paths| !paths.is_empty())
            || self
                .loaf_trait_rust_derive_paths(module_path, trait_name)
                .is_some_and(|paths| !paths.is_empty())
    }

    /// Resolve an imported name or alias to a module path.
    ///
    /// A dotted name walks from its first segment: when that segment is an imported module, the remaining segments
    /// are submodules of it, so `serde.json` after `from std import serde` names `std.serde.json` (#1887). A dotted
    /// name whose first segment is not an imported module is the path it spells.
    pub fn module_path_for_imported_name(&self, name: &str) -> Option<Vec<String>> {
        if let Some((head, rest)) = name.split_once('.') {
            let mut path = self
                .lookup_symbol(head)
                .and_then(|symbol| match &symbol.kind {
                    SymbolKind::Module(info) => Some(info.path.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| vec![head.to_string()]);
            path.extend(rest.split('.').map(str::to_string));
            return Some(path);
        }
        if let Some(symbol) = self.lookup_symbol(name)
            && let SymbolKind::Module(info) = &symbol.kind
        {
            return Some(info.path.clone());
        }
        self.import_binding_path(name).map(<[String]>::to_vec)
    }

    /// Define a compiler-internal trait symbol used for qualified imported trait references.
    pub fn define_hidden_trait_symbol(&mut self, name: &str, info: TraitInfo, span: Span) {
        // RFC 120: this binding names a dependency's trait declaration; its identity is unproven here rather than
        // minted from the referencing module.
        self.define_hidden_trait_symbol_with_identity(name, info, None, span);
    }

    /// Define a compiler-internal trait symbol for a qualified imported trait reference, with the declaration's
    /// checked identity when the referenced module proves one.
    pub(super) fn define_hidden_trait_symbol_with_identity(
        &mut self,
        name: &str,
        info: TraitInfo,
        identity: Option<CanonicalSymbolId>,
        span: Span,
    ) {
        if self.symbols.lookup(name).is_some() {
            return;
        }
        self.symbols.define_import_binding(
            Symbol {
                name: name.to_string(),
                kind: SymbolKind::Trait(info),
                span,
                scope: 0,
            },
            identity,
        );
    }
}
