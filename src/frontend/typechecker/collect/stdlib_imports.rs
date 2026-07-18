//! Stdlib-aware import collection and namespace validation.
//!
//! This keeps stdlib import enforcement (RFC 022) separate from general declaration collection while preserving the
//! existing behavior.

use std::collections::{HashMap, HashSet};

use crate::frontend::api_metadata::{
    ApiDeclaration, class_export_from_api, enum_export_from_api, function_export_from_api,
    function_export_from_api_projected, model_export_from_api, newtype_export_from_api, partial_export_from_api,
    trait_export_from_api,
};
use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::library_manifest_index::{LibraryManifestFailureKind, LibraryManifestIndexEntry};
use crate::frontend::module::{ExportedSymbol, canonicalize_source_module_segments};
use crate::frontend::symbols::*;
use crate::frontend::testing_markers::{
    TestingMarkerLoadError, TestingMarkerSemantics, load_testing_marker_semantics,
    testing_marker_semantics_from_manifest,
};
use crate::frontend::typechecker::type_info::RustTraitImportInfo;
use crate::frontend::typechecker::{
    PartialProjectionInfo, PartialProjectionPreset, PartialProjectionTargetKind, TypeChecker,
};
use crate::library_manifest::{
    AliasExport, ClassExport, ConstExport, EnumExport, EnumValueExport, EnumValueTypeExport, FieldExport,
    FunctionExport, LibraryManifest, MethodExport, ModelExport, NewtypeExport, ParamDefaultExport, ParamExport,
    ParamKindExport, PartialExport, PartialTargetKindExport, PresetValueExport, ProviderFactKind, ReceiverExport,
    StaticExport, TraitExport, TypeAliasExport, TypeBoundExport, TypeParamExport, resolved_type_from_manifest_type_ref,
};
use crate::provider::{ProviderModuleResolution, ProviderProvenance};
use incan_core::interop::{RustItemKind, RustTraitAssoc, fallback_rust_trait_methods, is_rust_capability_bound};
use incan_core::lang::stdlib::{self, is_typechecker_only_stdlib};
use incan_core::lang::surface::functions as surface_functions;
use incan_core::lang::surface::types as surface_types;
use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

enum ManifestExportRef<'a> {
    Alias(&'a AliasExport),
    Model(&'a ModelExport),
    Class(&'a ClassExport),
    Function(&'a FunctionExport),
    Partial(&'a PartialExport),
    Trait(&'a TraitExport),
    Enum(&'a EnumExport),
    EnumVariant {
        enum_name: &'a str,
        fields: &'a [crate::library_manifest::TypeRef],
    },
    TypeAlias(&'a TypeAliasExport),
    Newtype(&'a NewtypeExport),
    Const(&'a ConstExport),
    Static(&'a StaticExport),
}

/// Return the project dependency or SDK component name that owns a provider diagnostic remedy.
fn sdk_provider_component_id(provenance: &ProviderProvenance) -> &str {
    match provenance {
        ProviderProvenance::Sdk { component_id, .. } => component_id,
        ProviderProvenance::ProjectDependency { dependency_key, .. } => dependency_key,
        ProviderProvenance::Compiler => "compiler",
    }
}

/// Return the package or SDK identity used to explain provider authority in diagnostics.
fn sdk_provider_identity(provenance: &ProviderProvenance) -> &str {
    match provenance {
        ProviderProvenance::Sdk { sdk_identity, .. } => sdk_identity,
        ProviderProvenance::ProjectDependency { dependency_key, .. } => dependency_key,
        ProviderProvenance::Compiler => "compiler",
    }
}

/// Classified context for a `from ... import ...` declaration during first-pass collection.
///
/// This keeps stdlib namespace decisions close to the parsed module path while leaving concrete item materialization to
/// helpers that can return "not handled" and preserve the ordinary imported-module fallback.
struct FromImportContext<'a> {
    module: &'a ImportPath,
    stdlib: Option<StdlibFromImportContext>,
}

impl<'a> FromImportContext<'a> {
    /// Classify one parsed from-import module path for namespace validation and stdlib import materialization.
    fn new(module: &'a ImportPath, is_known_stdlib_module: bool, provider_owned: bool) -> Self {
        Self {
            module,
            stdlib: StdlibFromImportContext::new(module, is_known_stdlib_module, provider_owned),
        }
    }

    /// Return `true` when this from-import references an unknown stdlib module that should emit the RFC 022 diagnostic.
    fn is_unknown_stdlib_module(&self) -> bool {
        self.stdlib.as_ref().is_some_and(|stdlib| stdlib.is_unknown_module)
    }

    /// Return `true` when an unmaterialized import item from this context must be rejected instead of falling back.
    fn rejects_unmaterialized_stdlib_items(&self) -> bool {
        self.stdlib.as_ref().is_some_and(|stdlib| !stdlib.is_unknown_module)
    }

    /// Join the source module segments as the user-facing dotted stdlib path.
    fn dotted_module_path(&self) -> String {
        self.module.segments.join(".")
    }
}

/// Stdlib-specific classification for unqualified `from std... import ...` paths.
///
/// `incan_core::lang::stdlib` remains the source of truth for known modules; this struct only snapshots the repeated
/// predicates needed while collecting individual import items.
struct StdlibFromImportContext {
    module_path_str: String,
    is_unknown_module: bool,
    is_web_namespace: bool,
    is_async_namespace: bool,
    is_reflection_module: bool,
    is_testing_module: bool,
    has_stub: bool,
}

impl StdlibFromImportContext {
    /// Build stdlib classification for an unqualified `std...` module path.
    fn new(module: &ImportPath, is_known_module: bool, provider_owned: bool) -> Option<Self> {
        if module.parent_levels != 0 || module.is_absolute || !stdlib::is_any_stdlib_path(&module.segments) {
            return None;
        }

        let module_path_str = module.segments.join(".");
        let is_web_namespace = module.segments.len() >= 2
            && module.segments[0] == stdlib::STDLIB_ROOT
            && module.segments[1] == stdlib::STDLIB_WEB;
        let is_async_namespace =
            module.segments.len() >= 2 && module.segments[0] == stdlib::STDLIB_ROOT && module.segments[1] == "async";
        let is_reflection_module = module.segments.len() == 2
            && module.segments[0] == stdlib::STDLIB_ROOT
            && module.segments[1] == "reflection";
        let is_testing_module =
            module.segments.len() == 2 && module.segments[0] == stdlib::STDLIB_ROOT && module.segments[1] == "testing";
        let has_stub = is_known_module && !provider_owned && stdlib::stdlib_stub_path(&module.segments).is_some();

        Some(Self {
            module_path_str,
            is_unknown_module: !is_known_module,
            is_web_namespace,
            is_async_namespace,
            is_reflection_module,
            is_testing_module,
            has_stub,
        })
    }

    /// Return the imported surface type when it is legal from this stdlib module.
    fn allowed_surface_type_import(&self, item_name: &str) -> Option<surface_types::SurfaceTypeId> {
        let id = surface_types::from_str(item_name)?;
        let expected_module_path = surface_types::stdlib_module_path(id)?;

        let allowed = match expected_module_path {
            "std.web" => self.is_web_namespace,
            "std.reflection" => self.is_reflection_module,
            _ if expected_module_path.starts_with("std.async.") => {
                let async_root_or_prelude =
                    self.module_path_str == "std.async" || self.module_path_str == "std.async.prelude";
                self.is_async_namespace && (async_root_or_prelude || self.module_path_str == expected_module_path)
            }
            _ => false,
        };
        allowed.then_some(id)
    }
}

impl TypeChecker {
    /// Reject names that shadow reserved root namespaces.
    pub(super) fn validate_root_namespace(&mut self, name: &str, span: Span) {
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
            ImportKind::PubLibrary { library } => {
                self.collect_pub_library_import(library, import.alias.as_ref(), span);
            }
            ImportKind::PubFrom { library, items } => {
                self.collect_pub_imports(library, items, span);
            }
            ImportKind::Python(pkg) => {
                let name = import.alias.clone().unwrap_or_else(|| pkg.clone());
                self.validate_root_namespace(&name, span);
                self.define_import_symbol(name, vec![pkg.clone()], true, span);
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

    /// Collect a plain module import, including stdlib namespace validation.
    fn collect_module_import(&mut self, path: &ImportPath, alias: Option<&Ident>, span: Span) {
        if let Some(error) = self.sdk_provider_module_error(&path.segments, span) {
            self.errors.push(error);
            return;
        }
        // Reject `import std.f64.consts` - unknown stdlib module; suggest `import rust::std::f64::consts`.
        if stdlib::is_any_stdlib_path(&path.segments) && !self.is_known_stdlib_module(&path.segments) {
            self.errors
                .push(errors::unknown_stdlib_module(&path.segments.join("."), span));
        }

        let name = alias
            .cloned()
            .unwrap_or_else(|| path.segments.last().cloned().unwrap_or_else(|| "module".to_string()));
        // Allow `import std.web as std` (alias matches source root), but reject `import std.web as rust` (alias is a
        // different reserved root).
        let same_root = path.segments.first().map(|segment| segment.as_str()) == Some(&name);
        if !same_root {
            self.validate_root_namespace(&name, span);
        }
        let normalized_path = canonicalize_source_module_segments(&path.segments);
        self.define_import_symbol(name, normalized_path, false, span);
    }

    /// Collect a `from module import item, ...` declaration as concrete stdlib/dependency symbols when possible,
    /// otherwise as module-path placeholders.
    fn collect_from_imports(&mut self, module: &ImportPath, items: &[ImportItem], span: Span) {
        if let Some(error) = self.sdk_provider_module_error(&module.segments, span) {
            self.errors.push(error);
            return;
        }
        // Manifestless in-memory plans are the explicit source-backed compiler-test adapter exposed by
        // `set_sdk_provider_module_paths`; installed and package consumers always resolve checked manifests here.
        let provider_owned = matches!(
            self.provider_plan.resolve_module(&module.segments),
            ProviderModuleResolution::Active(provider) if provider.manifest.is_some()
        );
        let context = FromImportContext::new(module, self.is_known_stdlib_module(&module.segments), provider_owned);
        if context.is_unknown_stdlib_module() {
            self.errors
                .push(errors::unknown_stdlib_module(&context.dotted_module_path(), span));
        }

        let testing_semantics = self.load_testing_semantics_for_import(&context, span);
        self.cache_stdlib_stub_semantics(&context);

        for item in items {
            if self.materialize_stdlib_from_import(&context, item, testing_semantics.as_ref(), span) {
                continue;
            }
            if context.rejects_unmaterialized_stdlib_items() {
                self.errors.push(errors::stdlib_import_not_exported(
                    &item.name,
                    &context.dotted_module_path(),
                    span,
                ));
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

    /// Materialize one imported source dependency, preserving package-private implementation facts during provider
    /// construction before the published checked facade becomes the consumer authority.
    fn materialize_source_dependency_import(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        let Some(kind) = self.imported_source_dependency_symbol_kind(module, item) else {
            return false;
        };
        let projection = self.imported_source_dependency_partial_projection(module, item);
        self.define_resolved_source_import_symbol(module, item, kind, projection, span);
        true
    }

    /// Resolve module ownership from the active SDK catalog, retaining only explicit compiler-owned legacy surfaces.
    fn is_known_stdlib_module(&self, module: &[String]) -> bool {
        match self.provider_plan.resolve_module(module) {
            ProviderModuleResolution::Active(_)
            | ProviderModuleResolution::Disabled(_)
            | ProviderModuleResolution::Unavailable(_) => true,
            ProviderModuleResolution::Unknown if self.provider_plan.has_sdk_catalog() => {
                is_typechecker_only_stdlib(module)
                    || (self.provider_plan.bootstrap_owns_sdk_module(module) && stdlib::is_known_stdlib_module(module))
            }
            ProviderModuleResolution::Unknown => stdlib::is_known_stdlib_module(module),
        }
    }

    /// Preserve distinct remedies for known-but-disabled and enabled-but-unavailable SDK provider modules.
    fn sdk_provider_module_error(
        &self,
        module: &[String],
        span: Span,
    ) -> Option<crate::frontend::diagnostics::CompileError> {
        if module.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
            return None;
        }
        let dotted = module.join(".");
        match self.provider_plan.resolve_module(module) {
            ProviderModuleResolution::Disabled(provider) => Some(errors::sdk_component_disabled(
                &dotted,
                sdk_provider_component_id(&provider.provenance),
                span,
            )),
            ProviderModuleResolution::Unavailable(provider) => Some(errors::sdk_component_unavailable(
                &dotted,
                sdk_provider_component_id(&provider.provenance),
                sdk_provider_identity(&provider.provenance),
                span,
            )),
            ProviderModuleResolution::Active(_) | ProviderModuleResolution::Unknown => None,
        }
    }

    /// Cache all known top-level types and traits for a stub-backed stdlib module without making them source-visible.
    fn cache_stdlib_stub_semantics(&mut self, context: &FromImportContext<'_>) {
        if !context.stdlib.as_ref().is_some_and(|stdlib| stdlib.has_stub) {
            return;
        }

        for (type_name, type_info) in self.stdlib_cache.list_types(&context.module.segments) {
            self.transitive_stdlib_stub_types.entry(type_name).or_insert(type_info);
        }
        for (trait_name, trait_info) in self.stdlib_cache.list_traits(&context.module.segments) {
            self.transitive_stdlib_stub_traits
                .entry(trait_name)
                .or_insert(trait_info);
        }
    }

    /// Seed exact module-member and transitive semantic caches from active SDK providers.
    ///
    /// The artifact stores checked API metadata with paths relative to its own crate root (`environ`, `serde.json`,
    /// and so on). Consumers continue to spell those modules as `std.environ` and `std.serde.json`, so this method
    /// restores the public `std` prefix only in the consumer-side lookup keys. No provider source AST is loaded here.
    pub(crate) fn seed_sdk_provider_symbols(&mut self) {
        let providers = self
            .provider_plan
            .active_sdk_records()
            .filter_map(|provider| {
                let manifest = provider.manifest.clone()?;
                let api = manifest.contract_metadata.api.clone()?;
                Some((provider.namespace_claims.clone(), api))
            })
            .collect::<Vec<_>>();
        let mut seeded_modules = HashSet::new();
        let mut aliases = Vec::new();
        for (namespace_claims, api) in providers {
            for module in api.modules {
                let mut consumer_path = vec![stdlib::STDLIB_ROOT.to_string()];
                consumer_path.extend(module.module_path.clone());
                if !namespace_claims.contains(&consumer_path) {
                    continue;
                }

                let module_key = consumer_path.join(".");
                if !seeded_modules.insert(module_key.clone()) {
                    // Collision validation has already guaranteed one provider owner. Repeated API snapshots within one
                    // artifact are still one public module rather than a second overload set.
                    continue;
                }
                let function_counts = module
                    .declarations
                    .iter()
                    .filter_map(|declaration| match declaration {
                        ApiDeclaration::Function(function) => Some(function.name.clone()),
                        _ => None,
                    })
                    .fold(HashMap::<String, usize>::new(), |mut counts, name| {
                        *counts.entry(name).or_default() += 1;
                        counts
                    });
                let mut provider_members = Vec::new();
                for declaration in module.declarations {
                    if let ApiDeclaration::Alias(alias) = &declaration {
                        aliases.push((module_key.clone(), alias.name.clone(), alias.target_path.clone()));
                        continue;
                    }
                    let Some(name) = Self::api_declaration_name(&declaration).map(ToString::to_string) else {
                        continue;
                    };
                    let Some(mut kind) = self.symbol_kind_from_api_declaration(&declaration) else {
                        continue;
                    };
                    Self::qualify_provider_symbol_bounds(&mut kind, &[stdlib::STDLIB_ROOT.to_string()]);
                    if function_counts.get(name.as_str()).copied().unwrap_or_default() > 1
                        && let SymbolKind::Function(info) = &mut kind
                    {
                        info.emitted_name = Some(overloaded_function_emitted_name(&name, info));
                    }
                    provider_members.push((name.clone(), kind.clone()));
                    match kind {
                        SymbolKind::Type(type_info) => {
                            self.transitive_stdlib_stub_types.entry(name).or_insert(type_info);
                        }
                        SymbolKind::Trait(trait_info) => {
                            self.transitive_stdlib_stub_traits
                                .entry(name.clone())
                                .or_insert(trait_info.clone());
                            self.dependency_module_traits
                                .insert(format!("{module_key}.{name}"), trait_info);
                        }
                        _ => {}
                    }
                }
                let members = self.dependency_member_symbols.entry(module_key).or_default();
                for (name, kind) in provider_members {
                    Self::insert_provider_module_symbol(members, name, kind);
                }
            }
        }

        // Facade modules are represented by checked aliases in the artifact. Resolve them only against the artifact
        // maps we just seeded; consumers never need the source prelude to reconstruct a public `std.*` surface.
        let mut unresolved = aliases;
        while !unresolved.is_empty() {
            let mut progressed = false;
            unresolved.retain(|(module_key, name, target_path)| {
                let Some((target_module, target_name)) = Self::sdk_provider_alias_target(target_path) else {
                    return false;
                };
                let Some(kind) = self
                    .dependency_member_symbols
                    .get(&target_module)
                    .and_then(|members| members.get(&target_name))
                    .cloned()
                else {
                    return true;
                };

                match &kind {
                    SymbolKind::Type(type_info) => {
                        self.transitive_stdlib_stub_types
                            .entry(name.clone())
                            .or_insert_with(|| type_info.clone());
                    }
                    SymbolKind::Trait(trait_info) => {
                        self.transitive_stdlib_stub_traits
                            .entry(name.clone())
                            .or_insert_with(|| trait_info.clone());
                        self.dependency_module_traits
                            .insert(format!("{module_key}.{name}"), trait_info.clone());
                    }
                    _ => {}
                }
                let members = self.dependency_member_symbols.entry(module_key.clone()).or_default();
                Self::insert_provider_module_symbol(members, name.clone(), kind);
                progressed = true;
                false
            });
            if !progressed {
                break;
            }
        }
    }

    /// Return an SDK-provider module/member target for one checked facade alias.
    fn sdk_provider_alias_target(target_path: &[String]) -> Option<(String, String)> {
        if target_path.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
            return None;
        }
        let source_path = &target_path[1..];
        let (name, module_path) = source_path.split_last()?;
        Some((
            std::iter::once(stdlib::STDLIB_ROOT)
                .chain(module_path.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("."),
            name.clone(),
        ))
    }

    /// Return the public source spelling for one checked API declaration.
    fn api_declaration_name(declaration: &ApiDeclaration) -> Option<&str> {
        match declaration {
            ApiDeclaration::Function(item) => Some(&item.name),
            ApiDeclaration::Model(item) => Some(&item.name),
            ApiDeclaration::Class(item) => Some(&item.name),
            ApiDeclaration::Trait(item) => Some(&item.name),
            ApiDeclaration::Enum(item) => Some(&item.name),
            ApiDeclaration::Newtype(item) => Some(&item.name),
            ApiDeclaration::TypeAlias(item) => Some(&item.name),
            ApiDeclaration::Const(item) => Some(&item.name),
            ApiDeclaration::Static(item) => Some(&item.name),
            ApiDeclaration::Alias(item) => Some(&item.name),
            ApiDeclaration::Partial(item) => Some(&item.name),
        }
    }

    /// Preserve same-name checked overloads while indexing one provider module.
    fn insert_provider_module_symbol(members: &mut HashMap<String, SymbolKind>, name: String, incoming: SymbolKind) {
        let Some(existing) = members.remove(&name) else {
            members.insert(name, incoming);
            return;
        };
        let merged = match (existing, incoming) {
            (SymbolKind::Function(existing), SymbolKind::Function(incoming)) => SymbolKind::FunctionOverloads(vec![
                FunctionOverloadInfo {
                    info: existing,
                    span: Span::default(),
                },
                FunctionOverloadInfo {
                    info: incoming,
                    span: Span::default(),
                },
            ]),
            (SymbolKind::FunctionOverloads(mut overloads), SymbolKind::Function(incoming)) => {
                overloads.push(FunctionOverloadInfo {
                    info: incoming,
                    span: Span::default(),
                });
                SymbolKind::FunctionOverloads(overloads)
            }
            (SymbolKind::Function(existing), SymbolKind::FunctionOverloads(mut overloads)) => {
                overloads.insert(
                    0,
                    FunctionOverloadInfo {
                        info: existing,
                        span: Span::default(),
                    },
                );
                SymbolKind::FunctionOverloads(overloads)
            }
            (_, incoming) => incoming,
        };
        members.insert(name, merged);
    }

    /// Define `import pub::library` as a module placeholder after validating the manifest entry.
    fn collect_pub_library_import(&mut self, library: &str, alias: Option<&Ident>, span: Span) {
        let name = alias.cloned().unwrap_or_else(|| library.to_string());
        self.validate_root_namespace(&name, span);
        self.validate_pub_library_entry(library, span);
        self.define_import_symbol(name, vec!["pub".to_string(), library.to_string()], false, span);
    }

    /// Collect a Rust crate or crate-path import and attach metadata when available.
    fn collect_rust_crate_import(&mut self, crate_name: &str, path: &[Ident], alias: Option<&Ident>, span: Span) {
        if self.reject_unsupported_rust_core_alloc(crate_name, span) {
            return;
        }

        // Rust crate import: `import rust::serde_json` or `import rust::serde_json::Value`.
        let name = alias
            .cloned()
            .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.to_string()));
        let full_path = self.rust_import_full_path(crate_name, path, None);
        let binding = if path.is_empty() {
            RustImportBindingKind::CrateRoot
        } else {
            RustImportBindingKind::RootedPath
        };
        let canonical_path = full_path.join("::");
        let info = RustItemInfo {
            crate_name: crate_name.to_string(),
            path: canonical_path.clone(),
            binding,
            metadata: self.rust_item_metadata_for_path(&canonical_path),
        };
        self.define_rust_import_binding(name, info, span);
    }

    /// Collect `from rust::... import ...` items and attach any already-prepared metadata.
    fn collect_rust_from_imports(&mut self, crate_name: &str, path: &[Ident], items: &[ImportItem], span: Span) {
        if self.reject_unsupported_rust_core_alloc(crate_name, span) {
            return;
        }

        for item in items {
            let name = Self::import_item_local_name(item);
            let full_path = self.rust_import_full_path(crate_name, path, Some(&item.name));
            let canonical_path = full_path.join("::");
            let info = RustItemInfo {
                crate_name: crate_name.to_string(),
                path: canonical_path.clone(),
                binding: RustImportBindingKind::FromImport,
                metadata: self.rust_item_metadata_for_path(&canonical_path),
            };
            self.define_rust_import_binding(name, info, span);
        }
    }

    /// Return an import item's local binding name after applying `as alias`.
    fn import_item_local_name(item: &ImportItem) -> Ident {
        item.alias.clone().unwrap_or_else(|| item.name.clone())
    }

    /// Load stdlib testing marker metadata only for `from std.testing import ...`.
    fn load_testing_semantics_for_import(
        &mut self,
        context: &FromImportContext<'_>,
        span: Span,
    ) -> Option<TestingMarkerSemantics> {
        let stdlib_context = context.stdlib.as_ref()?;
        if !stdlib_context.is_testing_module {
            return None;
        }

        let checked_provider_manifest = self
            .provider_plan
            .active_sdk_provider_for_module(&context.module.segments)
            .and_then(|provider| provider.manifest.as_deref());
        let loaded = match checked_provider_manifest {
            Some(manifest) => testing_marker_semantics_from_manifest(manifest).and_then(|semantics| {
                semantics.ok_or_else(|| {
                    TestingMarkerLoadError::new(
                        "compiled std.testing provider does not contain checked marker metadata",
                    )
                })
            }),
            None => load_testing_marker_semantics(),
        };
        match loaded {
            Ok(semantics) => {
                self.testing_marker_semantics = Some(semantics.clone());
                Some(semantics)
            }
            Err(err) => {
                self.errors
                    .push(errors::invalid_std_testing_marker_metadata(&err.to_string(), span));
                None
            }
        }
    }

    /// Materialize one stdlib from-import item as a concrete symbol when stdlib metadata owns it.
    ///
    /// Returns `true` when the item was handled; callers should otherwise preserve the ordinary module-placeholder
    /// fallback.
    fn materialize_stdlib_from_import(
        &mut self,
        context: &FromImportContext<'_>,
        item: &ImportItem,
        testing_semantics: Option<&TestingMarkerSemantics>,
        span: Span,
    ) -> bool {
        let Some(stdlib_context) = context.stdlib.as_ref() else {
            return false;
        };

        if self.materialize_typechecker_only_stdlib_import(context.module, item, span) {
            return true;
        }
        if let Some(surface_type) = stdlib_context.allowed_surface_type_import(&item.name) {
            let local_name = Self::import_item_local_name(item);
            let symbol_id =
                self.define_named_import_symbol(local_name.clone(), SymbolKind::Type(TypeInfo::Builtin), span);
            self.surface_type_import_bindings
                .insert(local_name, (surface_type, symbol_id));
            return true;
        }
        if self.materialize_stdlib_submodule_import(context.module, item, span) {
            return true;
        }
        if self.materialize_sdk_provider_import(context, item, testing_semantics, span) {
            return true;
        }
        if self
            .provider_plan
            .active_sdk_provider_for_module(&context.module.segments)
            .is_some_and(|provider| provider.manifest.is_some())
        {
            // An active provider owns this module. Falling back to its source cache would hide an incomplete artifact
            // and give the source tree semantic authority again.
            return false;
        }
        if stdlib_context.has_stub {
            return self.materialize_stdlib_stub_import(context, item, testing_semantics, span);
        }
        false
    }

    /// Materialize one SDK-provider item from checked API metadata.
    fn materialize_sdk_provider_import(
        &mut self,
        context: &FromImportContext<'_>,
        item: &ImportItem,
        testing_semantics: Option<&TestingMarkerSemantics>,
        span: Span,
    ) -> bool {
        let Some(kind) = self
            .dependency_member_symbols
            .get(&context.module.segments.join("."))
            .and_then(|members| members.get(&item.name))
            .cloned()
        else {
            return false;
        };

        let local_name = Self::import_item_local_name(item);
        self.record_testing_marker_import(context, item, &local_name, testing_semantics);
        if let SymbolKind::FunctionOverloads(overloads) = &kind {
            self.record_function_overload_binding(&local_name, overloads, true);
        }
        if matches!(kind, SymbolKind::Static(_)) {
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
            );
        }
        self.define_named_import_symbol(local_name, kind, span);
        true
    }

    /// Materialize `from std.namespace import submodule` as a module binding when the submodule is registered.
    fn materialize_stdlib_submodule_import(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        if module.segments.len() != 2 {
            return false;
        }
        let mut submodule_path = module.segments.clone();
        submodule_path.push(item.name.clone());
        let is_known_submodule = match self.provider_plan.resolve_module(&submodule_path) {
            ProviderModuleResolution::Active(_)
            | ProviderModuleResolution::Disabled(_)
            | ProviderModuleResolution::Unavailable(_) => true,
            ProviderModuleResolution::Unknown if self.provider_plan.has_sdk_catalog() => {
                // A source-publisher bootstrap grant authorizes a namespace root; it does not claim that every item
                // imported from that root is itself a module. The legacy source registry remains the narrow publisher
                // adapter until source component entrypoints publish exact checked claims for ordinary consumers.
                self.provider_plan.bootstrap_owns_sdk_module(&submodule_path)
                    && stdlib::is_known_stdlib_module(&submodule_path)
            }
            ProviderModuleResolution::Unknown => stdlib::is_known_stdlib_module(&submodule_path),
        };
        if !is_known_submodule {
            return false;
        }

        let local_name = Self::import_item_local_name(item);
        self.validate_root_namespace(&local_name, span);
        let path = canonicalize_source_module_segments(&submodule_path);
        self.define_import_symbol(local_name, path, false, span);
        true
    }

    /// Materialize typechecker-only stdlib capability bounds as empty trait symbols.
    fn materialize_typechecker_only_stdlib_import(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        span: Span,
    ) -> bool {
        if !is_typechecker_only_stdlib(&module.segments) || !is_rust_capability_bound(item.name.as_str()) {
            return false;
        }

        self.define_from_import_symbol(
            item,
            SymbolKind::Trait(TraitInfo {
                type_params: vec![],
                methods: HashMap::new(),
                method_aliases: HashMap::new(),
                properties: HashMap::new(),
                requires: vec![],
                supertraits: vec![],
            }),
            span,
        );
        true
    }

    /// Materialize one known stdlib stub item from AST-derived function, trait, type, or constant metadata.
    fn materialize_stdlib_stub_import(
        &mut self,
        context: &FromImportContext<'_>,
        item: &ImportItem,
        testing_semantics: Option<&TestingMarkerSemantics>,
        span: Span,
    ) -> bool {
        if let Some(kind) = self
            .stdlib_cache
            .lookup_function_symbol(&context.module.segments, &item.name)
        {
            let local_name = Self::import_item_local_name(item);
            let surface_function = surface_functions::from_str(&item.name);
            self.record_testing_marker_import(context, item, &local_name, testing_semantics);
            if let SymbolKind::FunctionOverloads(overloads) = &kind {
                self.record_function_overload_binding(&local_name, overloads, true);
            }
            let symbol_id = self.define_named_import_symbol(local_name.clone(), kind, span);
            if let Some(surface_function) = surface_function {
                self.surface_function_import_bindings
                    .insert(local_name, (surface_function, symbol_id));
            }
            return true;
        }

        if let Some(info) = self.stdlib_cache.lookup_trait(&context.module.segments, &item.name) {
            self.define_from_import_symbol(item, SymbolKind::Trait(info), span);
            return true;
        }

        if let Some(info) = self.stdlib_cache.lookup_type(&context.module.segments, &item.name) {
            self.define_from_import_symbol(item, SymbolKind::Type(info), span);
            return true;
        }

        if let Some(info) = self.stdlib_cache.lookup_constant(&context.module.segments, &item.name) {
            self.define_from_import_symbol(item, SymbolKind::Variable(info), span);
            return true;
        }

        if let Some(info) = self.stdlib_cache.lookup_static(&context.module.segments, &item.name) {
            let local_name = Self::import_item_local_name(item);
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
            );
            self.define_named_import_symbol(local_name, SymbolKind::Static(info), span);
            return true;
        }

        false
    }

    /// Record imported `std.testing` marker aliases so decorator validation can reject runtime calls consistently.
    fn record_testing_marker_import(
        &mut self,
        context: &FromImportContext<'_>,
        item: &ImportItem,
        local_name: &str,
        testing_semantics: Option<&TestingMarkerSemantics>,
    ) {
        let Some(stdlib_context) = context.stdlib.as_ref() else {
            return;
        };
        if !stdlib_context.is_testing_module {
            return;
        }

        let mut resolved_marker_path = context.module.segments.clone();
        resolved_marker_path.push(item.name.clone());
        let module_feature = self.surface_context.decorator_feature_for_path(&resolved_marker_path);
        let marker_feature = testing_semantics
            .and_then(|semantics| semantics.marker_kind(&item.name))
            .map(|_| SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker));
        if module_feature == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
            && marker_feature == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker))
        {
            self.testing_marker_import_bindings.insert(local_name.to_string());
        }
    }

    /// Preserve an imported item that has already been materialized as a concrete symbol in this collection pass.
    ///
    /// This keeps dependency metadata imports, especially statics, from being rewritten as module path proxies.
    /// Returns `true` when the caller should skip fallback placeholder materialization.
    fn preserve_existing_from_import_symbol(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
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
                        crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
                    );
                }
                self.symbols.define(Symbol {
                    name: alias.clone(),
                    kind: imported_kind,
                    span,
                    scope: 0,
                });
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

    /// Record source origin metadata for an imported binding whose symbol was already materialized earlier.
    fn record_source_import_target(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        local_name: &str,
        kind: &SymbolKind,
    ) {
        if let Some(target_kind) = Self::source_target_kind(kind) {
            self.source_import_targets.insert(
                local_name.to_string(),
                crate::frontend::typechecker::SourceTargetInfo {
                    module_path: canonicalize_source_module_segments(&module.segments),
                    name: item.name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
    }

    /// Define a fallback module placeholder for one `from module import item` binding.
    fn define_from_import_placeholder(&mut self, module: &ImportPath, item: &ImportItem, span: Span) {
        let name = Self::import_item_local_name(item);
        self.validate_root_namespace(&name, span);
        let mut path = canonicalize_source_module_segments(&module.segments);
        path.push(item.name.clone());
        self.define_import_symbol(name, path, false, span);
    }

    /// Define one imported item under its local alias after root namespace validation.
    fn define_from_import_symbol(&mut self, item: &ImportItem, kind: SymbolKind, span: Span) {
        let local_name = Self::import_item_local_name(item);
        self.define_named_import_symbol(local_name, kind, span);
    }

    /// Return the exact source dependency member targeted by a `from module import item` declaration.
    fn imported_source_dependency_symbol_kind(&self, module: &ImportPath, item: &ImportItem) -> Option<SymbolKind> {
        self.dependency_member_symbol_for_path(module, &item.name)
    }

    /// Return the exact partial projection metadata targeted by a source import.
    fn imported_source_dependency_partial_projection(
        &self,
        module: &ImportPath,
        item: &ImportItem,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_member_partial_projection_for_path(module, &item.name)
    }

    /// Return the codegraph declaration kind for source targets this importer can preserve.
    fn source_target_kind(kind: &SymbolKind) -> Option<&'static str> {
        Self::source_target_kind_for_symbol(kind)
    }

    /// Define a source-imported dependency symbol under its local import name.
    fn define_resolved_source_import_symbol(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        mut kind: SymbolKind,
        projection: Option<PartialProjectionInfo>,
        span: Span,
    ) {
        let local_name = Self::import_item_local_name(item);
        if let Some(target_kind) = Self::source_target_kind(&kind) {
            self.source_import_targets.insert(
                local_name.clone(),
                crate::frontend::typechecker::SourceTargetInfo {
                    module_path: canonicalize_source_module_segments(&module.segments),
                    name: item.name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
        if let SymbolKind::FunctionOverloads(overloads) = &kind {
            self.record_function_overload_binding(&local_name, overloads, true);
        }
        if let SymbolKind::Static(info) = &mut kind {
            info.is_imported = true;
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
            );
        }
        if let Some(mut projection) = projection {
            projection.name.clone_from(&local_name);
            self.type_info.record_partial_projection(projection);
        }
        self.define_named_import_symbol(local_name, kind, span);
    }

    /// Define one already named imported symbol after root namespace validation.
    fn define_named_import_symbol(&mut self, name: Ident, kind: SymbolKind, span: Span) -> SymbolId {
        self.validate_root_namespace(&name, span);
        self.symbols.define(Symbol {
            name,
            kind,
            span,
            scope: 0,
        })
    }

    /// Validate one imported `pub::<library>` root before selected or wildcard item collection begins.
    fn validate_pub_library_entry(&mut self, library: &str, span: Span) {
        let library_manifests = self.provider_plan.library_manifest_index();
        let known_libraries = library_manifests.known_libraries();
        let Some(entry) = library_manifests.get(library).cloned() else {
            self.errors
                .push(errors::unknown_pub_library(library, &known_libraries, span));
            return;
        };
        if let LibraryManifestIndexEntry::Failed(failure) = entry {
            self.push_pub_library_failure(library, &failure, span);
        }
    }

    /// Collect selected public imports from one loaded library manifest.
    fn collect_pub_imports(&mut self, library: &str, items: &[ImportItem], span: Span) {
        let library_manifests = self.provider_plan.library_manifest_index();
        let known_libraries = library_manifests.known_libraries();
        let Some(entry) = library_manifests.get(library).cloned() else {
            self.errors
                .push(errors::unknown_pub_library(library, &known_libraries, span));
            return;
        };

        let manifest = match entry {
            LibraryManifestIndexEntry::Loaded { manifest, .. } => manifest,
            LibraryManifestIndexEntry::Failed(failure) => {
                self.push_pub_library_failure(library, &failure, span);
                return;
            }
        };

        self.cache_transitive_pub_export_semantics(library, &manifest);

        let available_exports = Self::manifest_export_names(&manifest);
        let mut imported_type_aliases: HashMap<String, String> = HashMap::new();
        for item in items {
            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
            if let Some(export) = Self::find_manifest_export(&manifest, &item.name)
                && Self::manifest_export_is_type(&export)
            {
                imported_type_aliases.insert(item.name.clone(), local_name);
            }
        }

        for item in items {
            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
            if let Some(mut kind) = self.pub_library_function_symbol(&manifest, &item.name) {
                self.validate_root_namespace(&local_name, span);
                if let Some(existing_kind) = self.existing_local_symbol_kind(&local_name) {
                    self.errors.push(errors::pub_library_import_name_collision(
                        &local_name,
                        existing_kind,
                        span,
                    ));
                    continue;
                }
                self.remap_symbol_kind_with_import_aliases(&mut kind, &imported_type_aliases);
                if let SymbolKind::FunctionOverloads(overloads) = &kind {
                    self.record_function_overload_binding(&local_name, overloads, true);
                }
                self.symbols.define(Symbol {
                    name: local_name,
                    kind,
                    span,
                    scope: 0,
                });
                continue;
            }

            let Some(export) = Self::find_manifest_export(&manifest, &item.name) else {
                if let Some(feature_sets) = Self::inactive_pub_export_features(&manifest, &item.name) {
                    self.errors.push(errors::pub_library_symbol_requires_features(
                        &item.name,
                        library,
                        &feature_sets,
                        span,
                    ));
                } else {
                    self.errors.push(errors::pub_library_symbol_not_exported(
                        &item.name,
                        library,
                        &available_exports,
                        span,
                    ));
                }
                continue;
            };

            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
            self.validate_root_namespace(&local_name, span);
            if let Some(existing_kind) = self.existing_local_symbol_kind(&local_name) {
                self.errors.push(errors::pub_library_import_name_collision(
                    &local_name,
                    existing_kind,
                    span,
                ));
                continue;
            }

            self.define_pub_import_symbol(&manifest, local_name, export, &imported_type_aliases, span);
        }
    }

    /// Return the missing public features for one known provider export that is inactive in this artifact projection.
    fn inactive_pub_export_features(manifest: &LibraryManifest, symbol: &str) -> Option<Vec<Vec<String>>> {
        let active = &manifest.contract_metadata.provider.active_features;
        let mut alternatives = manifest
            .contract_metadata
            .provider
            .fact_requirements
            .iter()
            .filter(|requirement| {
                requirement.kind == ProviderFactKind::Export
                    && requirement.identity.rsplit("::").next() == Some(symbol)
                    && !requirement.required_features.is_subset(active)
            })
            .map(|requirement| {
                requirement
                    .required_features
                    .difference(active)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        alternatives.sort();
        alternatives.dedup();
        (!alternatives.is_empty()).then_some(alternatives)
    }

    /// Resolve one exported function name from a public manifest into a local symbol kind.
    pub(in crate::frontend::typechecker) fn pub_library_function_symbol(
        &self,
        manifest: &LibraryManifest,
        member: &str,
    ) -> Option<SymbolKind> {
        let functions = manifest
            .exports
            .functions
            .iter()
            .filter(|item| item.name == member)
            .collect::<Vec<_>>();
        match functions.as_slice() {
            [] => None,
            [function] => Some(SymbolKind::Function(self.function_info_from_manifest(function))),
            _ => Some(SymbolKind::FunctionOverloads(
                functions
                    .into_iter()
                    .map(|function| FunctionOverloadInfo {
                        info: self.function_info_from_manifest(function),
                        span: Span::default(),
                    })
                    .collect(),
            )),
        }
    }

    /// Resolve one exported const/static value type from a loaded `pub::` library manifest.
    pub(in crate::frontend::typechecker) fn lookup_pub_library_constant_member(
        &self,
        library: &str,
        member: &str,
    ) -> Option<VariableInfo> {
        let entry = self.provider_plan.library_manifest_index().get(library)?;
        let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
            return None;
        };
        if let Some(export) = manifest.exports.consts.iter().find(|item| item.name == member) {
            return Some(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            });
        }
        if let Some(export) = manifest.exports.statics.iter().find(|item| item.name == member) {
            return Some(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: true,
                is_used: false,
            });
        }
        None
    }

    /// Resolve one exported member from an imported `pub::` library as a symbol kind.
    ///
    /// This is used by qualified alias collection, where the import remains a module binding (`lib.member`) instead of
    /// a direct `from pub::lib import member` symbol. Alias exports are followed to their manifest target before the
    /// projected kind is returned.
    pub(in crate::frontend::typechecker) fn lookup_pub_library_symbol_member(
        &self,
        library: &str,
        member: &str,
    ) -> Option<SymbolKind> {
        let entry = self.provider_plan.library_manifest_index().get(library)?;
        let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
            return None;
        };
        if let Some(kind) = self.pub_library_function_symbol(manifest, member) {
            return Some(kind);
        }
        let export = Self::find_manifest_export(manifest, member)?;
        Some(match export {
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
            ManifestExportRef::Partial(export) => SymbolKind::Function(self.partial_info_from_manifest(export)),
            ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::TypeAlias(_) => SymbolKind::Type(TypeInfo::TypeAlias),
            ManifestExportRef::Newtype(export) => {
                SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
            }
            ManifestExportRef::Const(export) => SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            }),
            ManifestExportRef::Static(export) => SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            }),
            ManifestExportRef::EnumVariant { enum_name, fields } => SymbolKind::Variant(VariantInfo {
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            }),
            ManifestExportRef::Alias(export) => {
                return self.symbol_kind_from_manifest_alias(manifest, export, &mut HashSet::new());
            }
        })
    }

    /// Seed internal semantic caches for one `pub::` library's exported types and traits.
    ///
    /// These caches are used only by the consumer-side typechecker when imported signatures mention provider types
    /// that the consumer did not explicitly import by name (for example `Session.read_csv(...) -> LazyFrame[T]`).
    /// They do not change source-visible name resolution.
    fn cache_transitive_pub_export_semantics(&mut self, library: &str, manifest: &LibraryManifest) {
        if !self.cached_pub_libraries.insert(library.to_string()) {
            return;
        }

        for model in &manifest.exports.models {
            let model_info = self.model_info_from_manifest(model);
            self.transitive_pub_types
                .entry(model.name.clone())
                .or_default()
                .push(TypeInfo::Model(model_info));
        }
        for class in &manifest.exports.classes {
            let class_info = self.class_info_from_manifest(class);
            self.transitive_pub_types
                .entry(class.name.clone())
                .or_default()
                .push(TypeInfo::Class(class_info));
        }
        for enum_export in &manifest.exports.enums {
            let enum_info = self.enum_info_from_manifest(enum_export);
            self.transitive_pub_types
                .entry(enum_export.name.clone())
                .or_default()
                .push(TypeInfo::Enum(enum_info));
        }
        for newtype in &manifest.exports.newtypes {
            let newtype_info = self.newtype_info_from_manifest(newtype);
            self.transitive_pub_types
                .entry(newtype.name.clone())
                .or_default()
                .push(TypeInfo::Newtype(newtype_info));
        }
        for trait_export in &manifest.exports.traits {
            let trait_info = self.trait_info_from_manifest(trait_export);
            self.transitive_pub_traits
                .entry(trait_export.name.clone())
                .or_default()
                .push(trait_info);
        }
    }

    fn format_manifest_failure_detail(
        &self,
        failure: &crate::frontend::library_manifest_index::LibraryManifestLoadFailure,
    ) -> String {
        match failure.kind {
            LibraryManifestFailureKind::ManifestRead => {
                format!("Manifest file is unreadable: {}", failure.message)
            }
            LibraryManifestFailureKind::ManifestParse => {
                format!("Manifest JSON is malformed: {}", failure.message)
            }
            LibraryManifestFailureKind::ManifestInvalid => {
                format!("Manifest is incompatible or invalid: {}", failure.message)
            }
            LibraryManifestFailureKind::ArtifactMissing => {
                format!("Generated library artifacts are missing: {}", failure.message)
            }
            LibraryManifestFailureKind::ArtifactInvalid => {
                format!("Generated library artifacts are invalid: {}", failure.message)
            }
            LibraryManifestFailureKind::ArtifactMismatch => {
                format!("Generated library artifact names do not match: {}", failure.message)
            }
        }
    }

    fn push_pub_library_failure(
        &mut self,
        library: &str,
        failure: &crate::frontend::library_manifest_index::LibraryManifestLoadFailure,
        span: Span,
    ) {
        let details = self.format_manifest_failure_detail(failure);
        let path = failure.path.to_string_lossy();
        let error = match failure.kind {
            LibraryManifestFailureKind::ManifestRead
            | LibraryManifestFailureKind::ManifestParse
            | LibraryManifestFailureKind::ManifestInvalid => {
                errors::pub_library_manifest_load_failed(library, path.as_ref(), &details, span)
            }
            LibraryManifestFailureKind::ArtifactMissing => {
                errors::pub_library_artifact_missing(library, path.as_ref(), &details, span)
            }
            LibraryManifestFailureKind::ArtifactInvalid => {
                errors::pub_library_artifact_invalid(library, path.as_ref(), &details, span)
            }
            LibraryManifestFailureKind::ArtifactMismatch => {
                errors::pub_library_artifact_mismatch(library, path.as_ref(), &details, span)
            }
        };
        self.errors.push(error);
    }

    /// Return all exported names in a manifest for diagnostics.
    fn manifest_export_names(manifest: &LibraryManifest) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(manifest.exports.models.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.aliases.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.partials.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.classes.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.functions.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.traits.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.enums.iter().map(|item| item.name.clone()));
        names.extend(
            manifest
                .exports
                .enums
                .iter()
                .flat_map(|item| item.variants.iter().map(|variant| variant.name.clone())),
        );
        names.extend(manifest.exports.type_aliases.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.newtypes.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.consts.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.statics.iter().map(|item| item.name.clone()));
        names.sort();
        names.dedup();
        names
    }

    /// Find one manifest export by name, including alias entries.
    fn find_manifest_export<'a>(manifest: &'a LibraryManifest, name: &str) -> Option<ManifestExportRef<'a>> {
        if let Some(item) = manifest.exports.aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Alias(item));
        }
        if let Some(item) = manifest.exports.models.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Model(item));
        }
        if let Some(item) = manifest.exports.classes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Class(item));
        }
        if let Some(item) = manifest.exports.functions.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Function(item));
        }
        if let Some(item) = manifest.exports.partials.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Partial(item));
        }
        if let Some(item) = manifest.exports.traits.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Trait(item));
        }
        if let Some(item) = manifest.exports.enums.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Enum(item));
        }
        for enum_export in &manifest.exports.enums {
            if let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == name) {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                });
            }
        }
        if let Some(item) = manifest.exports.type_aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::TypeAlias(item));
        }
        if let Some(item) = manifest.exports.newtypes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Newtype(item));
        }
        if let Some(item) = manifest.exports.consts.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Const(item));
        }
        if let Some(item) = manifest.exports.statics.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Static(item));
        }
        None
    }

    /// Find one manifest export by name without following alias entries.
    fn find_manifest_non_alias_export<'a>(manifest: &'a LibraryManifest, name: &str) -> Option<ManifestExportRef<'a>> {
        if let Some(item) = manifest.exports.models.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Model(item));
        }
        if let Some(item) = manifest.exports.classes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Class(item));
        }
        if let Some(item) = manifest.exports.functions.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Function(item));
        }
        if let Some(item) = manifest.exports.partials.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Partial(item));
        }
        if let Some(item) = manifest.exports.traits.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Trait(item));
        }
        if let Some(item) = manifest.exports.enums.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Enum(item));
        }
        for enum_export in &manifest.exports.enums {
            if let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == name) {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                });
            }
        }
        if let Some(item) = manifest.exports.type_aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::TypeAlias(item));
        }
        if let Some(item) = manifest.exports.newtypes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Newtype(item));
        }
        if let Some(item) = manifest.exports.consts.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Const(item));
        }
        if let Some(item) = manifest.exports.statics.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Static(item));
        }
        None
    }

    /// Resolve a manifest alias into the symbol it actually projects.
    ///
    /// Facades often publish aliases whose short target name is the same as the exported alias name (`Frame` →
    /// `exprs.Frame`). Resolving only by the final segment re-enters the alias export and can recurse forever. Prefer
    /// the explicit target path in checked API metadata, then fall back to non-alias manifest exports, and only follow
    /// another alias with a visited guard.
    fn symbol_kind_from_manifest_alias(
        &self,
        manifest: &LibraryManifest,
        export: &AliasExport,
        visited: &mut HashSet<Vec<String>>,
    ) -> Option<SymbolKind> {
        if let Some(function) = &export.projected_function {
            return Some(SymbolKind::Function(self.function_info_from_manifest(function)));
        }
        if let Some(target_path) = Self::manifest_identity_target_path(manifest, &export.name)
            && !visited.contains(target_path)
            && let Some(kind) = self.symbol_kind_from_manifest_path(manifest, target_path)
        {
            return Some(kind);
        }
        if !visited.insert(export.target_path.clone()) {
            return None;
        }
        if let Some(kind) = self.symbol_kind_from_manifest_path(manifest, &export.target_path) {
            return Some(kind);
        }
        let target_name = export.target_path.last()?;
        if let Some(kind) = self
            .find_manifest_non_alias_symbol_kind(manifest, target_name)
            .or_else(|| self.pub_library_function_symbol(manifest, target_name))
        {
            return Some(kind);
        }
        let ManifestExportRef::Alias(target_alias) = Self::find_manifest_export(manifest, target_name)? else {
            return None;
        };
        self.symbol_kind_from_manifest_alias(manifest, target_alias, visited)
    }

    /// Return the canonical target path published by the manifest identity graph for one public name.
    fn manifest_identity_target_path<'a>(manifest: &'a LibraryManifest, public_name: &str) -> Option<&'a [String]> {
        manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)
            .and_then(|entry| entry.target_path())
    }

    /// Resolve an exact manifest identity path into a semantic symbol kind before falling back to final-segment lookup.
    fn symbol_kind_from_manifest_path(&self, manifest: &LibraryManifest, target_path: &[String]) -> Option<SymbolKind> {
        if let Some(kind) = self.symbol_kind_from_api_target_path(manifest, target_path) {
            return Some(kind);
        }
        let target_name = target_path.last()?;
        self.find_manifest_non_alias_symbol_kind(manifest, target_name)
            .or_else(|| self.pub_library_function_symbol(manifest, target_name))
    }

    /// Convert one non-alias manifest export into checker symbol metadata.
    fn find_manifest_non_alias_symbol_kind(&self, manifest: &LibraryManifest, name: &str) -> Option<SymbolKind> {
        match Self::find_manifest_non_alias_export(manifest, name)? {
            ManifestExportRef::Model(export) => {
                Some(SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export))))
            }
            ManifestExportRef::Class(export) => {
                Some(SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export))))
            }
            ManifestExportRef::Function(export) => Some(SymbolKind::Function(self.function_info_from_manifest(export))),
            ManifestExportRef::Partial(export) => Some(SymbolKind::Function(self.partial_info_from_manifest(export))),
            ManifestExportRef::Trait(export) => Some(SymbolKind::Trait(self.trait_info_from_manifest(export))),
            ManifestExportRef::Enum(export) => {
                Some(SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))))
            }
            ManifestExportRef::EnumVariant { enum_name, fields } => Some(SymbolKind::Variant(VariantInfo {
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            })),
            ManifestExportRef::TypeAlias(_) => Some(SymbolKind::Type(TypeInfo::TypeAlias)),
            ManifestExportRef::Newtype(export) => Some(SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(export),
            ))),
            ManifestExportRef::Const(export) => Some(SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            })),
            ManifestExportRef::Static(export) => Some(SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            })),
            ManifestExportRef::Alias(_) => None,
        }
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    fn api_declaration_for_target_path<'a>(
        manifest: &'a LibraryManifest,
        target_path: &[String],
    ) -> Option<&'a ApiDeclaration> {
        let name = target_path.last()?;
        let module_path = if target_path.first().is_some_and(|segment| segment == "crate") {
            &target_path[1..]
        } else {
            target_path
        };
        let module_path = module_path.get(..module_path.len().saturating_sub(1))?;
        let api = manifest.contract_metadata.api.as_ref()?;
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        module.declarations.iter().find(|declaration| match declaration {
            ApiDeclaration::Function(item) => item.name == *name,
            ApiDeclaration::Model(item) => item.name == *name,
            ApiDeclaration::Class(item) => item.name == *name,
            ApiDeclaration::Trait(item) => item.name == *name,
            ApiDeclaration::Enum(item) => item.name == *name,
            ApiDeclaration::Newtype(item) => item.name == *name,
            ApiDeclaration::TypeAlias(item) => item.name == *name,
            ApiDeclaration::Const(item) => item.name == *name,
            ApiDeclaration::Static(item) => item.name == *name,
            ApiDeclaration::Alias(item) => item.name == *name,
            ApiDeclaration::Partial(item) => item.name == *name,
        })
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    fn symbol_kind_from_api_target_path(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        let declaration = Self::api_declaration_for_target_path(manifest, target_path)?;
        self.symbol_kind_from_api_declaration(declaration)
    }

    /// Convert one checked API declaration into the same semantic symbols used for manifest exports.
    fn symbol_kind_from_api_declaration(&self, declaration: &ApiDeclaration) -> Option<SymbolKind> {
        match declaration {
            ApiDeclaration::Function(item) => Some(SymbolKind::Function(
                self.function_info_from_manifest(&function_export_from_api(item)),
            )),
            ApiDeclaration::Model(item) => Some(SymbolKind::Type(TypeInfo::Model(
                self.model_info_from_manifest(&model_export_from_api(item)),
            ))),
            ApiDeclaration::Class(item) => Some(SymbolKind::Type(TypeInfo::Class(
                self.class_info_from_manifest(&class_export_from_api(item)),
            ))),
            ApiDeclaration::Trait(item) => Some(SymbolKind::Trait(
                self.trait_info_from_manifest(&trait_export_from_api(item)),
            )),
            ApiDeclaration::Enum(item) => Some(SymbolKind::Type(TypeInfo::Enum(
                self.enum_info_from_manifest(&enum_export_from_api(item)),
            ))),
            ApiDeclaration::Newtype(item) => Some(SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(&newtype_export_from_api(item)),
            ))),
            ApiDeclaration::TypeAlias(_) => Some(SymbolKind::Type(TypeInfo::TypeAlias)),
            ApiDeclaration::Const(item) => Some(SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&item.ty),
                is_mutable: false,
                is_used: false,
            })),
            ApiDeclaration::Static(item) => Some(SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&item.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            })),
            ApiDeclaration::Alias(item) => item.projected_function.as_ref().map(|function| {
                SymbolKind::Function(self.function_info_from_manifest(&function_export_from_api_projected(function)))
            }),
            ApiDeclaration::Partial(item) => Some(SymbolKind::Function(
                self.partial_info_from_manifest(&partial_export_from_api(item)),
            )),
        }
    }

    /// Return whether a manifest export introduces a type-like name into the importing module.
    fn manifest_export_is_type(export: &ManifestExportRef<'_>) -> bool {
        matches!(
            export,
            ManifestExportRef::Model(_)
                | ManifestExportRef::Class(_)
                | ManifestExportRef::Trait(_)
                | ManifestExportRef::Enum(_)
                | ManifestExportRef::TypeAlias(_)
                | ManifestExportRef::Newtype(_)
        )
    }

    /// Return a stable diagnostic label for a symbol that already exists in the current local scope.
    fn existing_local_symbol_kind(&self, name: &str) -> Option<&'static str> {
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
        };
        Some(kind)
    }

    /// Define one symbol imported from a public library manifest.
    fn define_pub_import_symbol(
        &mut self,
        manifest: &LibraryManifest,
        local_name: String,
        export: ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let partial_projection =
            self.partial_projection_from_manifest_export(manifest, &local_name, &export, imported_type_aliases, span);
        let mut kind = match export {
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
            ManifestExportRef::Partial(export) => SymbolKind::Function(self.partial_info_from_manifest(export)),
            ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::EnumVariant { enum_name, fields } => SymbolKind::Variant(VariantInfo {
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            }),
            ManifestExportRef::TypeAlias(export) => {
                let mut target = resolved_type_from_manifest_type_ref(&export.target);
                Self::remap_resolved_type_with_import_aliases(&mut target, imported_type_aliases);
                self.type_aliases.insert(
                    local_name.clone(),
                    crate::frontend::typechecker::TypeAliasTarget {
                        type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
                        target,
                    },
                );
                SymbolKind::Type(TypeInfo::TypeAlias)
            }
            ManifestExportRef::Newtype(export) => {
                SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
            }
            ManifestExportRef::Const(export) => SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            }),
            ManifestExportRef::Static(export) => SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            }),
            ManifestExportRef::Alias(export) => {
                let Some(kind) = self.symbol_kind_from_manifest_alias(manifest, export, &mut HashSet::new()) else {
                    return;
                };
                kind
            }
        };
        self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);

        if let SymbolKind::FunctionOverloads(overloads) = &kind {
            self.record_function_overload_binding(&local_name, overloads, true);
        }
        if let Some(projection) = partial_projection {
            self.type_info.record_partial_projection(projection);
        }

        if matches!(kind, SymbolKind::Static(_)) {
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
            );
        }

        self.symbols.define(Symbol {
            name: local_name,
            kind,
            span,
            scope: 0,
        });
    }

    /// Rewrite imported semantic type references through type aliases from the source library manifest.
    fn remap_symbol_kind_with_import_aliases(
        &self,
        kind: &mut SymbolKind,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if imported_type_aliases.is_empty() {
            return;
        }

        match kind {
            SymbolKind::Variable(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Static(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Function(info) => {
                for param in &mut info.params {
                    Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                }
                Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
            }
            SymbolKind::FunctionOverloads(overloads) => {
                for overload in overloads {
                    for param in &mut overload.info.params {
                        Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                    }
                    Self::remap_resolved_type_with_import_aliases(
                        &mut overload.info.return_type,
                        imported_type_aliases,
                    );
                }
            }
            SymbolKind::Type(ty_info) => match ty_info {
                TypeInfo::Class(info) => {
                    if let Some(extends) = &mut info.extends
                        && let Some(alias) = imported_type_aliases.get(extends)
                    {
                        *extends = alias.clone();
                    }
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for property in info.properties.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        for param in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Model(info) => {
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for property in info.properties.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        for param in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Newtype(info) => {
                    Self::remap_resolved_type_with_import_aliases(&mut info.underlying, imported_type_aliases);
                    for method in info.methods.values_mut() {
                        for param in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Enum(_) | TypeInfo::TypeAlias | TypeInfo::Builtin => {}
            },
            SymbolKind::Trait(info) => {
                for method in info.methods.values_mut() {
                    for param in &mut method.params {
                        Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                    }
                    Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                }
                for (_, ty) in &mut info.requires {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
                for property in info.properties.values_mut() {
                    Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                }
            }
            SymbolKind::Module(_)
            | SymbolKind::Variant(_)
            | SymbolKind::Field(_)
            | SymbolKind::Property(_)
            | SymbolKind::RustItem(_) => {}
        }
    }

    /// Rewrite resolved type names through import aliases after stdlib materialization.
    fn remap_resolved_type_with_import_aliases(ty: &mut ResolvedType, imported_type_aliases: &HashMap<String, String>) {
        match ty {
            ResolvedType::Named(name) => {
                if let Some(alias) = imported_type_aliases.get(name) {
                    *name = alias.clone();
                }
            }
            ResolvedType::Generic(name, args) => {
                if let Some(alias) = imported_type_aliases.get(name) {
                    *name = alias.clone();
                }
                for arg in args {
                    Self::remap_resolved_type_with_import_aliases(arg, imported_type_aliases);
                }
            }
            ResolvedType::Function(params, return_type) => {
                for param in params {
                    Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
                }
                Self::remap_resolved_type_with_import_aliases(return_type, imported_type_aliases);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    Self::remap_resolved_type_with_import_aliases(item, imported_type_aliases);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::TypeToken(inner) => {
                Self::remap_resolved_type_with_import_aliases(inner, imported_type_aliases);
            }
            ResolvedType::FrozenDict(key, value) => {
                Self::remap_resolved_type_with_import_aliases(key, imported_type_aliases);
                Self::remap_resolved_type_with_import_aliases(value, imported_type_aliases);
            }
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => {}
        }
    }

    /// Convert one manifest function export into semantic function metadata.
    fn function_info_from_manifest(&self, export: &FunctionExport) -> FunctionInfo {
        FunctionInfo {
            params: self.params_from_manifest(&export.params),
            return_type: resolved_type_from_manifest_type_ref(&export.return_type),
            is_async: export.is_async,
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            type_param_bounds: self.type_param_bounds_from_manifest(&export.type_params),
            type_param_bound_details: self.type_param_bound_details_from_manifest(&export.type_params),
            emitted_name: export.emitted_name.clone(),
        }
    }

    /// Convert one manifest partial export into callable metadata for consumers.
    fn partial_info_from_manifest(&self, export: &PartialExport) -> FunctionInfo {
        FunctionInfo {
            params: self.params_from_manifest(&export.params),
            return_type: resolved_type_from_manifest_type_ref(&export.return_type),
            is_async: export.is_async,
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            type_param_bounds: self.type_param_bounds_from_manifest(&export.type_params),
            type_param_bound_details: self.type_param_bound_details_from_manifest(&export.type_params),
            emitted_name: None,
        }
    }

    /// Convert one public-manifest export into partial projection metadata when it denotes a partial callable.
    fn partial_projection_from_manifest_export(
        &self,
        manifest: &LibraryManifest,
        local_name: &str,
        export: &ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<PartialProjectionInfo> {
        match export {
            ManifestExportRef::Partial(export) => {
                Self::partial_projection_from_manifest_partial(export, local_name, imported_type_aliases, span)
            }
            ManifestExportRef::Alias(export) => self.partial_projection_from_manifest_alias(
                manifest,
                local_name,
                export,
                imported_type_aliases,
                span,
                &mut HashSet::new(),
            ),
            _ => None,
        }
    }

    /// Follow manifest aliases so a public alias to a partial keeps the same projection under the alias name.
    fn partial_projection_from_manifest_alias(
        &self,
        manifest: &LibraryManifest,
        local_name: &str,
        alias: &AliasExport,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
        visited: &mut HashSet<String>,
    ) -> Option<PartialProjectionInfo> {
        let identity_target = Self::manifest_identity_target_path(manifest, &alias.name);
        let target_path = identity_target.unwrap_or(alias.target_path.as_slice());
        let target_name = target_path.last()?;
        if !visited.insert(target_name.clone()) {
            return None;
        }
        if let Some(ApiDeclaration::Partial(item)) = Self::api_declaration_for_target_path(manifest, target_path) {
            let export = partial_export_from_api(item);
            return Self::partial_projection_from_manifest_partial(&export, local_name, imported_type_aliases, span);
        }
        match Self::find_manifest_export(manifest, target_name)? {
            ManifestExportRef::Partial(export) => {
                Self::partial_projection_from_manifest_partial(export, local_name, imported_type_aliases, span)
            }
            ManifestExportRef::Alias(next_alias) => self.partial_projection_from_manifest_alias(
                manifest,
                local_name,
                next_alias,
                imported_type_aliases,
                span,
                visited,
            ),
            _ => None,
        }
    }

    /// Reconstruct import-visible projection metadata from serialized partial preset metadata.
    fn partial_projection_from_manifest_partial(
        export: &PartialExport,
        local_name: &str,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<PartialProjectionInfo> {
        let mut target_path = export.target_path.clone();
        if let Some(target_name) = target_path.last_mut()
            && let Some(local_alias) = imported_type_aliases.get(target_name)
        {
            target_name.clone_from(local_alias);
        }
        Some(PartialProjectionInfo {
            name: local_name.to_string(),
            target_path,
            target_kind: Self::partial_projection_target_kind_from_manifest(export.target_kind),
            presets: export
                .presets
                .iter()
                .map(|preset| {
                    Some(PartialProjectionPreset {
                        name: preset.name.clone(),
                        value: Self::manifest_preset_value_expr(&preset.value, imported_type_aliases, span)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }

    /// Convert manifest partial target kind metadata into frontend projection vocabulary.
    fn partial_projection_target_kind_from_manifest(kind: PartialTargetKindExport) -> PartialProjectionTargetKind {
        match kind {
            PartialTargetKindExport::Function => PartialProjectionTargetKind::Function,
            PartialTargetKindExport::ModelConstructor => PartialProjectionTargetKind::ModelConstructor,
            PartialTargetKindExport::ClassConstructor => PartialProjectionTargetKind::ClassConstructor,
            PartialTargetKindExport::NewtypeConstructor => PartialProjectionTargetKind::NewtypeConstructor,
            PartialTargetKindExport::Partial | PartialTargetKindExport::Unknown => PartialProjectionTargetKind::Unknown,
        }
    }

    /// Rebuild a metadata-safe preset value as a synthetic AST expression.
    fn manifest_preset_value_expr(
        value: &PresetValueExport,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let expr = match value {
            PresetValueExport::Int(value) => Expr::Literal(Literal::Int(IntLiteral::synthetic(*value))),
            PresetValueExport::Float(value) => Expr::Literal(Literal::Float(FloatLiteral {
                value: value.parse().ok()?,
                repr: value.clone(),
            })),
            PresetValueExport::Bool(value) => Expr::Literal(Literal::Bool(*value)),
            PresetValueExport::String(value) => Expr::Literal(Literal::String(value.clone())),
            PresetValueExport::Bytes(value) => Expr::Literal(Literal::Bytes(value.clone())),
            PresetValueExport::None => Expr::Literal(Literal::None),
            PresetValueExport::List(values) => Expr::List(
                values
                    .iter()
                    .map(|item| {
                        Self::manifest_preset_value_expr(item, imported_type_aliases, span).map(ListEntry::Element)
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Dict(entries) => Expr::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        Some(DictEntry::Pair(
                            Self::manifest_preset_value_expr(&entry.key, imported_type_aliases, span)?,
                            Self::manifest_preset_value_expr(&entry.value, imported_type_aliases, span)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::ConstRef(path) => Self::manifest_const_ref_expr(path, span)?.node,
            PresetValueExport::ModelLiteral { name, fields } => {
                let constructor = imported_type_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
                Expr::Call(
                    Box::new(Spanned::new(Expr::Ident(constructor), span)),
                    Vec::new(),
                    fields
                        .iter()
                        .map(|field| {
                            Some(CallArg::Named(
                                field.name.clone(),
                                Self::manifest_preset_value_expr(&field.value, imported_type_aliases, span)?,
                            ))
                        })
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            PresetValueExport::Unsupported => return None,
        };
        Some(Spanned::new(expr, span))
    }

    /// Build an identifier/field expression for a serialized const reference path.
    fn manifest_const_ref_expr(path: &[String], span: Span) -> Option<Spanned<Expr>> {
        let (first, rest) = path.split_first()?;
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in rest {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        Some(expr)
    }

    /// Convert one manifest model export into semantic model metadata.
    fn model_info_from_manifest(&self, export: &ModelExport) -> ModelInfo {
        let methods = self.methods_from_manifest(&export.methods);
        let method_overloads = self.method_overloads_from_manifest(&export.methods);
        ModelInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(&export.fields),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: std::collections::HashMap::new(),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Convert one manifest class export into semantic class metadata.
    fn class_info_from_manifest(&self, export: &ClassExport) -> ClassInfo {
        let methods = self.methods_from_manifest(&export.methods);
        let method_overloads = self.method_overloads_from_manifest(&export.methods);
        ClassInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            extends: export.extends.clone(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(&export.fields),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: std::collections::HashMap::new(),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Convert one manifest trait export into semantic trait metadata.
    fn trait_info_from_manifest(&self, export: &TraitExport) -> TraitInfo {
        TraitInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            supertraits: export
                .supertraits
                .iter()
                .map(|bound| {
                    (
                        bound.name.clone(),
                        bound
                            .type_args
                            .iter()
                            .map(resolved_type_from_manifest_type_ref)
                            .collect(),
                    )
                })
                .collect(),
            methods: self.methods_from_manifest(&export.methods),
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
            properties: std::collections::HashMap::new(),
            requires: export
                .requires
                .iter()
                .map(|required| {
                    (
                        required.name.clone(),
                        resolved_type_from_manifest_type_ref(&required.ty),
                    )
                })
                .collect(),
        }
    }

    /// Convert manifest trait adoption metadata, falling back to legacy trait-name-only manifests.
    fn trait_adoptions_from_manifest(
        trait_names: &[String],
        trait_adoptions: &[TypeBoundExport],
    ) -> Vec<TypeBoundInfo> {
        if trait_adoptions.is_empty() {
            return trait_names
                .iter()
                .map(|name| TypeBoundInfo {
                    name: name.clone(),
                    source_name: None,
                    type_args: Vec::new(),
                    module_path: None,
                })
                .collect();
        }

        trait_adoptions
            .iter()
            .map(|bound| TypeBoundInfo {
                name: bound.name.clone(),
                source_name: bound.source_name.clone(),
                type_args: bound
                    .type_args
                    .iter()
                    .map(resolved_type_from_manifest_type_ref)
                    .collect(),
                module_path: bound.module_path.clone(),
            })
            .collect()
    }

    /// Apply a consumer namespace grant to provider-local trait provenance carried by checked artifact facts.
    fn qualify_provider_symbol_bounds(kind: &mut SymbolKind, namespace_prefix: &[String]) {
        match kind {
            SymbolKind::Function(info) => Self::qualify_provider_function_bounds(info, namespace_prefix),
            SymbolKind::FunctionOverloads(overloads) => {
                for overload in overloads {
                    Self::qualify_provider_function_bounds(&mut overload.info, namespace_prefix);
                }
            }
            SymbolKind::Type(TypeInfo::Model(info)) => {
                Self::qualify_provider_type_bounds(
                    &mut info.trait_adoptions,
                    &mut info.methods,
                    &mut info.method_overloads,
                    namespace_prefix,
                );
            }
            SymbolKind::Type(TypeInfo::Class(info)) => {
                Self::qualify_provider_type_bounds(
                    &mut info.trait_adoptions,
                    &mut info.methods,
                    &mut info.method_overloads,
                    namespace_prefix,
                );
            }
            SymbolKind::Type(TypeInfo::Newtype(info)) => {
                Self::qualify_provider_type_bounds(
                    &mut info.trait_adoptions,
                    &mut info.methods,
                    &mut info.method_overloads,
                    namespace_prefix,
                );
            }
            SymbolKind::Type(TypeInfo::Enum(info)) => {
                Self::qualify_provider_type_bounds(
                    &mut info.trait_adoptions,
                    &mut info.methods,
                    &mut info.method_overloads,
                    namespace_prefix,
                );
            }
            SymbolKind::Trait(info) => {
                for method in info.methods.values_mut() {
                    Self::qualify_provider_method_bounds(method, namespace_prefix);
                }
            }
            SymbolKind::Type(TypeInfo::Builtin | TypeInfo::TypeAlias)
            | SymbolKind::Variable(_)
            | SymbolKind::Static(_)
            | SymbolKind::Module(_)
            | SymbolKind::Variant(_)
            | SymbolKind::Field(_)
            | SymbolKind::Property(_)
            | SymbolKind::RustItem(_) => {}
        }
    }

    /// Qualify one provider-owned nominal type's trait adoptions and callable bounds.
    fn qualify_provider_type_bounds(
        adoptions: &mut [TypeBoundInfo],
        methods: &mut HashMap<String, MethodInfo>,
        overloads: &mut HashMap<String, Vec<MethodInfo>>,
        namespace_prefix: &[String],
    ) {
        for adoption in adoptions {
            Self::qualify_provider_bound(adoption, namespace_prefix);
        }
        for method in methods.values_mut() {
            Self::qualify_provider_method_bounds(method, namespace_prefix);
        }
        for candidates in overloads.values_mut() {
            for method in candidates {
                Self::qualify_provider_method_bounds(method, namespace_prefix);
            }
        }
    }

    /// Qualify generic bounds on one provider function.
    fn qualify_provider_function_bounds(info: &mut FunctionInfo, namespace_prefix: &[String]) {
        for bounds in info.type_param_bound_details.values_mut() {
            for bound in bounds {
                Self::qualify_provider_bound(bound, namespace_prefix);
            }
        }
    }

    /// Qualify generic and explicit trait-target bounds on one provider method.
    fn qualify_provider_method_bounds(info: &mut MethodInfo, namespace_prefix: &[String]) {
        if let Some(target) = info.trait_target.as_mut() {
            Self::qualify_provider_bound(target, namespace_prefix);
        }
        for bounds in info.type_param_bound_details.values_mut() {
            for bound in bounds {
                Self::qualify_provider_bound(bound, namespace_prefix);
            }
        }
    }

    /// Prefix a provider-local module path while preserving already canonical external provenance.
    fn qualify_provider_bound(bound: &mut TypeBoundInfo, namespace_prefix: &[String]) {
        let Some(module_path) = bound.module_path.as_mut() else {
            return;
        };
        if module_path.starts_with(namespace_prefix)
            || module_path
                .first()
                .is_some_and(|root| matches!(root.as_str(), "std" | "pub" | "rust" | "crate"))
        {
            return;
        }
        let mut canonical = namespace_prefix.to_vec();
        canonical.append(module_path);
        *module_path = canonical;
    }

    /// Convert a manifest enum export into local enum symbol metadata.
    fn enum_info_from_manifest(&self, export: &EnumExport) -> EnumInfo {
        let value_enum = export.value_type.map(|value_type| ValueEnumInfo {
            value_type: match value_type {
                EnumValueTypeExport::Str => ValueEnumBacking::Str,
                EnumValueTypeExport::Int => ValueEnumBacking::Int,
            },
            values: export
                .variants
                .iter()
                .filter_map(|variant| {
                    let value = match variant.value.as_ref()? {
                        EnumValueExport::Str(value) => ValueEnumValue::Str(value.clone()),
                        EnumValueExport::Int(value) => ValueEnumValue::Int(*value),
                    };
                    Some((variant.name.clone(), value))
                })
                .collect(),
        });

        EnumInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            variants: export.variants.iter().map(|variant| variant.name.clone()).collect(),
            variant_fields: export
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant
                            .fields
                            .iter()
                            .map(resolved_type_from_manifest_type_ref)
                            .collect(),
                    )
                })
                .collect(),
            variant_aliases: export
                .variant_aliases
                .iter()
                .map(|alias| (alias.name.clone(), alias.target.clone()))
                .collect(),
            value_enum,
            derives: export.derives.clone(),
            method_overloads: self.method_overloads_from_manifest(&export.methods),
            methods: self.methods_from_manifest(&export.methods),
        }
    }

    /// Convert a manifest newtype export into local typechecker metadata.
    fn newtype_info_from_manifest(&self, export: &NewtypeExport) -> NewtypeInfo {
        NewtypeInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            is_rusttype: export.is_rusttype,
            has_interop: false,
            underlying: resolved_type_from_manifest_type_ref(&export.underlying),
            constraints: export
                .constraints
                .iter()
                .map(|constraint| constraint.to_checked())
                .collect(),
            implicit_coercion_enabled: export.implicit_coercion_enabled,
            method_rebindings: std::collections::HashMap::new(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
            methods: self.methods_from_manifest(&export.methods),
            method_overloads: self.method_overloads_from_manifest(&export.methods),
        }
    }

    fn type_param_bounds_from_manifest(
        &self,
        type_params: &[TypeParamExport],
    ) -> std::collections::HashMap<String, Vec<String>> {
        type_params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.bounds.iter().map(|bound| bound.name.clone()).collect(),
                )
            })
            .collect()
    }

    /// Convert manifest type-parameter bounds while preserving generic trait arguments.
    fn type_param_bound_details_from_manifest(
        &self,
        type_params: &[TypeParamExport],
    ) -> std::collections::HashMap<String, Vec<TypeBoundInfo>> {
        type_params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param
                        .bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: bound.name.clone(),
                            source_name: bound.source_name.clone(),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(resolved_type_from_manifest_type_ref)
                                .collect(),
                            module_path: bound.module_path.clone(),
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Convert exported manifest fields into semantic field metadata for imported-library typechecking.
    fn fields_from_manifest(&self, fields: &[FieldExport]) -> std::collections::HashMap<String, FieldInfo> {
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    FieldInfo {
                        ty: resolved_type_from_manifest_type_ref(&field.ty),
                        visibility: crate::frontend::ast::Visibility::Public,
                        owner: None,
                        has_default: field.has_default,
                        alias: field.alias.clone(),
                        description: field.description.clone(),
                    },
                )
            })
            .collect()
    }

    /// Convert manifest methods into the legacy single-method-per-name lookup map.
    fn methods_from_manifest(&self, methods: &[MethodExport]) -> std::collections::HashMap<String, MethodInfo> {
        methods
            .iter()
            .map(|method| (method.name.clone(), self.method_info_from_manifest(method)))
            .collect()
    }

    /// Preserve same-type aliases so lowering can emit the canonical Rust method from a compiled dependency.
    fn method_aliases_from_manifest(methods: &[MethodExport]) -> HashMap<String, String> {
        methods
            .iter()
            .filter_map(|method| {
                method
                    .alias_of
                    .as_ref()
                    .map(|target| (method.name.clone(), target.clone()))
            })
            .collect()
    }

    /// Group manifest methods by name without dropping same-name trait-backed overloads.
    fn method_overloads_from_manifest(
        &self,
        methods: &[MethodExport],
    ) -> std::collections::HashMap<String, Vec<MethodInfo>> {
        let mut groups: std::collections::HashMap<String, Vec<MethodInfo>> = std::collections::HashMap::new();
        for method in methods {
            groups
                .entry(method.name.clone())
                .or_default()
                .push(self.method_info_from_manifest(method));
        }
        groups
    }

    /// Convert one manifest method export into semantic method metadata.
    fn method_info_from_manifest(&self, method: &MethodExport) -> MethodInfo {
        MethodInfo {
            type_params: method.type_params.iter().map(|tp| tp.name.clone()).collect(),
            type_param_bounds: method
                .type_params
                .iter()
                .map(|tp| {
                    (
                        tp.name.clone(),
                        tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
                    )
                })
                .collect(),
            type_param_bound_details: method
                .type_params
                .iter()
                .map(|tp| {
                    (
                        tp.name.clone(),
                        tp.bounds
                            .iter()
                            .map(|bound| TypeBoundInfo {
                                name: bound.name.clone(),
                                source_name: bound.source_name.clone(),
                                type_args: bound
                                    .type_args
                                    .iter()
                                    .map(resolved_type_from_manifest_type_ref)
                                    .collect(),
                                module_path: bound.module_path.clone(),
                            })
                            .collect(),
                    )
                })
                .collect(),
            trait_target: None,
            receiver: self.receiver_from_manifest(method.receiver.as_ref()),
            params: self.params_from_manifest(&method.params),
            return_type: resolved_type_from_manifest_type_ref(&method.return_type),
            is_async: method.is_async,
            has_body: method.has_body,
            alias_of: method.alias_of.clone(),
        }
    }

    /// Convert manifest parameters into checked callable parameters.
    fn params_from_manifest(&self, params: &[ParamExport]) -> Vec<CallableParam> {
        params
            .iter()
            .map(|param| {
                CallableParam::named_with_default(
                    param.name.clone(),
                    resolved_type_from_manifest_type_ref(&param.ty),
                    param_kind_from_manifest(param.kind),
                    param
                        .default
                        .as_ref()
                        .map_or(param.has_default, ParamDefaultExport::is_materializable),
                )
            })
            .collect()
    }

    fn receiver_from_manifest(&self, receiver: Option<&ReceiverExport>) -> Option<Receiver> {
        match receiver {
            Some(ReceiverExport::Immutable) => Some(Receiver::Immutable),
            Some(ReceiverExport::Mutable) => Some(Receiver::Mutable),
            None => None,
        }
    }

    /// Ensure imported items are public in the dependency module.
    fn validate_import_visibility(&mut self, import: &ImportDecl, span: Span) {
        let ImportKind::From { module, items } = &import.kind else {
            return;
        };

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
                | ExportedSymbol::Reexported(name) => {
                    exported_names.insert(name.clone());
                }
                ExportedSymbol::Variant { variant_name, .. } => {
                    exported_names.insert(variant_name.clone());
                }
            }
        }

        let exported_list: Vec<String> = exported_names.iter().cloned().collect();

        for item in items {
            if !exported_names.contains(&item.name)
                && self.dependency_member_symbol_for_path(module, &item.name).is_none()
            {
                self.errors.push(errors::import_not_exported(
                    &item.name,
                    &module.to_rust_path(),
                    &exported_list,
                    span,
                ));
            }
        }
    }

    /// Emit the RFC 005 diagnostic for unsupported `rust::core` / `rust::alloc` imports.
    ///
    /// Returns `true` when the crate is unsupported and an error was emitted.
    fn reject_unsupported_rust_core_alloc(&mut self, crate_name: &str, span: Span) -> bool {
        if crate_name == "core" || crate_name == "alloc" {
            self.errors.push(errors::unsupported_rust_core_alloc(crate_name, span));
            return true;
        }
        false
    }

    /// Build a full Rust import path vector from crate, optional module path, and optional item name.
    fn rust_import_full_path(&self, crate_name: &str, path: &[Ident], item: Option<&str>) -> Vec<Ident> {
        let mut full_path = vec![crate_name.to_string()];
        full_path.extend(path.to_vec());
        if let Some(item_name) = item {
            full_path.push(item_name.to_string());
        }
        full_path
    }

    /// Validate and register a Rust import symbol for codegen and RFC 041 provenance.
    fn define_rust_import_binding(&mut self, name: Ident, info: RustItemInfo, span: Span) {
        self.validate_root_namespace(&name, span);
        let mut trait_methods = HashSet::new();
        let mut trait_method_signatures = std::collections::HashMap::new();
        if let Some(metadata) = &info.metadata
            && let RustItemKind::Trait(trait_info) = &metadata.kind
        {
            for item in &trait_info.items {
                match item {
                    RustTraitAssoc::Function { name, signature } => {
                        trait_methods.insert(name.clone());
                        trait_method_signatures.insert(name.clone(), signature.clone());
                    }
                    RustTraitAssoc::TypeAlias { .. } | RustTraitAssoc::Constant { .. } => {}
                }
            }
        }
        if trait_methods.is_empty() {
            trait_methods.extend(
                fallback_rust_trait_methods(info.path.as_str())
                    .iter()
                    .map(|method| (*method).to_string()),
            );
        }
        if !trait_methods.is_empty() {
            self.type_info.rust.trait_imports.insert(
                name.clone(),
                RustTraitImportInfo {
                    trait_path: info.path.clone(),
                    definition_path: info
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.definition_path.clone()),
                    methods: trait_methods,
                    method_signatures: trait_method_signatures,
                },
            );
        }
        self.define_rust_import_symbol(name, info, span);
    }

    /// Define a symbol for a Rust crate import.
    ///
    /// Explicit Rust imports must be allowed to shadow dependency-exported Incan types with the same simple name. This
    /// matters for Rust metadata display types such as `Duration`, where the current module's `from rust::... import
    /// Duration` is the only reliable hint that an unqualified metadata return type means `std::time::Duration`.
    fn define_rust_import_symbol(&mut self, name: Ident, info: RustItemInfo, span: Span) {
        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::RustItem(info),
            span,
            scope: 0, // Will be set by define()
        });
    }

    /// Define a symbol for a module import, skipping if a real definition exists.
    fn define_import_symbol(&mut self, name: Ident, path: Vec<Ident>, is_python: bool, span: Span) {
        if self.has_real_definition(&name) {
            return;
        }
        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::Module(ModuleInfo { path, is_python }),
            span,
            scope: 0,
        });
    }

    /// Returns the existing symbol kind for a `from ... import ...` item when it resolves to a concrete, non-implicit
    /// symbol in the current compilation context.
    fn existing_from_import_symbol_kind(&self, name: &str) -> Option<SymbolKind> {
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
    fn mark_static_binding_imported(&mut self, name: &str) {
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
                crate::frontend::typechecker::StaticBindingInfo { is_imported: true },
            );
        }
    }

    /// Returns `true` if `name` already resolves to a real definition that should not be overwritten by a module
    /// placeholder.
    fn has_real_definition(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|sym| {
            matches!(
                sym.kind,
                SymbolKind::Type(_)
                    | SymbolKind::Function(_)
                    | SymbolKind::Trait(_)
                    | SymbolKind::Variant(_)
                    | SymbolKind::Variable(_)
                    | SymbolKind::Static(_)
            )
        })
    }
}

/// Convert a manifest parameter kind into a checked parameter kind.
fn param_kind_from_manifest(kind: ParamKindExport) -> ParamKind {
    match kind {
        ParamKindExport::Normal => ParamKind::Normal,
        ParamKindExport::RestPositional => ParamKind::RestPositional,
        ParamKindExport::RestKeyword => ParamKind::RestKeyword,
    }
}

#[cfg(test)]
mod provider_feature_tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::library_manifest::{ProviderFactKind, ProviderFactRequirement};

    #[test]
    fn inactive_export_features_preserve_alternative_additive_paths() {
        let mut manifest = LibraryManifest::new("widgets".to_string(), "0.1.0".to_string());
        manifest.contract_metadata.provider.fact_requirements = vec![
            ProviderFactRequirement {
                kind: ProviderFactKind::Export,
                identity: "leaf::Leaf".to_string(),
                required_features: BTreeSet::from(["alternate".to_string()]),
            },
            ProviderFactRequirement {
                kind: ProviderFactKind::Export,
                identity: "leaf::Leaf".to_string(),
                required_features: BTreeSet::from(["inner".to_string(), "outer".to_string()]),
            },
        ];

        assert_eq!(
            TypeChecker::inactive_pub_export_features(&manifest, "Leaf"),
            Some(vec![
                vec!["alternate".to_string()],
                vec!["inner".to_string(), "outer".to_string()],
            ])
        );
    }
}
