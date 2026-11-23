//! Rust item metadata queries, Rust path display, and native and public-library type identity.

use crate::rust_abi_cache::RustTypeAliasLookup;

use super::*;

impl TypeChecker {
    /// Return canonical Rust metadata queries successfully resolved by this checker.
    ///
    /// Library publication uses these paths in addition to explicit imports so inferred return receivers and other
    /// transitive ABI lookups survive the source-to-Loaf boundary. Failed speculative lookups are not requirements.
    pub fn observed_rust_metadata_queries(&self) -> BTreeSet<String> {
        self.native_queries.rust_metadata_queries.borrow().clone()
    }

    /// Retain a successful lookup's nominal query spelling without turning missing speculative paths into demands.
    pub(super) fn record_rust_metadata_query(&self, canonical_path: &str) {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        if let Some(lookup_path) = Self::rust_metadata_lookup_path(canonical_path) {
            let mut queries = self.native_queries.rust_metadata_queries.borrow_mut();
            if !queries.contains(lookup_path) {
                queries.insert(lookup_path.to_string());
            }
        }
    }

    /// Resolve Rust metadata through the selected shipped ABI or inspection authority and retain successful demand.
    pub fn rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let metadata = self.lookup_rust_item_metadata_for_path(canonical_path)?;
        self.record_rust_metadata_query(canonical_path);
        Some(metadata)
    }

    /// Return Rust item metadata from the shipped ABI of the Loaves the checker reaches.
    pub(super) fn lookup_rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        let lookup_path = Self::rust_metadata_lookup_path(canonical_path)?;
        self.loaves.rust_abi_item(lookup_path)
    }

    /// Return Rust item metadata from the shipped ABI of the Loaves the checker reaches.
    pub fn rust_item_metadata_for_method_call(
        &self,
        canonical_path: &str,
        _method: &str,
    ) -> Option<Arc<RustItemMetadata>> {
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Return complete type metadata from the shipped ABI of the Loaves the checker reaches.
    pub fn rust_item_metadata_for_complete_type(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        #[cfg(test)]
        self.native_queries
            .rust_complete_type_lookup_requests
            .set(self.native_queries.rust_complete_type_lookup_requests.get() + 1);
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Split top-level generic arguments from a Rust display type.
    pub(super) fn split_top_level_generic_args(args: &str) -> Vec<&str> {
        split_top_level_rust_args(args)
    }

    /// Normalize a Rust metadata lookup path down to the nominal item path.
    ///
    /// rust-analyzer metadata is keyed by the item path (`foo::Bar`), not by instantiated spellings like
    /// `foo::Bar<T>` or placeholder displays like `{unknown}`. This strips outer generic instantiation from a Rust
    /// path while rejecting obviously non-item spellings before hitting the metadata cache/extractor.
    pub(super) fn rust_metadata_lookup_path(canonical_path: &str) -> Option<&str> {
        let trimmed = canonical_path.trim();
        if trimmed == "[]" {
            return Some(trimmed);
        }
        if trimmed.is_empty() || trimmed == "{unknown}" {
            return None;
        }
        let had_generics = trimmed.contains('<');
        let base = trimmed.split_once('<').map_or(trimmed, |(base, _)| base);
        if had_generics && !base.contains("::") {
            return None;
        }
        if base.is_empty() || base.contains(['{', '}', '(', ')', '[', ']', ',', ' ']) {
            return None;
        }
        Some(base)
    }

    /// Strip the synthetic `rust::` namespace prefix used in Incan source paths.
    ///
    /// Canonical Rust metadata item keys are crate-rooted paths (`crate_name::...`), so compatibility checks and
    /// metadata lookups should normalize away the language surface namespace first.
    pub(super) fn normalize_rust_namespace_path(path: &str) -> &str {
        path.strip_prefix("rust::").unwrap_or(path)
    }

    /// Return the inspected Rust owner path for a synthetic `rust::Type.method` call display.
    ///
    /// Method diagnostics retain the callable spelling, but metadata signatures resolve `crate::`, `self::`, and
    /// `super::` relative to the receiver type's module, never relative to the synthetic method segment.
    pub fn rust_method_owner_path(callable_display: &str) -> &str {
        let path = Self::normalize_rust_namespace_path(callable_display);
        path.rsplit_once('.').map_or(path, |(owner, _method)| owner)
    }

    /// Split a Rust display into its nominal path and the Incan-visible generic type arguments.
    ///
    /// Rust lifetimes are erased at the language boundary, while nested type arguments keep their normal display
    /// form and are resolved for nominal comparison. The returned base is also normalized from Incan's synthetic
    /// `rust::` namespace to the inspector's crate-rooted path.
    pub(super) fn rust_path_base_and_args(&self, path: &str) -> (String, Vec<ResolvedType>) {
        let trimmed = path.trim();
        if let Some(start) = trimmed.find('<')
            && trimmed.ends_with('>')
        {
            let base = Self::normalize_rust_namespace_path(trimmed[..start].trim()).to_string();
            let inner = &trimmed[start + 1..trimmed.len() - 1];
            let args = Self::split_top_level_generic_args(inner)
                .into_iter()
                .filter(|arg| !Self::rust_generic_arg_is_lifetime(arg))
                .map(|arg| self.resolved_type_from_rust_display(arg))
                .collect();
            return (base, args);
        }
        (Self::normalize_rust_namespace_path(trimmed).to_string(), Vec::new())
    }

    /// Return whether a Rust generic argument is a lifetime, which Incan deliberately does not model as a type.
    ///
    /// Rust metadata may qualify an inferred lifetime through the owner module (for example, `crate::'_`). Preserve
    /// real type arguments while eliding both plain and qualified lifetime placeholders before nominal comparison.
    pub(crate) fn rust_generic_arg_is_lifetime(arg: &str) -> bool {
        let candidate = arg.trim().rsplit("::").next().unwrap_or(arg.trim());
        let Some(name) = candidate.strip_prefix('\'') else {
            return false;
        };
        !name.is_empty() && name.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    }

    /// Rewrite Rust's crate-relative type displays against the inspected callable or type path that produced them.
    ///
    /// rust-analyzer reports signatures from inside a Rust crate using displays such as
    /// `crate::ScalarFunctionImplementation`. Incan source, imports, and generated Rust refer to the dependency by its
    /// crate name (`datafusion_expr::ScalarFunctionImplementation`), so boundary typing must canonicalize those
    /// displays before compatibility checks or contextual argument checking run.
    pub fn rust_display_for_owner_path(&self, rust_ty: &str, owner_path: &str) -> String {
        let owner = Self::normalize_rust_namespace_path(owner_path);
        let owner_definition = self
            .rust_definition_for_path(owner)
            .unwrap_or_else(|| owner.to_string());
        let owner_segments = owner_definition.split("::").collect::<Vec<_>>();
        let Some(crate_name) = owner_segments.first().filter(|segment| !segment.is_empty()) else {
            return rust_ty.to_string();
        };
        let replacement = format!("{crate_name}::");
        let mut rendered = String::with_capacity(rust_ty.len() + replacement.len());
        let mut remaining = rust_ty;
        while let Some(idx) = remaining.find("crate::") {
            let (before, after) = remaining.split_at(idx);
            rendered.push_str(before);
            let prev = rendered.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
                rendered.push_str("crate::");
            } else if after["crate::".len()..].starts_with('[') {
                // rust-analyzer can render a slice nested in a generic callback bound as `crate::[T]`. A slice is
                // not a crate item, so preserve its Rust spelling instead of producing the invalid `dep::[T]`.
            } else {
                rendered.push_str(replacement.as_str());
            }
            remaining = &after["crate::".len()..];
        }
        rendered.push_str(remaining);

        let owner_module = &owner_segments[..owner_segments.len().saturating_sub(1)];
        let self_replacement = owner_module.join("::");
        let mut self_expanded = String::with_capacity(rendered.len() + self_replacement.len());
        let mut remaining = rendered.as_str();
        while let Some(idx) = remaining.find("self::") {
            let (before, after) = remaining.split_at(idx);
            self_expanded.push_str(before);
            let prev = self_expanded.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) || self_replacement.is_empty() {
                self_expanded.push_str("self::");
            } else {
                self_expanded.push_str(self_replacement.as_str());
                self_expanded.push_str("::");
            }
            remaining = &after["self::".len()..];
        }
        self_expanded.push_str(remaining);

        let owner_module_prefix = (!owner_module.is_empty()).then(|| format!("{}::", owner_module.join("::")));
        let mut super_expanded = String::with_capacity(self_expanded.len() + owner_definition.len());
        let mut remaining = self_expanded.as_str();
        while let Some(idx) = remaining.find("super::") {
            let (before, after) = remaining.split_at(idx);
            super_expanded.push_str(before);
            let mut rest = after;
            let mut parent_count = 0usize;
            while let Some(next) = rest.strip_prefix("super::") {
                parent_count += 1;
                rest = next;
            }
            // rust-analyzer can qualify a relative path with the method owner's module, for example
            // `datafusion::execution::context::super::options::...`. The prefix is context for the relative
            // segment, not part of the resulting type path, so remove it before replacing `super` with its parent.
            if let Some(prefix) = owner_module_prefix.as_deref()
                && super_expanded.ends_with(prefix)
            {
                super_expanded.truncate(super_expanded.len() - prefix.len());
            }
            let prev = super_expanded.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) || parent_count >= owner_module.len() {
                super_expanded.push_str(&after[..after.len() - rest.len()]);
            } else {
                super_expanded.push_str(owner_module[..owner_module.len() - parent_count].join("::").as_str());
                super_expanded.push_str("::");
            }
            remaining = rest;
        }
        super_expanded.push_str(remaining);
        super_expanded
    }

    /// Return already-attached or cache-only metadata for one canonical Rust identity path.
    pub(super) fn known_rust_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        if let Some(metadata) = self.symbols.all_symbols().iter().find_map(|sym| {
            let SymbolKind::RustItem(info) = &sym.kind else {
                return None;
            };
            if Self::normalize_rust_namespace_path(info.path.as_str()) != canonical_path {
                return None;
            }
            info.metadata.clone()
        }) {
            return Some(metadata);
        }
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Return alias-only metadata from an attached item or directly from the ABI cache's record projection.
    ///
    /// Attached metadata is already owned for another checker operation. A cache-only request must not expand an
    /// entire type surface merely to learn whether it has an alias target.
    pub(super) fn known_rust_type_alias_for_path(&self, canonical_path: &str) -> Option<RustTypeAliasLookup> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        if let Some(metadata) = self.symbols.all_symbols().iter().find_map(|sym| {
            let SymbolKind::RustItem(info) = &sym.kind else {
                return None;
            };
            if Self::normalize_rust_namespace_path(info.path.as_str()) != canonical_path {
                return None;
            }
            info.metadata.as_deref()
        }) {
            return Some(RustTypeAliasLookup::from_item(metadata));
        }
        self.loaves.rust_type_alias(canonical_path)
    }

    /// Return the Rust definition metadata for a canonical path.
    pub(super) fn rust_definition_for_path(&self, canonical_path: &str) -> Option<String> {
        self.known_rust_metadata_for_path(canonical_path)
            .and_then(|metadata| metadata.definition_path.clone())
    }

    /// Resolve inspected default type arguments without inventing defaults for metadata that omitted them.
    pub(super) fn rust_type_param_defaults(&self, metadata: Option<&RustItemMetadata>) -> Vec<Option<ResolvedType>> {
        let Some(RustItemMetadata {
            kind: RustItemKind::Type(type_info),
            ..
        }) = metadata
        else {
            return Vec::new();
        };
        type_info
            .type_param_defaults
            .iter()
            .map(|default| {
                default
                    .as_deref()
                    .map(|default| self.resolved_type_from_rust_display(default))
            })
            .collect()
    }

    /// Extract the cheap Rust identity already known to the checker for compatibility checks.
    ///
    /// This must stay metadata-light. `types_compatible(...)` calls it frequently, so it may only use symbol-local
    /// metadata already attached during import collection plus cache-only Rust ABI reads. Fresh
    /// rustc extraction from this path would leak a heavy metadata-loading concern into ordinary semantic
    /// checks. Transparent rusttypes and Rust aliases use their retained native targets before identity comparison.
    pub(super) fn rust_identity_for_type(&self, ty: &ResolvedType) -> Option<RustTypeIdentity> {
        if let Some(target) = self.rusttype_native_target(ty) {
            return self.rust_identity_for_type(&target);
        }
        match ty {
            ResolvedType::RustPath(path) => {
                let (base, args) = self.rust_path_base_and_args(path);
                let metadata = self.known_rust_metadata_for_path(base.as_str());
                if metadata.as_ref().is_some_and(
                    |metadata| matches!(&metadata.kind, RustItemKind::Type(info) if info.alias_target.is_some()),
                ) {
                    let displays = args.iter().map(render_resolved_type_as_rust_arg).collect::<Vec<_>>();
                    let arguments = displays.iter().map(String::as_str).collect::<Vec<_>>();
                    if let Some(target) = self.rust_alias_expansion(&base, &arguments)
                        && target != *path
                    {
                        return self.rust_identity_for_type(&ResolvedType::RustPath(target));
                    }
                }
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((base, definition, args, defaults))
            }
            ResolvedType::Named(name) => {
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let metadata = info
                    .metadata
                    .clone()
                    .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    Vec::new(),
                    defaults,
                ))
            }
            ResolvedType::Generic(name, args) => {
                if name.contains("::") {
                    let arguments = args
                        .iter()
                        .map(render_resolved_type_as_rust_arg)
                        .collect::<Vec<_>>()
                        .join(", ");
                    return self.rust_identity_for_type(&ResolvedType::RustPath(format!("{name}<{arguments}>")));
                }
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let metadata = info
                    .metadata
                    .clone()
                    .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    args.clone(),
                    defaults,
                ))
            }
            _ => None,
        }
    }

    /// Resolve a bare Rust display name through imported Rust items when the local scope makes that identity unique.
    ///
    /// Rust metadata can report field and payload displays relative to the Rust module that owns the item, for example
    /// a field displayed as `Payload` while Incan imported the same Rust type as `DemoPayload`. Source aliases are
    /// display projections; the Rust boundary still needs the canonical Rust identity so local/import/reexport
    /// paths do not diverge.
    pub(super) fn unique_imported_rust_item_path_for_display_tail(&self, display_tail: &str) -> Option<String> {
        if display_tail.is_empty() {
            return None;
        }
        // A qualified display names an item outright. Match it against the imported items by ancestral identity so
        // metadata's `alloc::boxed::Box` finds a source-level `from rust::std::boxed import Box`, and hand back the
        // import's own spelling so later nominal checks line up with the symbol the source actually bound.
        if display_tail.contains("::") {
            let wanted = incan_lang::interop::ancestral_rust_path(display_tail);
            let mut matches = HashSet::new();
            for symbol in self.symbols.all_symbols() {
                let SymbolKind::RustItem(info) = &symbol.kind else {
                    continue;
                };
                if matches!(info.binding, RustImportBindingKind::CrateRoot) {
                    continue;
                }
                let path = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
                if incan_lang::interop::ancestral_rust_path(path.as_str()) == wanted {
                    matches.insert(path);
                }
            }
            let mut iter = matches.into_iter();
            let first = iter.next()?;
            return if iter.next().is_none() { Some(first) } else { None };
        }
        let mut matches = HashSet::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            if matches!(info.binding, RustImportBindingKind::CrateRoot) {
                continue;
            }
            let path = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
            if path.rsplit("::").next() == Some(display_tail) {
                matches.insert(path);
                continue;
            }
            if let Some(metadata) = info.metadata.as_ref() {
                let canonical_path = Self::normalize_rust_namespace_path(metadata.canonical_path.as_str()).to_string();
                if canonical_path.rsplit("::").next() == Some(display_tail) {
                    matches.insert(canonical_path);
                }
            }
        }
        let mut iter = matches.into_iter();
        let first = iter.next()?;
        if iter.next().is_none() { Some(first) } else { None }
    }

    /// Resolve a source-level rusttype backing type to its canonical Rust path spelling.
    pub fn rust_path_for_rusttype_underlying(&self, ty: &ResolvedType) -> Option<String> {
        if let ResolvedType::RustPath(path) = ty {
            return Some(path.clone());
        }

        let (base, _definition, args, _defaults) = self.rust_identity_for_type(ty)?;
        if args.is_empty() {
            return Some(base);
        }

        let rendered_args = args
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{base}<{rendered_args}>"))
    }

    /// Bind a nominal declaration to the exact artifact already admitted for this consumer dependency route.
    ///
    /// `source_path` is the path the caller reached the type through, which for a re-export is the hop it forwards
    /// through. When the library's identity graph publishes the declaration `canonical` names, the identity carries
    /// the declaration's own source path instead, so every public path of one type yields one identity.
    pub(super) fn admitted_public_type_identity(
        &self,
        library: &str,
        source_path: &[String],
        canonical: Option<CanonicalSymbolId>,
    ) -> PublicLibraryTypeIdentity {
        let declaration_path = match (&canonical, self.loaves.manifest(library)) {
            (Some(canonical), Some(manifest)) => manifest
                .contract_metadata
                .identity_graph
                .declaration_source_path(canonical),
            _ => None,
        };
        let mut identity =
            PublicLibraryTypeIdentity::new(library, declaration_path.unwrap_or(source_path)).with_canonical(canonical);
        // The declaring Loaf of a re-exported type may be another Loaf than the one it was imported through.
        identity.selected_provider = match &identity.canonical {
            Some(canonical) => self.loaves.declaring_loaf(canonical),
            None => self.loaves.entry(library),
        }
        .map(|entry| entry.artifact.identity.clone());
        identity
    }

    /// Retain foreign nominal origins from accepted import bindings, without looking up source names again.
    pub fn checked_nominal_type_origins(
        &self,
    ) -> std::collections::BTreeMap<String, crate::library_manifest::NominalTypeOriginExport> {
        self.public_library_type_identities
            .iter()
            .filter_map(|(name, identity)| {
                let provider = identity.selected_provider.as_ref()?;
                let canonical = identity.canonical.as_ref()?;
                let incan_semantics_core::SymbolOrigin::Package { library, .. } = &canonical.origin else {
                    return None;
                };
                if library != &provider.name {
                    return None;
                }
                Some((
                    name.clone(),
                    crate::library_manifest::NominalTypeOriginExport {
                        provider: provider.clone(),
                        canonical: crate::library_manifest::CanonicalIdentityExport::from_canonical(
                            library, canonical,
                        )?,
                    },
                ))
            })
            .collect()
    }

    /// Return the canonical public-library identity and generic arguments carried by one named type or trait spelling.
    pub(super) fn public_library_type_identity_for_type<'a>(
        &'a self,
        ty: &'a ResolvedType,
    ) -> Option<(&'a PublicLibraryTypeIdentity, &'a [ResolvedType])> {
        match ty {
            ResolvedType::Named(name) => self
                .public_library_type_identities
                .get(name)
                .map(|identity| (identity, &[][..])),
            ResolvedType::Generic(name, args) => self
                .public_library_type_identities
                .get(name)
                .map(|identity| (identity, args.as_slice())),
            _ => None,
        }
    }

    /// Compare provider-qualified public-library identities before ordinary nominal spelling equality.
    ///
    /// Returning `None` leaves non-provider types and distinct trait-related declarations on the ordinary compatibility
    /// path, where adoption and supertrait relations are checked. Equal provider identities compare their generic
    /// arguments recursively; distinct nominal owners remain incompatible.
    pub(super) fn public_library_type_identities_compatible(
        &self,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> Option<bool> {
        let (actual_identity, actual_args) = self.public_library_type_identity_for_type(actual)?;
        let (expected_identity, expected_args) = self.public_library_type_identity_for_type(expected)?;
        if actual_identity != expected_identity {
            if [actual_identity, expected_identity].iter().any(|identity| {
                identity
                    .canonical
                    .as_ref()
                    .is_some_and(|canonical| canonical.kind == SemanticSourceTargetKind::Trait)
            }) {
                return None;
            }
            return Some(false);
        }
        if actual_args.len() != expected_args.len() {
            return Some(false);
        }
        Some(
            actual_args
                .iter()
                .zip(expected_args)
                .all(|(actual_arg, expected_arg)| self.types_compatible(actual_arg, expected_arg)),
        )
    }

    /// Return whether two Rust type identities describe the same boundary type.
    pub(super) fn rust_type_identities_compatible(
        &self,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> Option<bool> {
        if let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = expected
            && let Some(matches) = self.rust_type_identities_compatible(actual, inner)
        {
            return Some(matches);
        }
        let (actual_path, actual_def, actual_args, actual_defaults) = self.rust_identity_for_type(actual)?;
        let (expected_path, expected_def, expected_args, expected_defaults) = self.rust_identity_for_type(expected)?;
        let same_base = actual_path == expected_path;
        let same_definition =
            actual_def.is_some() && expected_def.is_some() && actual_def.as_ref() == expected_def.as_ref();
        let actual_resolves_to_expected = actual_def.as_deref() == Some(expected_path.as_str());
        let expected_resolves_to_actual = expected_def.as_deref() == Some(actual_path.as_str());
        if !same_base && !same_definition && !actual_resolves_to_expected && !expected_resolves_to_actual {
            // Without concrete definition metadata for both sides, cross-crate Rust paths can still be equivalent
            // through re-exports. Keep the Rust-path boundary permissive instead of reporting a false mismatch.
            if actual_def.is_none() || expected_def.is_none() {
                return None;
            }
            return Some(false);
        }
        let actual_args = self.rust_type_args_with_defaults(actual_args, actual_defaults);
        let expected_args = self.rust_type_args_with_defaults(expected_args, expected_defaults);
        Some(self.rust_type_args_compatible(actual_args.as_slice(), expected_args.as_slice()))
    }

    /// Fill only a contiguous trailing run of inspected Rust default type arguments.
    pub(super) fn rust_type_args_with_defaults(
        &self,
        mut args: Vec<ResolvedType>,
        defaults: Vec<Option<ResolvedType>>,
    ) -> Vec<ResolvedType> {
        for default in defaults.into_iter().skip(args.len()) {
            let Some(default) = default else {
                break;
            };
            args.push(default);
        }
        args
    }

    /// Return whether Rust generic type arguments are compatible.
    pub(super) fn rust_type_args_compatible(
        &self,
        actual_args: &[ResolvedType],
        expected_args: &[ResolvedType],
    ) -> bool {
        if actual_args.len() != expected_args.len() {
            return (actual_args.is_empty() && expected_args.iter().all(Self::rust_type_arg_is_unknown_placeholder))
                || (expected_args.is_empty() && actual_args.iter().all(Self::rust_type_arg_is_unknown_placeholder));
        }
        actual_args.iter().zip(expected_args.iter()).all(|(actual, expected)| {
            Self::rust_type_arg_is_unknown_placeholder(actual)
                || Self::rust_type_arg_is_unknown_placeholder(expected)
                || self.types_compatible(actual, expected)
        })
    }

    /// Return whether a Rust type argument is an unknown placeholder.
    pub(super) fn rust_type_arg_is_unknown_placeholder(arg: &ResolvedType) -> bool {
        match arg {
            ResolvedType::Unknown => true,
            ResolvedType::RustPath(path) => {
                matches!(
                    path.trim().as_bytes(),
                    [b'?'] | [b'{', b'u', b'n', b'k', b'n', b'o', b'w', b'n', b'}']
                )
            }
            _ => false,
        }
    }

    /// Whether a Rust signature parameter is the implicit receiver (`self`/`&self`/`&mut self`).
    pub fn rust_param_is_receiver(param: &RustParam) -> bool {
        if param.name.as_deref() == Some("self") {
            return true;
        }
        let normalized = param.type_display.replace(' ', "");
        matches!(
            normalized.as_str(),
            "self" | "&self" | "&mutself" | "Self" | "&Self" | "&mutSelf"
        )
    }

    /// Whether a Rust function signature starts with an implicit receiver parameter.
    pub fn rust_signature_has_receiver(sig: &RustFunctionSig) -> bool {
        sig.params.first().is_some_and(Self::rust_param_is_receiver)
    }
}
