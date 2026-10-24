//! Imports from compiled Loaves (DD-0007): `pub::<key>` dependencies and the `std.*` namespaces of the standard
//! packages, both read from the Loaf's checked manifest and bound under the Loaf's binding root.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::api_metadata::{
    ApiDeclaration, CheckedApiMetadataPackage, DecoratorArgMetadata, DecoratorValue, SafeMetadataValue,
    checked_api_declaration_is_public_namespace_member, checked_api_modules_for_public_namespace,
    checked_api_public_module_paths, checked_api_public_namespace, class_export_from_api, enum_export_from_api,
    function_export_from_api, function_export_from_api_projected, manifest_partial_with_target_defaults,
    model_export_from_api, newtype_export_from_api, partial_export_from_api, partial_export_with_target_defaults,
    trait_export_from_api,
};
use crate::ast::*;
use crate::diagnostics::errors;
use crate::library_exports::{
    CheckedParamDefault, CheckedParamDefaultArg, CheckedParamDefaultCallSignature, CheckedPresetValue,
};
use crate::library_manifest::{
    AliasExport, ClassExport, ConstExport, EnumExport, EnumValueExport, EnumValueTypeExport, FieldExport,
    FunctionExport, ImplementationTraitBoundOriginExport, ImplementationTypeParamExport, LibraryManifest, MethodExport,
    ModelExport, NewtypeExport, NominalTypeOriginExport, ParamDefaultExport, ParamExport, ParamKindExport,
    PartialExport, PartialTargetKindExport, PresetValueExport, PropertyExport, ProviderFactKind, ReceiverExport,
    StaticExport, TraitExport, TypeAliasExport, TypeBoundExport, TypeParamExport, resolved_type_from_manifest_type_ref,
};
use crate::loaf_index::LoafEntry;
use crate::symbols::*;
use crate::testing_markers::testing_marker_semantics_from_manifest;
use crate::typechecker::{
    PartialProjectionInfo, PartialProjectionPreset, PartialProjectionTargetKind, PublicLibraryTypeIdentity,
    TypeChecker, canonical_public_library_type_name,
};
use incan_lang::interop::is_rust_capability_bound;
use incan_lang::lang::decorators::{self as core_decorators, DecoratorId};
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::functions as surface_functions;
use incan_lang::lang::surface::types as surface_types;
use incan_semantics_core::{CanonicalSymbolId, DecoratorFeature, SurfaceFeatureKey, SymbolOrigin};

/// One import that reaches a compiled Loaf: the Loaf's index key and the public module path inside its manifest.
pub(in crate::typechecker) struct LoafImport {
    /// The Loaf's index key: the dependency key, or the standard package's name.
    pub key: String,
    /// The public module path inside the Loaf's manifest; empty for the Loaf's root.
    pub module: Vec<Ident>,
}

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
        canonical: Option<&'a crate::library_manifest::CanonicalIdentityExport>,
    },
    TypeAlias(&'a TypeAliasExport),
    Newtype(&'a NewtypeExport),
    Const(&'a ConstExport),
    Static(&'a StaticExport),
}

struct PublicModuleMember {
    kind: SymbolKind,
    canonical: Option<CanonicalSymbolId>,
    source_module_path: Vec<String>,
    source_name: String,
    type_alias: Option<(Vec<TypeParamExport>, ResolvedType)>,
    partial_projection: Option<PartialProjectionInfo>,
}

/// Shared package context while following a public alias chain to partial projection metadata.
struct PartialProjectionAliasContext<'a> {
    library: &'a str,
    manifest: &'a LibraryManifest,
    local_name: &'a str,
    imported_type_aliases: &'a HashMap<String, String>,
    span: Span,
}

/// Exact checked identity returned when resolving a declaration through a public package namespace.
pub(in crate::typechecker) struct ResolvedPublicModuleSymbol {
    pub kind: SymbolKind,
    pub canonical: Option<CanonicalSymbolId>,
    pub source_module_path: Vec<String>,
    pub source_name: String,
}

impl TypeChecker {
    /// Resolve the ambient logger through its ordinary factory's checked return type. This keeps its nominal
    /// identity and method parameter context independent of whether the caller explicitly imports `Logger`.
    pub(in crate::typechecker) fn ambient_logger_type(&mut self) -> Option<ResolvedType> {
        let module = vec!["std".to_string(), "logging".to_string()];
        let (library, path) = self.loaf_module(&module)?;
        let (SymbolKind::Function(factory), _) =
            self.lookup_pub_library_module_symbol_member(&library, &path, "get_logger")?
        else {
            return None;
        };
        Some(factory.return_type)
    }

    /// The trait a Loaf publishes as `<module_path>.<name>`, read from its manifest.
    pub(in crate::typechecker) fn loaf_trait(&mut self, module_path: &[String], name: &str) -> Option<TraitInfo> {
        let (library, rest) = self.loaf_module(module_path)?;
        match self.lookup_pub_library_module_symbol_member(&library, &rest, name)?.0 {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }

    /// The Rust derive paths a Loaf trait declares (`@rust.derive(...)`), in declaration order.
    pub(in crate::typechecker) fn loaf_trait_rust_derive_paths(
        &self,
        module_path: &[String],
        name: &str,
    ) -> Option<Vec<String>> {
        let (library, _) = self.loaf_module(module_path)?;
        let manifest = self.loaves.manifest(&library)?;
        let canonical = self.loaf_member_identity(module_path, name)?;
        let source_path = manifest
            .contract_metadata
            .identity_graph
            .declaration_source_path(&canonical)?;
        let ApiDeclaration::Trait(declaration) = Self::api_declaration_for_target_path(manifest, source_path)? else {
            return None;
        };
        let mut paths = Vec::new();
        for decorator in &declaration.decorators {
            if core_decorators::from_str(&decorator.path.join(".")) != Some(DecoratorId::RustDerive) {
                continue;
            }
            for argument in &decorator.args {
                if let DecoratorArgMetadata::Positional {
                    value:
                        DecoratorValue::Literal {
                            value: SafeMetadataValue::String(path),
                        },
                } = argument
                    && !paths.contains(path)
                {
                    paths.push(path.clone());
                }
            }
        }
        Some(paths)
    }

    /// The RFC 024 `__derives__` trait list a Loaf module publishes in its checked API metadata.
    pub(in crate::typechecker) fn loaf_module_derivable_traits(&self, module_path: &[String]) -> Option<Vec<String>> {
        let (library, rest) = self.loaf_module(module_path)?;
        let api = self.loaves.manifest(&library)?.contract_metadata.api.as_ref()?;
        api.modules
            .iter()
            .find(|module| module.module_path == rest)
            .map(|module| module.derivable_traits.clone())
    }

    /// Record the standard `Registry` declarations when this module defines, imports or enters a registry, so lowering
    /// selects the registry's reserved methods from the checked declaration.
    pub(in crate::typechecker) fn record_standard_registry_types(&mut self) {
        let registry = &self.type_info.registry;
        if registry.definitions.is_empty()
            && registry.imported_definitions.is_empty()
            && registry.explicit_entries.is_empty()
        {
            return;
        }
        let module = vec![stdlib::STDLIB_ROOT.to_string(), stdlib::STDLIB_REGISTRY.to_string()];
        let standard = if self.owns_standard_module(&module) {
            self.own_standard_registry_types(&module)
        } else {
            self.loaf_standard_registry_types(&module)
        };
        if let Some(standard) = standard {
            self.type_info.registry.standard = Some(standard);
        }
    }

    /// Whether the active binding denotes this exact Loaf declaration or its registry-owned builtin placeholder.
    /// A placeholder is compatible with its declaring module, but still needs that module's checked contract.
    fn rebinds_same_declaration(&self, local_name: &str, module: &[String], item: &str) -> bool {
        let Some(symbol) = self.symbols.lookup_local(local_name) else {
            return false;
        };
        match self.symbols.identity_of(symbol) {
            Some(existing) if !matches!(existing.origin, SymbolOrigin::Builtin) => {
                self.loaf_member_identity(module, item).as_ref() == Some(existing)
            }
            // A compiler-owned placeholder may be imported through a facade only when both exports name the
            // registered declaring module's exact trait. A matching short name grants no declaration authority.
            Some(existing) if existing.origin == SymbolOrigin::Builtin => {
                if local_name != item
                    || !matches!(
                        self.symbols.get(symbol).map(|found| &found.kind),
                        Some(SymbolKind::Trait(_))
                    )
                {
                    return false;
                }
                let Some(declaring) =
                    incan_lang::lang::traits::from_str(item).and_then(incan_lang::lang::traits::source_module)
                else {
                    return false;
                };
                let declaring = declaring.split('.').map(str::to_owned).collect::<Vec<_>>();
                let Some(imported) = self.loaf_member_identity(module, item) else {
                    return false;
                };
                self.loaf_member_identity(&declaring, item).as_ref() == Some(&imported)
            }
            _ => false,
        }
    }

    /// Keep real imports idempotent while replacing a compatible builtin placeholder with checked Loaf metadata.
    /// A bare spelling is never sufficient: only the builtin registry's declaring module can supply its contract.
    fn loaf_import_needs_binding(&mut self, local_name: &str, module: &[String], item: &str, span: Span) -> bool {
        let Some(existing_kind) = self.existing_local_symbol_kind(local_name) else {
            return true;
        };
        if self.rebinds_same_declaration(local_name, module, item) {
            return self
                .symbols
                .lookup_local(local_name)
                .and_then(|symbol| self.symbols.identity_of(symbol))
                .is_some_and(|identity| matches!(identity.origin, SymbolOrigin::Builtin));
        }
        self.errors.push(errors::pub_library_import_name_collision(
            local_name,
            existing_kind,
            span,
        ));
        false
    }

    /// The standard `Registry` declarations of the package that declares `std.registry`: its own checked modules.
    fn own_standard_registry_types(&self, module: &[String]) -> Option<crate::typechecker::StandardRegistryTypes> {
        let path = ImportPath {
            parent_levels: 0,
            is_absolute: false,
            segments: module.to_vec(),
        };
        let Some(SymbolKind::Type(TypeInfo::Class(registry_class))) =
            self.dependency_member_symbol_for_path(&path, "Registry")
        else {
            return None;
        };
        Some(crate::typechecker::StandardRegistryTypes {
            registry: self.dependency_member_identity(&path, "Registry")?,
            registry_class,
            entry: self.dependency_member_identity(&path, "RegistryEntry")?,
        })
    }

    /// The standard `Registry` declarations, read from the standard Loaf that owns `std.registry`.
    fn loaf_standard_registry_types(&mut self, module: &[String]) -> Option<crate::typechecker::StandardRegistryTypes> {
        let (library, rest) = self.loaf_module(module)?;
        let Some((SymbolKind::Type(TypeInfo::Class(registry_class)), _)) =
            self.lookup_pub_library_module_symbol_member(&library, &rest, "Registry")
        else {
            return None;
        };
        Some(crate::typechecker::StandardRegistryTypes {
            registry: self.loaf_member_identity(module, "Registry")?,
            registry_class,
            entry: self.loaf_member_identity(module, "RegistryEntry")?,
        })
    }

    /// The standard declaration of the builtin trait `name` (`Eq`, `Ord`, …), read once from the standard package
    /// owning its module.
    pub(in crate::typechecker) fn standard_trait(&mut self, name: &str) -> Option<TraitInfo> {
        if let Some(info) = self.standard_traits.get(name) {
            return Some(info.clone());
        }
        // The derive-method table names a derivable trait's module; every other builtin trait (`TryFrom`, `Error`)
        // names its declaring module itself.
        let segments = stdlib::trait_method_module_segments(name).or_else(|| {
            incan_lang::lang::traits::from_str(name)
                .and_then(incan_lang::lang::traits::source_module)
                .map(|module| module.split('.').map(str::to_string).collect())
        })?;
        let info = self.loaf_trait(&segments, name)?;
        self.standard_traits.insert(name.to_string(), info.clone());
        Some(info)
    }

    /// The path `library`'s public modules are bound under (see [`LoafEntry::binding_root`]).
    pub(in crate::typechecker) fn loaf_binding_root(&self, library: &str) -> Vec<String> {
        self.loaves
            .entry(library)
            .map(LoafEntry::binding_root)
            .unwrap_or_else(|| vec!["pub".to_string(), library.to_string()])
    }

    /// Identify an admitted module by its provider package while its lexical binding keeps the dependency alias.
    /// Standard namespaces retain their `std` module origins; a consumer's package alias never becomes a declaring
    /// package in executable requirements.
    fn loaf_module_identity(&self, library: &str, module: &[String]) -> Option<CanonicalSymbolId> {
        let entry = self.loaves.entry(library)?;
        let mut root = entry.binding_root();
        if let [namespace, owner] = root.as_mut_slice()
            && namespace == "pub"
        {
            owner.clone_from(&entry.artifact.name);
        }
        root.extend_from_slice(module);
        SymbolTable::module_path_identity(&root)
    }

    /// The Loaf import a `std.*` module path reaches: the standard package owning its namespace root, and the module
    /// path inside that package's manifest.
    pub(in crate::typechecker) fn standard_loaf_import(&self, segments: &[String]) -> Option<LoafImport> {
        let entry = self.loaves.owner_of(segments)?;
        Some(LoafImport {
            key: entry.key.clone(),
            module: segments[1..].to_vec(),
        })
    }

    /// Bind a compiled module while preserving its written source root for reserved-name validation. A `pub::` import
    /// supplies no standard root exemption, even when its dependency happens to provide standard modules.
    pub(in crate::typechecker) fn collect_loaf_module_import(
        &mut self,
        import: &LoafImport,
        alias: Option<&Ident>,
        source_root: Option<&str>,
        span: Span,
    ) {
        let library = import.key.as_str();
        let name = alias
            .cloned()
            .or_else(|| import.module.last().cloned())
            .unwrap_or_else(|| library.to_string());
        self.validate_module_import_binding(&name, source_root, span);
        let Some(manifest) = self.loaf_manifest_or_refuse(library, span) else {
            return;
        };
        if !import.module.is_empty() && !Self::manifest_public_module_exists(&manifest, &import.module) {
            self.push_loaf_module_not_found(library, &manifest, &import.module, span);
            return;
        }
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let mut canonical_path = self.loaf_binding_root(library);
        canonical_path.extend(import.module.iter().cloned());
        let target_identity = self.loaf_module_identity(library, &import.module);
        self.define_import_symbol(name, canonical_path, false, target_identity, span);
    }

    /// The manifest of the Loaf indexed as `library`, or `None` after refusing the unknown key.
    pub(super) fn loaf_manifest_or_refuse(
        &mut self,
        library: &str,
        span: Span,
    ) -> Option<std::sync::Arc<LibraryManifest>> {
        if let Some(entry) = self.loaves.entry(library) {
            return Some(std::sync::Arc::clone(&entry.artifact.manifest));
        }
        let known = self.loaves.keys().map(str::to_owned).collect::<Vec<_>>();
        self.errors.push(errors::unknown_pub_library(library, &known, span));
        None
    }

    /// Refuse a module path the Loaf's manifest does not publish, listing the modules it does.
    pub(super) fn push_loaf_module_not_found(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        module: &[Ident],
        span: Span,
    ) {
        let available = Self::manifest_public_module_paths(manifest)
            .into_iter()
            .map(|module_path| module_path.join("."))
            .collect::<Vec<_>>();
        self.errors
            .push(errors::pub_library_module_not_found(library, module, &available, span));
    }

    /// The names `from <module> import …` may bind from one Loaf module, for refusals.
    pub(super) fn loaf_module_member_names(manifest: &LibraryManifest, module: &[Ident]) -> Vec<String> {
        if !module.is_empty() {
            return Self::public_module_member_names(manifest, module);
        }
        let mut names = Self::manifest_export_names(manifest);
        names.extend(
            Self::manifest_public_module_paths(manifest)
                .into_iter()
                .filter_map(|module_path| module_path.first().cloned()),
        );
        names.sort();
        names.dedup();
        names
    }

    /// Bind `from <module> import item, …` from a compiled Loaf. Builtin trait placeholders are replaced through
    /// ordinary import binding so the manifest supplies their parameters, declaration identities, and accepted import
    /// paths together.
    pub(in crate::typechecker) fn collect_loaf_from_imports(
        &mut self,
        import: &LoafImport,
        items: &[ImportItem],
        span: Span,
    ) {
        let library = import.key.as_str();
        let path = import.module.as_slice();
        let Some(manifest) = self.loaf_manifest_or_refuse(library, span) else {
            return;
        };
        if !path.is_empty() && !Self::manifest_public_module_exists(&manifest, path) {
            self.push_loaf_module_not_found(library, &manifest, path, span);
            return;
        }
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let available_exports = Self::loaf_module_member_names(&manifest, path);
        let root = self.loaf_binding_root(library);
        let mut source_module = root.clone();
        source_module.extend(path.iter().cloned());
        for item in items {
            let local_name = Self::import_item_local_name(item);
            if self.validate_protected_builtin_binding(&local_name, span) {
                continue;
            }
            self.validate_root_namespace(&local_name, span);
            if !self.loaf_import_needs_binding(&local_name, &source_module, &item.name, span) {
                continue;
            }
            let imported_type_aliases = self.public_library_type_import_remapping(library, &manifest);
            if path.is_empty() {
                self.collect_loaf_root_from_import(import, &manifest, item, local_name, &imported_type_aliases, span);
                continue;
            }
            let mut child_module_path = path.to_vec();
            child_module_path.push(item.name.clone());
            let imports_namespace = Self::manifest_public_module_exists(&manifest, &child_module_path);
            let mut namespace_path = root.clone();
            namespace_path.extend(child_module_path.iter().cloned());

            let resolved = match self.public_module_member(library, &manifest, path, &item.name) {
                Ok(resolved) => resolved,
                Err(source_modules) => {
                    self.errors.push(errors::pub_library_module_member_ambiguous(
                        library,
                        path,
                        &item.name,
                        &source_modules,
                        span,
                    ));
                    continue;
                }
            };
            if let Some(member) = resolved {
                self.define_pub_module_import_symbol(library, local_name.clone(), namespace_path, member, span);
                self.record_standard_surface_import(&root, path, item, &local_name, span);
                continue;
            }
            if imports_namespace {
                let target_identity = self.loaf_module_identity(library, &child_module_path);
                self.define_import_symbol(local_name, namespace_path, false, target_identity, span);
                continue;
            }
            self.errors.push(errors::pub_library_symbol_not_exported(
                &item.name,
                &format!("{}.{}", root.join("."), path.join(".")),
                &available_exports,
                span,
            ));
        }
    }

    /// Bind a package-root member after collision checks. The caller has proved that the import's module path is empty;
    /// namespace/function ambiguity and feature-visibility rules remain shared with ordinary imports.
    fn collect_loaf_root_from_import(
        &mut self,
        import: &LoafImport,
        manifest: &LibraryManifest,
        item: &ImportItem,
        local_name: String,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let library = import.key.as_str();
        let root = self.loaf_binding_root(library);
        let child_module_path = vec![item.name.clone()];
        let imports_namespace = Self::manifest_public_module_exists(manifest, &child_module_path);
        let mut namespace_path = root.clone();
        namespace_path.extend(child_module_path.iter().cloned());
        let flat_function = self.pub_library_function_symbol(manifest, &item.name);
        let flat_export = Self::find_manifest_export(manifest, &item.name);
        if imports_namespace && (flat_function.is_some() || flat_export.is_some()) {
            self.errors.push(errors::pub_library_module_member_ambiguous(
                library,
                &[],
                &item.name,
                &[
                    format!("package-root export `{}`", item.name),
                    format!("module `{}`", child_module_path.join(".")),
                ],
                span,
            ));
            return;
        }
        if imports_namespace {
            let target_identity = self.loaf_module_identity(library, &child_module_path);
            self.define_import_symbol(local_name, namespace_path, false, target_identity, span);
            return;
        }

        if let Some(mut kind) = flat_function {
            let canonical = manifest
                .contract_metadata
                .identity_graph
                .canonical_for_public_name(&item.name);
            self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
            self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);
            let mut binding_path = root;
            binding_path.push(item.name.clone());
            let symbol_id = self.symbols.define_import_binding_at_path(
                Symbol {
                    name: local_name.clone(),
                    kind: kind.clone(),
                    span,
                    scope: 0,
                },
                canonical.clone(),
                binding_path,
            );
            if self.symbols.is_active_lookup_binding(symbol_id) {
                if let Some(identity) = canonical {
                    self.type_info
                        .declarations
                        .resolved_import_identities
                        .insert(local_name.clone(), identity);
                }
                self.record_imported_function_binding(&local_name, &kind);
            }
            return;
        }

        let Some(export) = flat_export else {
            if let Some(feature_sets) = Self::inactive_pub_export_features(manifest, &item.name) {
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
                    &Self::loaf_module_member_names(manifest, &[]),
                    span,
                ));
            }
            return;
        };

        self.define_pub_import_symbol(
            library,
            manifest,
            &item.name,
            local_name,
            export,
            imported_type_aliases,
            span,
        );
    }

    /// Record the compiler-known surface a `std.*` import binds: surface types (`Html`), surface functions and the
    /// `std.testing` markers. Their declarations come from the manifest like any other; this only records which local
    /// name now spells the surface.
    pub(super) fn record_standard_surface_import(
        &mut self,
        root: &[String],
        module: &[Ident],
        item: &ImportItem,
        local_name: &str,
        span: Span,
    ) {
        if root != [stdlib::STDLIB_ROOT] {
            return;
        }
        let Some(symbol_id) = self.symbols.lookup(local_name) else {
            return;
        };
        let dotted = std::iter::once(stdlib::STDLIB_ROOT)
            .chain(module.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(".");
        if let Some(surface_type) = surface_types::from_str(&item.name)
            && surface_types::stdlib_module_path(surface_type)
                .is_some_and(|owner| owner == dotted || (dotted == "std.async" && owner.starts_with("std.async.")))
        {
            self.surface_type_import_bindings
                .insert(local_name.to_string(), (surface_type, symbol_id));
        }
        if let Some(surface_function) = surface_functions::from_str(&item.name) {
            self.surface_function_import_bindings
                .insert(local_name.to_string(), (surface_function, symbol_id));
        }
        if dotted == "std.testing" {
            self.record_testing_marker_import(module, item, local_name, span);
        }
    }

    /// Record an imported `std.testing` marker alias so decorator validation can reject runtime calls consistently.
    ///
    /// The markers are read from the checked metadata of the standard package that owns `std.testing`.
    pub(super) fn record_testing_marker_import(
        &mut self,
        module: &[Ident],
        item: &ImportItem,
        local_name: &str,
        span: Span,
    ) {
        let mut module_path = vec![stdlib::STDLIB_ROOT.to_string()];
        module_path.extend(module.iter().cloned());
        let Some(manifest) = self.loaves.owner_of(&module_path).map(|entry| &entry.artifact.manifest) else {
            return;
        };
        let semantics = match testing_marker_semantics_from_manifest(manifest) {
            Ok(Some(semantics)) => semantics,
            Ok(None) => return,
            Err(error) => {
                self.errors
                    .push(errors::invalid_std_testing_marker_metadata(&error.to_string(), span));
                return;
            }
        };
        let mut marker_path = module_path;
        marker_path.push(item.name.clone());
        let module_feature = self.surface_context.decorator_feature_for_path(&marker_path);
        let is_marker = semantics.marker_kind(&item.name).is_some();
        if module_feature == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction)) && is_marker
        {
            self.testing_marker_import_bindings.insert(local_name.to_string());
        }
        self.testing_marker_semantics = Some(semantics);
    }

    /// Bind `from std.rust import <bound>`: the typechecker-only capability bounds, which no Loaf declares.
    pub(in crate::typechecker) fn collect_capability_bound_import(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        span: Span,
    ) {
        if !is_rust_capability_bound(item.name.as_str()) {
            self.errors.push(errors::stdlib_import_not_exported(
                &item.name,
                &module.segments.join("."),
                span,
            ));
            return;
        }
        self.define_from_import_symbol(
            module,
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
    }

    /// Return the public source spelling for one checked API declaration.
    pub(super) fn api_declaration_name(declaration: &ApiDeclaration) -> &str {
        match declaration {
            ApiDeclaration::Function(item) => &item.name,
            ApiDeclaration::Model(item) => &item.name,
            ApiDeclaration::Class(item) => &item.name,
            ApiDeclaration::Trait(item) => &item.name,
            ApiDeclaration::Enum(item) => &item.name,
            ApiDeclaration::Newtype(item) => &item.name,
            ApiDeclaration::TypeAlias(item) => &item.name,
            ApiDeclaration::Const(item) => &item.name,
            ApiDeclaration::Static(item) => &item.name,
            ApiDeclaration::Alias(item) => &item.name,
            ApiDeclaration::Partial(item) => &item.name,
        }
    }

    /// Return all derived public module namespace paths carried by the checked API artifact.
    pub(super) fn manifest_public_module_paths(manifest: &LibraryManifest) -> Vec<Vec<String>> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        checked_api_public_module_paths(api)
    }

    /// Return whether the checked API artifact contains this public module namespace.
    pub(super) fn manifest_public_module_exists(manifest: &LibraryManifest, module_path: &[String]) -> bool {
        !module_path.is_empty()
            && Self::manifest_public_module_paths(manifest)
                .iter()
                .any(|candidate| candidate == module_path)
    }

    /// Return source API modules whose public declarations participate directly in one derived namespace.
    ///
    /// A directory namespace exposes declarations from its own entrypoint and its immediate source-file children.
    /// Deeper directories remain nested namespaces rather than leaking all descendants into every ancestor.
    pub(super) fn api_modules_for_public_namespace<'a>(
        manifest: &'a LibraryManifest,
        module_path: &[String],
    ) -> Vec<&'a crate::api_metadata::CheckedApiMetadata> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        checked_api_modules_for_public_namespace(api, module_path)
    }

    /// Return the public names available directly from one derived module namespace.
    pub(super) fn public_module_member_names(manifest: &LibraryManifest, module_path: &[String]) -> Vec<String> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        let Some(namespace) = checked_api_public_namespace(api, module_path) else {
            return Vec::new();
        };
        let mut names = namespace
            .members
            .into_iter()
            .map(|member| member.name)
            .chain(namespace.child_modules)
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// Resolve one declaration from a derived public module, preserving its authored source-module identity.
    pub(super) fn public_module_member(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        module_path: &[String],
        member: &str,
    ) -> Result<Option<PublicModuleMember>, Vec<String>> {
        let matches = Self::api_modules_for_public_namespace(manifest, module_path)
            .into_iter()
            .filter_map(|module| {
                let declarations = module
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        checked_api_declaration_is_public_namespace_member(declaration)
                            && Self::api_declaration_name(declaration) == member
                    })
                    .collect::<Vec<_>>();
                (!declarations.is_empty()).then_some((module.module_path.clone(), declarations))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Ok(None);
        }

        let source_modules = matches
            .iter()
            .map(|(source_module, _)| source_module.join("."))
            .collect::<Vec<_>>();
        let (source_module_path, declarations) = match matches.as_slice() {
            [only] => only,
            _ => Self::namespace_own_module_match(module_path, &matches)
                .or_else(|| Self::one_declaration_behind_namespace_matches(manifest, member, &matches))
                .ok_or_else(|| source_modules.clone())?,
        };
        let mut kinds = declarations
            .iter()
            .filter_map(|declaration| match declaration {
                ApiDeclaration::Alias(alias) => self
                    .symbol_kind_from_nominal_projection(alias.projected_type.as_ref())
                    .or_else(|| {
                        self.symbol_kind_from_api_target_path(
                            manifest,
                            Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
                        )
                    }),
                _ => self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration),
            })
            .collect::<Vec<_>>();
        let mut type_alias = declarations
            .iter()
            .find_map(|declaration| Self::public_member_type_alias(manifest, source_module_path, declaration));
        let (resolved_source_module_path, resolved_source_name) =
            declarations
                .iter()
                .find_map(|declaration| match declaration {
                    ApiDeclaration::Alias(alias) => Self::normalized_api_declaration_target(
                        Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
                    ),
                    _ => None,
                })
                .unwrap_or_else(|| (source_module_path.clone(), member.to_string()));
        let public_path = std::iter::once(manifest.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(member.to_string()))
            .collect::<Vec<_>>();
        let source_path = resolved_source_module_path
            .iter()
            .cloned()
            .chain(std::iter::once(resolved_source_name.clone()))
            .collect::<Vec<_>>();
        let overload_identities = manifest
            .contract_metadata
            .identity_graph
            .identities_for_public_source_path(&public_path, &source_path);
        let mut kind = match kinds.as_mut_slice() {
            [kind] => kind.clone(),
            kinds if kinds.iter().all(|kind| matches!(kind, SymbolKind::Function(_))) => SymbolKind::FunctionOverloads(
                kinds
                    .iter()
                    .enumerate()
                    .filter_map(|(index, kind)| match kind {
                        SymbolKind::Function(info) => Some(FunctionOverloadInfo {
                            info: info.clone(),
                            span: Span::default(),
                            identity: overload_identities.get(index).cloned().flatten(),
                        }),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => return Err(source_modules),
        };
        let canonical = overload_identities.first().cloned().flatten().filter(|first| {
            overload_identities
                .iter()
                .all(|identity| identity.as_ref() == Some(first))
        });
        let remapping = self.public_module_type_remapping(library, manifest, &resolved_source_module_path);
        self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        if let Some((_, target)) = &mut type_alias {
            Self::remap_resolved_type_with_import_aliases(target, &remapping);
        }
        let partial_projection = declarations.iter().find_map(|declaration| {
            Self::public_member_partial_projection(
                library,
                manifest,
                source_module_path,
                member,
                declaration,
                &remapping,
            )
        });
        Self::mark_compiled_class_field_provider(&mut kind, library);
        Ok(Some(PublicModuleMember {
            kind,
            canonical,
            source_module_path: resolved_source_module_path,
            source_name: resolved_source_name,
            type_alias,
            partial_projection,
        }))
    }

    /// Read a public alias's type parameters and target through the retained declaration. Native union projections
    /// already carry their closed target and need no second declaration lookup.
    fn public_member_type_alias(
        manifest: &LibraryManifest,
        source_module_path: &[String],
        declaration: &ApiDeclaration,
    ) -> Option<(Vec<TypeParamExport>, ResolvedType)> {
        if let ApiDeclaration::Alias(alias) = declaration
            && let Some(target) = alias.projected_type.as_ref().filter(|ty| ty.has_native_union())
        {
            return Some((Vec::new(), resolved_type_from_manifest_type_ref(target)));
        }
        let declaration = match declaration {
            ApiDeclaration::Alias(alias) => Self::api_declaration_for_target_path(
                manifest,
                Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
            )?,
            declaration => declaration,
        };
        match declaration {
            ApiDeclaration::TypeAlias(item) => Some((
                item.type_alias.type_params.clone(),
                resolved_type_from_manifest_type_ref(&item.type_alias.target),
            )),
            _ => None,
        }
    }

    /// Preserve partial application metadata when a public namespace reaches it through a declaration alias.
    fn public_member_partial_projection(
        library: &str,
        manifest: &LibraryManifest,
        source_module_path: &[String],
        member: &str,
        declaration: &ApiDeclaration,
        remapping: &HashMap<String, String>,
    ) -> Option<PartialProjectionInfo> {
        let declaration = match declaration {
            ApiDeclaration::Alias(alias) => Self::api_declaration_for_target_path(
                manifest,
                Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
            )?,
            declaration => declaration,
        };
        let ApiDeclaration::Partial(partial) = declaration else {
            return None;
        };
        let export = partial_export_from_api(partial);
        Self::partial_projection_from_manifest_partial(&export, member, remapping, Span::default(), library)
    }

    /// Choose one of several source modules that each publish `member` into the same derived namespace, when the
    /// identity graph says they all publish the same declarations.
    ///
    /// Select the declaration the namespace's own module makes when an immediate child module declares the same name.
    ///
    /// A module such as `snappy` that owns the default surface of its namespace and keeps an alternative (`snappy.raw`)
    /// in a child module is not ambiguous: the own module's declaration takes precedence, as a declared member does
    /// over a child namespace, and the child stays reachable through its exact module path. Two child modules that
    /// declare the name remain ambiguous.
    pub(super) fn namespace_own_module_match<'m, 'a>(
        namespace_path: &[String],
        matches: &'m [(Vec<String>, Vec<&'a ApiDeclaration>)],
    ) -> Option<&'m (Vec<String>, Vec<&'a ApiDeclaration>)> {
        matches
            .iter()
            .find(|(module_path, _)| module_path.as_slice() == namespace_path)
    }

    /// A directory namespace exposes its entrypoint's declarations and its source-file children's, so a facade that
    /// re-exports a child's declaration (`pkg/mod.incn` holding `pub from crate.pkg.inner import Greeting`) puts the
    /// name there twice. Each source module publishes the member at its own namespace path, and the identity graph
    /// records the canonical declarations there; when every module's set is the same, the spellings name one
    /// declaration and nothing is ambiguous. The declaring module's match is preferred, so the member keeps its
    /// declaration rather than a forwarding alias. Different or unproven identities stay ambiguous.
    pub(super) fn one_declaration_behind_namespace_matches<'m, 'a>(
        manifest: &LibraryManifest,
        member: &str,
        matches: &'m [(Vec<String>, Vec<&'a ApiDeclaration>)],
    ) -> Option<&'m (Vec<String>, Vec<&'a ApiDeclaration>)> {
        let graph = &manifest.contract_metadata.identity_graph;
        let canonicals_of = |module_path: &[String]| {
            let public_path = std::iter::once(manifest.name.clone())
                .chain(module_path.iter().cloned())
                .chain(std::iter::once(member.to_string()))
                .collect::<Vec<_>>();
            let mut canonicals = graph.canonicals_at_public_path(&public_path);
            canonicals.sort();
            canonicals
        };
        let (first, rest) = matches.split_first()?;
        let published = canonicals_of(&first.0);
        if published.is_empty()
            || rest
                .iter()
                .any(|(module_path, _)| canonicals_of(module_path) != published)
        {
            return None;
        }
        matches
            .iter()
            .find(|(_, declarations)| {
                declarations
                    .iter()
                    .any(|declaration| !matches!(declaration, ApiDeclaration::Alias(_)))
            })
            .or(Some(first))
    }

    /// Build canonical and local type spellings for API declarations consumed through public module namespaces.
    pub(super) fn public_module_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        owner_module_path: &[String],
    ) -> HashMap<String, String> {
        let mut remapping = self.public_library_canonical_type_remapping(library, manifest);
        self.register_pub_library_declarations(library, manifest, &remapping);
        remapping.extend(self.pub_library_declaration_scope(library, manifest, owner_module_path));
        remapping
    }

    /// Define one declaration imported through a derived public module namespace, retaining provider identity for
    /// both nominal types and trait types so an imported alias matches its declaring-module signature spelling.
    pub(super) fn define_pub_module_import_symbol(
        &mut self,
        library: &str,
        local_name: String,
        binding_path: Vec<String>,
        member: PublicModuleMember,
        span: Span,
    ) {
        let type_alias_target = member
            .type_alias
            .map(|(type_params, target)| crate::typechecker::TypeAliasTarget {
                type_params: type_params.iter().map(|param| param.name.clone()).collect(),
                target,
                nominal_identities: Default::default(),
            });
        let mut partial_projection = member.partial_projection;
        let canonical = member.canonical;
        let source_module_path = member.source_module_path;
        let source_name = member.source_name;
        let mut kind = member.kind;
        Self::mark_compiled_class_field_provider(&mut kind, library);
        let symbol_id = self.symbols.define_import_binding_at_path(
            Symbol {
                name: local_name.clone(),
                kind: kind.clone(),
                span,
                scope: 0,
            },
            canonical.clone(),
            binding_path,
        );
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        if let Some(identity) = canonical {
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name.clone(), identity);
        }

        if let Some(target) = type_alias_target {
            self.record_dependency_import_type_alias_before_change(&local_name);
            self.type_aliases.insert(local_name.clone(), target);
        }
        self.record_imported_function_binding(&local_name, &kind);
        if let Some(projection) = &mut partial_projection {
            projection.name.clone_from(&local_name);
        }
        if let Some(projection) = partial_projection {
            self.type_info.record_partial_projection(projection);
        }
        if let Some(target_kind) = Self::source_target_kind(&kind) {
            let mut target_path = self.loaf_binding_root(library);
            target_path.extend(source_module_path.clone());
            self.source_import_targets.insert(
                local_name.clone(),
                crate::typechecker::SourceTargetInfo {
                    module_path: target_path,
                    name: source_name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
        if matches!(kind, SymbolKind::Static(_)) {
            self.type_info.declarations.static_bindings.insert(
                local_name.clone(),
                crate::typechecker::StaticBindingInfo { is_imported: true },
            );
        }
        if matches!(
            kind,
            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
                | SymbolKind::Trait(_)
        ) {
            let mut source_path = source_module_path;
            source_path.push(source_name);
            let declaration_identity = self
                .type_info
                .declarations
                .resolved_import_identities
                .get(&local_name)
                .cloned();
            self.public_library_type_identities.insert(
                local_name,
                self.admitted_public_type_identity(library, &source_path, declaration_identity),
            );
        }
    }

    /// Build the provider-aware type remapping shared by every import statement for one compiled library.
    ///
    /// Already-checked type imports can retain their local spelling in a later callable import. Provider-owned
    /// signature types without an active local binding receive an internal qualified spelling, and a later type import
    /// records the same provider identity for its local spelling. Both declaration orders therefore compare as the
    /// same nominal type without consulting a raw whole-program import scan.
    pub(super) fn public_library_type_import_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        let mut local_names_by_public_export: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let root = self.loaf_binding_root(library);
        for (local_name, path) in self.symbols.active_import_binding_paths() {
            let Some([public_name]) = path.strip_prefix(root.as_slice()) else {
                continue;
            };
            if !self.manifest_public_name_is_type(manifest, public_name) {
                continue;
            }
            local_names_by_public_export
                .entry(public_name.clone())
                .or_default()
                .push(local_name.to_string());
        }
        for local_names in local_names_by_public_export.values_mut() {
            local_names.sort();
            local_names.dedup();
        }

        let mut remapping = self.public_library_canonical_type_remapping(library, manifest);
        for (public_name, local_names) in local_names_by_public_export {
            if let Some(identity) = self.public_library_nominal_type_identity(library, manifest, &public_name) {
                for local_name in &local_names {
                    self.public_library_type_identities
                        .insert(local_name.clone(), identity.clone());
                }
            }
            if let Some(local_name) = local_names.into_iter().next() {
                remapping.insert(public_name, local_name);
            }
        }
        remapping
    }

    /// Build an import-scope-independent provider-qualified remapping for one compiled library.
    ///
    /// Qualified module access has no local `from` import whose alias can carry identity, so every nominal manifest
    /// spelling is mapped to its checker-owned canonical key before the member's signature enters expression checking.
    pub(super) fn public_library_canonical_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        let mut remapping = HashMap::new();
        let public_names = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .map(|entry| entry.public_name.clone())
            .collect::<Vec<_>>();
        for public_name in public_names {
            let Some(identity) = self.public_library_nominal_type_identity(library, manifest, &public_name) else {
                continue;
            };
            let canonical_name = canonical_public_library_type_name(library, &public_name);
            self.public_library_type_identities
                .insert(canonical_name.clone(), identity);
            remapping.insert(public_name, canonical_name);
        }
        remapping.extend(self.public_library_foreign_type_remapping(library, manifest));
        remapping
    }

    /// Hydrate foreign signature types from the Loaves that declare them.
    ///
    /// Leaf binding keys carry identity; this map only chooses the consumer spelling, `pub::<loaf key>::<public
    /// path>`. Required nominal layouts come from the declaring Loaf's public identity graph, including types never
    /// imported by source name.
    ///
    /// The work is three phases and their order is the correctness argument. Seeding finds the origins this
    /// manifest can reach directly; the closure walks from those to every Loaf transitively required; only then
    /// can layouts be reconstructed, because a layout may reference a type discovered late, and remapping it
    /// against a partial map would bake in a name that never resolves.
    pub(super) fn public_library_foreign_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        if let Some(remapping) = self.foreign_pub_type_remappings.get(library) {
            return remapping.clone();
        }
        let seeds = collect_nominal_type_origins(manifest);
        let (remapping, loaves) = self.resolve_public_type_origins(library, seeds);
        self.reconstruct_artifact_layouts(library, &remapping, &loaves);
        self.foreign_pub_type_remappings
            .insert(library.to_string(), remapping.clone());
        remapping
    }

    /// Walk from the seed origins to every Loaf transitively required, registering each type's spelling.
    ///
    /// Resolving one origin can reveal a Loaf whose own manifest names further origins, so this is a worklist rather
    /// than a pass. `seen` is keyed by binding key so a type reached by two routes is resolved once, which is what
    /// keeps mutual references from looping. An origin no indexed Loaf declares is reported and skipped rather than
    /// failing the whole map, so a single bad type does not cost every other import in the library.
    pub(super) fn resolve_public_type_origins(
        &mut self,
        library: &str,
        seeds: BTreeMap<String, NominalTypeOriginExport>,
    ) -> (HashMap<String, String>, BTreeMap<String, LoafEntry>) {
        let mut pending = std::collections::VecDeque::from_iter(seeds.into_values());
        let mut seen = HashSet::new();
        let mut loaves = BTreeMap::new();
        let mut remapping = HashMap::new();

        while let Some(origin) = pending.pop_front() {
            let key = origin.binding_key();
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(canonical) = origin.canonical.hydrate() else {
                continue;
            };
            let Some((entry, export)) = self
                .loaves
                .public_nominal(&canonical)
                .map(|(entry, export)| (entry.clone(), export.clone()))
            else {
                self.push_public_library_error(
                    library,
                    format!("names a public type no indexed Loaf declares: {canonical:?}"),
                );
                continue;
            };
            let qualified = canonical_public_library_type_name(&entry.key, &export.public_path[1..].join("::"));
            let identity = PublicLibraryTypeIdentity {
                dependency_key: library.to_string(),
                source_path: export.source_path.clone(),
                canonical: Some(canonical),
                selected_provider: Some(entry.artifact.identity.clone()),
            };
            self.public_library_type_identities
                .insert(key.clone(), identity.clone());
            self.public_library_type_identities.insert(qualified.clone(), identity);
            remapping.insert(key, qualified);

            if let std::collections::btree_map::Entry::Vacant(vacant) = loaves.entry(entry.key.clone()) {
                pending.extend(collect_nominal_type_origins(&entry.artifact.manifest).into_values());
                vacant.insert(entry);
            }
        }
        (remapping, loaves)
    }

    /// Rebuild each discovered Loaf's nominal layouts against the completed remapping.
    ///
    /// This runs only after every type mapping exists, which is what preserves mutually referenced layouts and
    /// foreign field, property and default signatures without reopening Loaves or looking up source. The same
    /// ordering holds inside each Loaf: every export's public name is registered first, so the second pass can
    /// remap a layout that refers to a sibling declared after it.
    pub(super) fn reconstruct_artifact_layouts(
        &mut self,
        library: &str,
        remapping: &HashMap<String, String>,
        loaves: &BTreeMap<String, LoafEntry>,
    ) {
        for entry in loaves.values() {
            let local_remapping = self.register_artifact_type_names(library, entry, remapping);
            self.rebuild_artifact_symbol_layouts(entry, &local_remapping);
        }
    }

    /// Register every nominal export of one artifact under its qualified consumer name.
    ///
    /// Returns the remapping extended with this artifact's own top-level spellings, so a layout rebuilt next can
    /// resolve a sibling type by the bare name its signature uses. A top-level spelling is a declaration of the
    /// package root or a root re-export of a submodule's declaration (`pub from search import Nomination`): both are
    /// the name a published signature uses for the type, and a re-export left out would leave a field typed by it
    /// with no members.
    pub(super) fn register_artifact_type_names(
        &mut self,
        library: &str,
        entry: &LoafEntry,
        remapping: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        let mut local_remapping = remapping.clone();
        let artifact = &entry.artifact;
        let owner_route = entry.key.as_str();
        let graph = &artifact.manifest.contract_metadata.identity_graph;
        for export in &graph.exports {
            let declared = matches!(
                export.kind,
                crate::library_manifest::ExportIdentityKind::Model
                    | crate::library_manifest::ExportIdentityKind::Class
                    | crate::library_manifest::ExportIdentityKind::Enum
                    | crate::library_manifest::ExportIdentityKind::Newtype
            );
            let top_level = export.public_path.len() == 2;
            let reexported = !declared
                && top_level
                && self
                    .manifest_nominal_type_info(&artifact.manifest, &export.public_name)
                    .is_some();
            if !declared && !reexported {
                continue;
            }
            let qualified = canonical_public_library_type_name(owner_route, &export_public_name(export));
            let canonical = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            // A re-export names the declaration its canonical identity points at, so its identity carries that
            // declaration's own source path rather than the re-export hop.
            let source_path = if reexported {
                canonical
                    .as_ref()
                    .and_then(|canonical| graph.declaration_source_path(canonical))
                    .map_or_else(|| export.source_path.clone(), <[String]>::to_vec)
            } else {
                export.source_path.clone()
            };
            let identity = PublicLibraryTypeIdentity {
                dependency_key: library.to_string(),
                source_path,
                canonical,
                selected_provider: Some(artifact.identity.clone()),
            };
            if reexported {
                self.public_library_type_identities
                    .entry(qualified.clone())
                    .or_insert(identity);
            } else {
                self.public_library_type_identities.insert(qualified.clone(), identity);
            }
            // Only a top-level export is reachable by a bare name in a signature; a nested one is always written
            // through its path, so adding it here would shadow the wrong spelling.
            if top_level {
                local_remapping.insert(export.public_name.clone(), qualified);
            }
        }
        local_remapping
    }

    /// Rebuild one artifact's exported symbol layouts with foreign type names already resolved.
    ///
    /// Member types are read in the declaring module's scope before the artifact's top-level spellings, so a layout
    /// names the declaration its own module names whether the root republishes it or not.
    pub(super) fn rebuild_artifact_symbol_layouts(
        &mut self,
        entry: &LoafEntry,
        local_remapping: &HashMap<String, String>,
    ) {
        let artifact = &entry.artifact;
        let owner_route = entry.key.as_str();
        for export in &artifact.manifest.contract_metadata.identity_graph.exports {
            // A top-level export is described by the manifest's own nominal info; a nested one has to be read back
            // from its declaration, because only the declaration records the members.
            let info = if export.public_path.len() == 2 {
                self.manifest_nominal_type_info(&artifact.manifest, &export.public_name)
            } else {
                Self::api_declaration_for_target_path(&artifact.manifest, &export.source_path)
                    .and_then(|declaration| {
                        self.symbol_kind_from_api_declaration(
                            artifact.manifest.contract_metadata.api.as_ref(),
                            declaration,
                        )
                    })
                    .and_then(|kind| match kind {
                        SymbolKind::Type(info) => Some(info),
                        _ => None,
                    })
            };
            let Some(info) = info else {
                continue;
            };
            let qualified = canonical_public_library_type_name(owner_route, &export_public_name(export));
            let mut kind = SymbolKind::Type(info);
            // A member type is spelled as the declaring module spells it; the names that module reaches through a
            // private import or a submodule the root does not republish resolve through its scope first.
            let declaration = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            self.remap_symbol_kind_through_declaring_module(
                owner_route,
                &artifact.manifest,
                declaration.as_ref(),
                &mut kind,
            );
            self.remap_symbol_kind_with_import_aliases(&mut kind, local_remapping);
            Self::mark_compiled_class_field_provider(&mut kind, owner_route);
            if let SymbolKind::Type(info) = kind {
                self.transitive_pub_types.entry(qualified).or_default().push(info);
            }
        }
    }

    /// Report a malformed compiled-library input without repeating the diagnostic's shape at each call.
    pub(super) fn push_public_library_error(&mut self, library: &str, detail: String) {
        self.errors.push(crate::diagnostics::CompileError::type_error(
            format!("compiled library `{library}` {detail}"),
            Span::default(),
        ));
    }

    /// Return the nominal origin carried by a type reference, when it has one.
    pub(super) fn nominal_type_origin(ty: &crate::library_manifest::TypeRef) -> Option<&NominalTypeOriginExport> {
        use crate::library_manifest::TypeRef;
        match ty {
            TypeRef::Named {
                origin: Some(origin), ..
            }
            | TypeRef::Applied {
                origin: Some(origin), ..
            } => Some(origin),
            _ => None,
        }
    }

    /// Return whether one public manifest spelling resolves to a type-like import binding.
    pub(super) fn manifest_public_name_is_type(&self, manifest: &LibraryManifest, public_name: &str) -> bool {
        let Some(export) = Self::find_manifest_export(manifest, public_name) else {
            return false;
        };
        match export {
            ManifestExportRef::Alias(alias) => self
                .symbol_kind_from_manifest_alias(manifest, alias, &mut HashSet::new())
                .is_some_and(|kind| matches!(kind, SymbolKind::Type(_) | SymbolKind::Trait(_))),
            other => Self::manifest_export_is_type(&other),
        }
    }

    /// Resolve one public spelling to nominal model/class/enum/newtype metadata.
    pub(super) fn manifest_nominal_type_info(&self, manifest: &LibraryManifest, public_name: &str) -> Option<TypeInfo> {
        let export = Self::find_manifest_export(manifest, public_name)?;
        let kind = match export {
            ManifestExportRef::Alias(alias) => {
                self.symbol_kind_from_manifest_alias(manifest, alias, &mut HashSet::new())?
            }
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::Newtype(export) => {
                SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
            }
            _ => return None,
        };
        match kind {
            SymbolKind::Type(
                info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_)),
            ) => Some(info),
            _ => None,
        }
    }

    /// Resolve one public nominal type spelling to its dependency-qualified provider source identity.
    pub(super) fn public_library_nominal_type_identity(
        &self,
        library: &str,
        manifest: &LibraryManifest,
        public_name: &str,
    ) -> Option<PublicLibraryTypeIdentity> {
        self.manifest_nominal_type_info(manifest, public_name)?;
        let entry = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)?;
        let source_path = entry.target_path().unwrap_or(entry.source_path.as_slice());
        Some(self.admitted_public_type_identity(
            library,
            source_path,
            entry.canonical.as_ref().and_then(|identity| identity.hydrate()),
        ))
    }

    /// Return the missing public features for one known provider export that is inactive in this artifact projection.
    pub(super) fn inactive_pub_export_features(manifest: &LibraryManifest, symbol: &str) -> Option<Vec<Vec<String>>> {
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
    pub(in crate::typechecker) fn pub_library_function_symbol(
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
            _ => {
                let identities = manifest
                    .contract_metadata
                    .identity_graph
                    .function_identities_for_public_name(member);
                Some(SymbolKind::FunctionOverloads(
                    functions
                        .into_iter()
                        .enumerate()
                        .map(|(index, function)| FunctionOverloadInfo {
                            info: self.function_info_from_manifest(function),
                            span: Span::default(),
                            identity: identities.get(index).cloned().flatten(),
                        })
                        .collect(),
                ))
            }
        }
    }

    /// Resolve one exported member from an imported `pub::` library as a symbol kind.
    ///
    /// This is used by qualified alias collection, where the import remains a module binding (`lib.member`) instead of
    /// a direct `from pub::lib import member` symbol. Alias exports are followed to their manifest target before the
    /// projected kind is returned.
    pub(in crate::typechecker) fn lookup_pub_library_symbol_member(
        &mut self,
        library: &str,
        member: &str,
    ) -> Option<SymbolKind> {
        let manifest = std::sync::Arc::clone(&self.loaves.entry(library)?.artifact.manifest);
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let remapping = self.public_library_canonical_type_remapping(library, &manifest);
        let mut kind = if let Some(kind) = self.pub_library_function_symbol(&manifest, member) {
            kind
        } else {
            let export = Self::find_manifest_export(&manifest, member)?;
            match export {
                ManifestExportRef::Model(export) => {
                    SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
                }
                ManifestExportRef::Class(export) => {
                    SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
                }
                ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
                ManifestExportRef::Partial(export) => {
                    SymbolKind::Function(self.partial_info_from_manifest(&manifest, export))
                }
                ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
                ManifestExportRef::Enum(export) => {
                    SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export)))
                }
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
                ManifestExportRef::EnumVariant {
                    enum_name,
                    fields,
                    canonical,
                } => SymbolKind::Variant(VariantInfo {
                    identity: canonical.and_then(|identity| identity.hydrate()),
                    enum_name: enum_name.to_string(),
                    fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
                }),
                ManifestExportRef::Alias(export) => {
                    self.symbol_kind_from_manifest_alias(&manifest, export, &mut HashSet::new())?
                }
            }
        };
        let canonical = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(member);
        self.remap_symbol_kind_through_declaring_module(library, &manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        Self::mark_compiled_class_field_provider(&mut kind, library);
        Some(kind)
    }

    /// Resolve a member through a checked public module namespace and return its authored source module path.
    pub(in crate::typechecker) fn lookup_pub_library_module_symbol_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        self.resolve_pub_library_module_symbol_member(library, module_path, member)
            .ok()
            .flatten()
            .map(|resolved| (resolved.kind, resolved.source_module_path))
    }

    /// Resolve a checked public module member while preserving sibling ambiguity for caller-owned diagnostics.
    pub(in crate::typechecker) fn resolve_pub_library_module_symbol_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Result<Option<ResolvedPublicModuleSymbol>, Vec<String>> {
        if module_path.is_empty() {
            let Some(kind) = self.lookup_pub_library_symbol_member(library, member) else {
                return Ok(None);
            };
            let canonical = self.loaves.manifest(library).and_then(|manifest| {
                manifest
                    .contract_metadata
                    .identity_graph
                    .canonical_for_public_name(member)
            });
            return Ok(Some(ResolvedPublicModuleSymbol {
                kind,
                canonical,
                source_module_path: self.loaf_binding_root(library),
                source_name: member.to_string(),
            }));
        }
        let Some(manifest) = self
            .loaves
            .entry(library)
            .map(|entry| std::sync::Arc::clone(&entry.artifact.manifest))
        else {
            return Ok(None);
        };
        let Some(resolved) = self.public_module_member(library, &manifest, module_path, member)? else {
            return Ok(None);
        };
        let mut source_module_path = self.loaf_binding_root(library);
        source_module_path.extend(resolved.source_module_path);
        Ok(Some(ResolvedPublicModuleSymbol {
            kind: resolved.kind,
            canonical: resolved.canonical,
            source_module_path,
            source_name: resolved.source_name,
        }))
    }

    /// Resolve partial-call projection metadata for a member reached through a checked public module namespace.
    pub(in crate::typechecker) fn lookup_pub_library_module_partial_projection(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
        binding_name: &str,
    ) -> Option<PartialProjectionInfo> {
        let manifest = std::sync::Arc::clone(&self.loaves.entry(library)?.artifact.manifest);
        let mut projection = self
            .public_module_member(library, &manifest, module_path, member)
            .ok()
            .flatten()?
            .partial_projection?;
        projection.name = binding_name.to_string();
        Some(projection)
    }

    /// Seed internal semantic caches for one `pub::` library's exported types and traits.
    ///
    /// These caches are used only by the consumer-side typechecker when imported signatures mention provider types
    /// that the consumer did not explicitly import by name (for example `Session.read_csv(...) -> LazyFrame[T]`).
    /// They do not change source-visible name resolution.
    pub(super) fn cache_transitive_pub_export_semantics(&mut self, library: &str, manifest: &LibraryManifest) {
        if !self.cached_pub_libraries.insert(library.to_string()) {
            return;
        }
        let canonical_remapping = self.public_library_canonical_type_remapping(library, manifest);

        for model in &manifest.exports.models {
            let model_info = self.model_info_from_manifest(model);
            self.transitive_pub_types
                .entry(model.name.clone())
                .or_default()
                .push(TypeInfo::Model(model_info));
        }
        for class in &manifest.exports.classes {
            let mut kind = SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(class)));
            Self::mark_compiled_class_field_provider(&mut kind, library);
            if let SymbolKind::Type(info) = kind {
                self.transitive_pub_types
                    .entry(class.name.clone())
                    .or_default()
                    .push(info);
            }
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

        self.register_pub_library_declarations(library, manifest, &canonical_remapping);
        if let Some(api) = manifest.contract_metadata.api.as_ref() {
            for module in &api.modules {
                for declaration in &module.declarations {
                    let public_path = std::iter::once(manifest.name.clone())
                        .chain(module.module_path.iter().cloned())
                        .chain(std::iter::once(Self::api_declaration_name(declaration).to_string()))
                        .collect::<Vec<_>>();
                    let canonical = manifest
                        .contract_metadata
                        .identity_graph
                        .canonical_for_public_path(&public_path);
                    self.record_provider_type_param_bounds(declaration, canonical.as_ref());
                }
            }
        }
        let identity_graph = &manifest.contract_metadata.identity_graph;
        let canonical_types = identity_graph
            .exports
            .iter()
            .filter_map(|identity| {
                let info = self.manifest_nominal_type_info(manifest, &identity.public_name)?;
                Some((
                    canonical_public_library_type_name(library, &identity.public_name),
                    identity_graph.canonical_for_public_name(&identity.public_name),
                    info,
                ))
            })
            .collect::<Vec<_>>();
        for (canonical_name, declaration, info) in canonical_types {
            let mut kind = SymbolKind::Type(info);
            self.remap_symbol_kind_through_declaring_module(library, manifest, declaration.as_ref(), &mut kind);
            self.remap_symbol_kind_with_import_aliases(&mut kind, &canonical_remapping);
            Self::mark_compiled_class_field_provider(&mut kind, library);
            if let SymbolKind::Type(info) = kind {
                self.transitive_pub_types.entry(canonical_name).or_default().push(info);
            }
        }
    }

    /// Return all exported names in a manifest for diagnostics.
    pub(super) fn manifest_export_names(manifest: &LibraryManifest) -> Vec<String> {
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
    pub(super) fn find_manifest_export<'a>(manifest: &'a LibraryManifest, name: &str) -> Option<ManifestExportRef<'a>> {
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
                    canonical: variant.canonical.as_ref(),
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
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
    pub(super) fn find_manifest_non_alias_export<'a>(
        manifest: &'a LibraryManifest,
        name: &str,
    ) -> Option<ManifestExportRef<'a>> {
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
                    canonical: variant.canonical.as_ref(),
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
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
    pub(super) fn symbol_kind_from_manifest_alias(
        &self,
        manifest: &LibraryManifest,
        export: &AliasExport,
        visited: &mut HashSet<Vec<String>>,
    ) -> Option<SymbolKind> {
        if let Some(function) = &export.projected_function {
            return Some(SymbolKind::Function(self.function_info_from_manifest(function)));
        }
        if let Some(kind) = self.symbol_kind_from_nominal_projection(export.projected_type.as_ref()) {
            return Some(kind);
        }
        let identity_target_path = Self::manifest_identity_target_path(manifest, &export.name);
        let target_path = identity_target_path.unwrap_or(export.target_path.as_slice());
        if let Some(kind) = Self::symbol_kind_from_manifest_rust_target(manifest, target_path) {
            return Some(kind);
        }
        if let Some(target_path) = identity_target_path
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

    /// Hydrate a forwarded nominal only from its exact admitted declaring artifact and public membership.
    pub(super) fn symbol_kind_from_nominal_projection(
        &self,
        projected: Option<&crate::library_manifest::TypeRef>,
    ) -> Option<SymbolKind> {
        if projected.is_some_and(|ty| ty.has_native_union()) {
            return Some(SymbolKind::Type(TypeInfo::TypeAlias));
        }
        let crate::library_manifest::TypeRef::Named {
            origin: Some(origin), ..
        } = projected?
        else {
            return None;
        };
        let canonical = origin.canonical.hydrate()?;
        let (entry, identity) = self.loaves.public_nominal(&canonical)?;
        let artifact = &entry.artifact;
        if identity.public_path.len() == 2 {
            self.manifest_nominal_type_info(&artifact.manifest, &identity.public_name)
                .map(SymbolKind::Type)
        } else {
            Self::api_declaration_for_target_path(&artifact.manifest, &identity.source_path).and_then(|declaration| {
                self.symbol_kind_from_api_declaration(artifact.manifest.contract_metadata.api.as_ref(), declaration)
            })
        }
    }

    /// Reconstruct a checked `rust::` reexport from the ABI metadata shipped beside the public manifest.
    pub(super) fn symbol_kind_from_manifest_rust_target(
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        let kind = Self::rust_reexport_symbol_kind(manifest, target_path)?;
        matches!(&kind, SymbolKind::RustItem(info) if info.metadata.is_some()).then_some(kind)
    }

    /// The Rust item a provider's public `rust::` reexport names, bound as the provider's own import is. Metadata the
    /// provider shipped travels with it; otherwise the consumer's checker inspects the item by its path on demand.
    pub(super) fn rust_reexport_symbol_kind(manifest: &LibraryManifest, target_path: &[String]) -> Option<SymbolKind> {
        let [namespace, crate_name, rest @ ..] = target_path else {
            return None;
        };
        if namespace != "rust" || rest.is_empty() {
            return None;
        }
        let path = target_path[1..].join("::");
        let metadata = manifest
            .rust_abi
            .as_ref()
            .and_then(|abi| abi.get(&path))
            .map(|metadata| Arc::new(metadata.clone()));
        Some(SymbolKind::RustItem(RustItemInfo {
            crate_name: crate_name.clone(),
            metadata,
            path,
            binding: RustImportBindingKind::FromImport,
        }))
    }

    /// Return the canonical target path published by the manifest identity graph for one public name.
    pub(super) fn manifest_identity_target_path<'a>(
        manifest: &'a LibraryManifest,
        public_name: &str,
    ) -> Option<&'a [String]> {
        manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)
            .and_then(|entry| entry.target_path())
    }

    /// Resolve an exact manifest identity path into a semantic symbol kind before falling back to final-segment lookup.
    pub(super) fn symbol_kind_from_manifest_path(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        if let Some(kind) = self.symbol_kind_from_api_target_path(manifest, target_path) {
            return Some(kind);
        }
        let target_name = target_path.last()?;
        self.find_manifest_non_alias_symbol_kind(manifest, target_name)
            .or_else(|| self.pub_library_function_symbol(manifest, target_name))
    }

    /// Convert one non-alias manifest export into checker symbol metadata.
    pub(super) fn find_manifest_non_alias_symbol_kind(
        &self,
        manifest: &LibraryManifest,
        name: &str,
    ) -> Option<SymbolKind> {
        match Self::find_manifest_non_alias_export(manifest, name)? {
            ManifestExportRef::Model(export) => {
                Some(SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export))))
            }
            ManifestExportRef::Class(export) => {
                Some(SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export))))
            }
            ManifestExportRef::Function(export) => Some(SymbolKind::Function(self.function_info_from_manifest(export))),
            ManifestExportRef::Partial(export) => {
                Some(SymbolKind::Function(self.partial_info_from_manifest(manifest, export)))
            }
            ManifestExportRef::Trait(export) => Some(SymbolKind::Trait(self.trait_info_from_manifest(export))),
            ManifestExportRef::Enum(export) => {
                Some(SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))))
            }
            ManifestExportRef::EnumVariant {
                enum_name,
                fields,
                canonical,
            } => Some(SymbolKind::Variant(VariantInfo {
                identity: canonical.and_then(|identity| identity.hydrate()),
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

    /// Follow a published alias's retained declaration identity before its written target. Standard-library facades
    /// may spell a same-package target as `std.io.IoError`, while the package API stores its declaration at
    /// `io.IoError`. Foreign projections without a declaration in this package retain their admitted target path.
    pub(super) fn api_alias_target_path<'a>(
        manifest: &'a LibraryManifest,
        module_path: &[String],
        name: &str,
        target_path: &'a [String],
    ) -> &'a [String] {
        let public_path = std::iter::once(manifest.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>();
        let identities = &manifest.contract_metadata.identity_graph;
        identities
            .canonical_for_public_path(&public_path)
            .and_then(|canonical| identities.declaration_source_path(&canonical))
            .unwrap_or(target_path)
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    pub(super) fn api_declaration_for_target_path<'a>(
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

    /// Normalize one API declaration target into its authored module path and declaration name.
    pub(super) fn normalized_api_declaration_target(target_path: &[String]) -> Option<(Vec<String>, String)> {
        let path = if target_path.first().is_some_and(|segment| segment == "crate") {
            &target_path[1..]
        } else {
            target_path
        };
        let (name, module_path) = path.split_last()?;
        Some((module_path.to_vec(), name.clone()))
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    pub(super) fn symbol_kind_from_api_target_path(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        self.symbol_kind_from_api_target_path_inner(manifest, target_path, &mut HashSet::new())
    }

    /// Follow checked API alias targets while refusing cycles.
    pub(super) fn symbol_kind_from_api_target_path_inner(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
        visited: &mut HashSet<Vec<String>>,
    ) -> Option<SymbolKind> {
        if !visited.insert(target_path.to_vec()) {
            return None;
        }
        // A re-exported Rust item binds as the Rust item its provider imported.
        if let Some(kind) = Self::rust_reexport_symbol_kind(manifest, target_path) {
            return Some(kind);
        }
        let declaration = Self::api_declaration_for_target_path(manifest, target_path)?;
        match declaration {
            ApiDeclaration::Alias(alias) => {
                if let Some(function) = &alias.projected_function {
                    return Some(SymbolKind::Function(
                        self.function_info_from_manifest(&function_export_from_api_projected(function)),
                    ));
                }
                self.symbol_kind_from_nominal_projection(alias.projected_type.as_ref())
                    .or_else(|| self.symbol_kind_from_api_target_path_inner(manifest, &alias.target_path, visited))
            }
            _ => self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration),
        }
    }

    /// Convert one checked API declaration into the same semantic symbols used for manifest exports.
    ///
    /// `api` is the checked API the declaration belongs to; a partial's leftover defaulted parameters are completed
    /// from its target there (#1760).
    pub(super) fn symbol_kind_from_api_declaration(
        &self,
        api: Option<&CheckedApiMetadataPackage>,
        declaration: &ApiDeclaration,
    ) -> Option<SymbolKind> {
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
            ApiDeclaration::Alias(item) => item
                .projected_function
                .as_ref()
                .map(|function| {
                    SymbolKind::Function(
                        self.function_info_from_manifest(&function_export_from_api_projected(function)),
                    )
                })
                .or_else(|| self.symbol_kind_from_nominal_projection(item.projected_type.as_ref())),
            ApiDeclaration::Partial(item) => {
                Some(SymbolKind::Function(self.partial_info_from_export(&api.map_or_else(
                    || partial_export_from_api(item),
                    |api| partial_export_with_target_defaults(api, item),
                ))))
            }
        }
    }

    /// Return whether a manifest export introduces a type-like name into the importing module.
    pub(super) fn manifest_export_is_type(export: &ManifestExportRef<'_>) -> bool {
        matches!(export, ManifestExportRef::Alias(alias) if alias.projected_type.is_some())
            || matches!(
                export,
                ManifestExportRef::Model(_)
                    | ManifestExportRef::Class(_)
                    | ManifestExportRef::Trait(_)
                    | ManifestExportRef::Enum(_)
                    | ManifestExportRef::TypeAlias(_)
                    | ManifestExportRef::Newtype(_)
            )
    }

    /// Define one symbol imported from a public library manifest.
    #[allow(clippy::too_many_arguments)] // Keeps provider, projection, alias, and source evidence explicit.
    pub(super) fn define_pub_import_symbol(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        source_name: &str,
        local_name: String,
        export: ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let partial_projection = self.partial_projection_from_manifest_export(
            library,
            manifest,
            &local_name,
            &export,
            imported_type_aliases,
            span,
        );
        let declared_type_params = match &export {
            ManifestExportRef::Model(model) => model.type_params.clone(),
            ManifestExportRef::Class(class) => class.type_params.clone(),
            _ => Vec::new(),
        };
        let mut type_alias_target = None;
        let mut kind = match export {
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
            ManifestExportRef::Partial(export) => {
                SymbolKind::Function(self.partial_info_from_manifest(manifest, export))
            }
            ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::EnumVariant {
                enum_name,
                fields,
                canonical,
            } => SymbolKind::Variant(VariantInfo {
                identity: canonical.and_then(|identity| identity.hydrate()),
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            }),
            ManifestExportRef::TypeAlias(export) => {
                let mut target = resolved_type_from_manifest_type_ref(&export.target);
                Self::remap_resolved_type_with_import_aliases(&mut target, imported_type_aliases);
                type_alias_target = Some(crate::typechecker::TypeAliasTarget {
                    type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
                    target,
                    nominal_identities: Default::default(),
                });
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
                if let Some(projected) = export.projected_type.as_ref().filter(|ty| ty.has_native_union()) {
                    let mut target = resolved_type_from_manifest_type_ref(projected);
                    Self::remap_resolved_type_with_import_aliases(&mut target, imported_type_aliases);
                    type_alias_target = Some(crate::typechecker::TypeAliasTarget {
                        type_params: Vec::new(),
                        target,
                        nominal_identities: Default::default(),
                    });
                }
                let Some(kind) = self.symbol_kind_from_manifest_alias(manifest, export, &mut HashSet::new()) else {
                    return;
                };
                kind
            }
        };
        let canonical = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(source_name);
        self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);
        Self::mark_compiled_class_field_provider(&mut kind, library);

        let mut binding_path = self.loaf_binding_root(library);
        binding_path.push(source_name.to_string());
        if let SymbolKind::RustItem(info) = kind {
            self.symbols.define_import_binding_with_inferred_target_at_path(
                Symbol {
                    name: local_name,
                    kind: SymbolKind::RustItem(info),
                    span,
                    scope: 0,
                },
                binding_path,
            );
            return;
        }
        let symbol_id = self.symbols.define_import_binding_at_path(
            Symbol {
                name: local_name.clone(),
                kind: kind.clone(),
                span,
                scope: 0,
            },
            canonical.clone(),
            binding_path,
        );
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        if let Some(identity) = canonical {
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name.clone(), identity);
        }
        if let Some(identity) = self.public_library_nominal_type_identity(library, manifest, source_name) {
            self.public_library_type_identities.insert(local_name.clone(), identity);
        }
        self.record_manifest_type_param_bounds(&local_name, &declared_type_params);
        if let Some(target) = type_alias_target {
            self.record_dependency_import_type_alias_before_change(&local_name);
            self.type_aliases.insert(local_name.clone(), target);
        }
        self.record_imported_function_binding(&local_name, &kind);
        if let Some(projection) = partial_projection {
            self.type_info.record_partial_projection(projection);
        }
        if matches!(kind, SymbolKind::Static(_)) {
            self.type_info
                .declarations
                .static_bindings
                .insert(local_name, crate::typechecker::StaticBindingInfo { is_imported: true });
        }
    }

    /// Attach the importing dependency key to every field reconstructed from one compiled class manifest.
    pub(super) fn mark_compiled_class_field_provider(kind: &mut SymbolKind, library: &str) {
        let SymbolKind::Type(TypeInfo::Class(info)) = kind else {
            return;
        };
        for name in &info.field_order {
            info.field_provider_libraries.insert(name.clone(), library.to_string());
        }
    }

    /// Rewrite imported semantic type references through type aliases from the source library manifest.
    pub(in crate::typechecker) fn remap_symbol_kind_with_import_aliases(
        &self,
        kind: &mut SymbolKind,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if imported_type_aliases.is_empty() {
            return;
        }

        match kind {
            SymbolKind::Capability(info) => {
                for (_, ty) in &mut info.scope {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
            }
            SymbolKind::Variable(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Static(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Function(info) => {
                Self::remap_function_info_with_import_aliases(info, imported_type_aliases);
            }
            SymbolKind::FunctionOverloads(overloads) => {
                for overload in overloads {
                    Self::remap_function_info_with_import_aliases(&mut overload.info, imported_type_aliases);
                }
            }
            SymbolKind::Type(ty_info) => match ty_info {
                TypeInfo::Class(info) => {
                    if let Some(extends) = &mut info.extends
                        && let Some(alias) = imported_type_aliases.get(extends)
                    {
                        *extends = alias.clone();
                    }
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for property in info.properties.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::Model(info) => {
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for property in info.properties.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::Newtype(info) => {
                    Self::remap_resolved_type_with_import_aliases(&mut info.underlying, imported_type_aliases);
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::Enum(info) => {
                    Self::remap_type_bounds_with_import_aliases(&mut info.trait_adoptions, imported_type_aliases);
                    for fields in info.variant_fields.values_mut() {
                        for field in fields {
                            Self::remap_resolved_type_with_import_aliases(field, imported_type_aliases);
                        }
                    }
                    for method in info.methods.values_mut() {
                        Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                    }
                    for overloads in info.method_overloads.values_mut() {
                        for method in overloads {
                            Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                        }
                    }
                }
                TypeInfo::TypeAlias | TypeInfo::Builtin => {}
            },
            SymbolKind::Trait(info) => {
                for (_, type_args) in &mut info.supertraits {
                    for type_arg in type_args {
                        Self::remap_resolved_type_with_import_aliases(type_arg, imported_type_aliases);
                    }
                }
                for method in info.methods.values_mut() {
                    Self::remap_method_info_with_import_aliases(method, imported_type_aliases);
                }
                for (_, ty) in &mut info.requires {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
                for property in info.properties.values_mut() {
                    Self::remap_resolved_type_with_import_aliases(&mut property.return_type, imported_type_aliases);
                }
            }
            SymbolKind::Variant(info) => {
                if let Some(alias) = imported_type_aliases.get(&info.enum_name) {
                    info.enum_name = alias.clone();
                }
                for field in &mut info.fields {
                    Self::remap_resolved_type_with_import_aliases(field, imported_type_aliases);
                }
            }
            SymbolKind::Field(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.ty, imported_type_aliases);
            }
            SymbolKind::Property(info) => {
                Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
            }
            SymbolKind::Module(_) | SymbolKind::RustItem(_) => {}
        }
    }

    /// Rewrite all resolved carrier types in one imported function signature and its generic bounds.
    pub(super) fn remap_function_info_with_import_aliases(
        info: &mut FunctionInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for param in &mut info.params {
            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
        }
        Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
        for bounds in info.type_param_bound_details.values_mut() {
            Self::remap_type_bounds_with_import_aliases(bounds, imported_type_aliases);
        }
    }

    /// Rewrite all resolved carrier types in one imported method signature and its generic or trait-target bounds.
    pub(super) fn remap_method_info_with_import_aliases(
        info: &mut MethodInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for param in &mut info.params {
            Self::remap_resolved_type_with_import_aliases(&mut param.ty, imported_type_aliases);
        }
        Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
        for bounds in info.type_param_bound_details.values_mut() {
            Self::remap_type_bounds_with_import_aliases(bounds, imported_type_aliases);
        }
        if let Some(target) = info.trait_target.as_mut() {
            Self::remap_type_bound_with_import_aliases(target, imported_type_aliases);
        }
    }

    /// Rewrite resolved generic arguments carried by imported trait-bound metadata.
    pub(super) fn remap_type_bounds_with_import_aliases(
        bounds: &mut [TypeBoundInfo],
        imported_type_aliases: &HashMap<String, String>,
    ) {
        for bound in bounds {
            Self::remap_type_bound_with_import_aliases(bound, imported_type_aliases);
        }
    }

    /// Retain an imported bound's trait binding and generic arguments in the same canonical namespace as its signature.
    pub(super) fn remap_type_bound_with_import_aliases(
        bound: &mut TypeBoundInfo,
        imported_type_aliases: &HashMap<String, String>,
    ) {
        if let Some(name) = imported_type_aliases.get(&bound.name) {
            bound.source_name.get_or_insert_with(|| bound.name.clone());
            bound.name = name.clone();
        }
        for type_arg in &mut bound.type_args {
            Self::remap_resolved_type_with_import_aliases(type_arg, imported_type_aliases);
        }
    }

    /// Rewrite resolved type names through import aliases after stdlib materialization.
    pub(super) fn remap_resolved_type_with_import_aliases(
        ty: &mut ResolvedType,
        imported_type_aliases: &HashMap<String, String>,
    ) {
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
            ResolvedType::Never
            | ResolvedType::Int
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
    pub(super) fn function_info_from_manifest(&self, export: &FunctionExport) -> FunctionInfo {
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
    ///
    /// The partial's leftover defaulted parameters are completed from its target in the manifest's checked API, so a
    /// call may omit one exactly when its default can be filled in (#1760).
    pub(super) fn partial_info_from_manifest(
        &self,
        manifest: &LibraryManifest,
        export: &PartialExport,
    ) -> FunctionInfo {
        self.partial_info_from_export(&manifest_partial_with_target_defaults(
            manifest.contract_metadata.api.as_ref(),
            export,
        ))
    }

    /// Convert one completed partial export into callable metadata for consumers.
    pub(super) fn partial_info_from_export(&self, export: &PartialExport) -> FunctionInfo {
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
    pub(super) fn partial_projection_from_manifest_export(
        &self,
        library: &str,
        manifest: &LibraryManifest,
        local_name: &str,
        export: &ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<PartialProjectionInfo> {
        match export {
            ManifestExportRef::Partial(export) => {
                Self::partial_projection_from_manifest_partial(export, local_name, imported_type_aliases, span, library)
            }
            ManifestExportRef::Alias(export) => {
                let context = PartialProjectionAliasContext {
                    library,
                    manifest,
                    local_name,
                    imported_type_aliases,
                    span,
                };
                self.partial_projection_from_manifest_alias(&context, export, &mut HashSet::new())
            }
            _ => None,
        }
    }

    /// Follow manifest aliases so a public alias to a partial keeps the same projection under the alias name.
    pub(super) fn partial_projection_from_manifest_alias(
        &self,
        context: &PartialProjectionAliasContext<'_>,
        alias: &AliasExport,
        visited: &mut HashSet<String>,
    ) -> Option<PartialProjectionInfo> {
        let identity_target = Self::manifest_identity_target_path(context.manifest, &alias.name);
        let target_path = identity_target.unwrap_or(alias.target_path.as_slice());
        let target_name = target_path.last()?;
        if !visited.insert(target_name.clone()) {
            return None;
        }
        if let Some(ApiDeclaration::Partial(item)) =
            Self::api_declaration_for_target_path(context.manifest, target_path)
        {
            let export = partial_export_from_api(item);
            return Self::partial_projection_from_manifest_partial(
                &export,
                context.local_name,
                context.imported_type_aliases,
                context.span,
                context.library,
            );
        }
        match Self::find_manifest_export(context.manifest, target_name)? {
            ManifestExportRef::Partial(export) => Self::partial_projection_from_manifest_partial(
                export,
                context.local_name,
                context.imported_type_aliases,
                context.span,
                context.library,
            ),
            ManifestExportRef::Alias(next_alias) => {
                self.partial_projection_from_manifest_alias(context, next_alias, visited)
            }
            _ => None,
        }
    }

    /// Reconstruct import-visible projection metadata from serialized partial preset metadata.
    pub(super) fn partial_projection_from_manifest_partial(
        export: &PartialExport,
        local_name: &str,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
        library: &str,
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
                        external_value: Self::checked_preset_value_from_manifest(&preset.value, imported_type_aliases),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            external_library: Some(library.to_string()),
        })
    }

    /// Rebuild one provider-owned preset value without losing canonical external reference identity.
    pub(super) fn checked_preset_value_from_manifest(
        value: &PresetValueExport,
        imported_type_aliases: &HashMap<String, String>,
    ) -> Option<CheckedPresetValue> {
        Some(match value {
            PresetValueExport::Int(value) => CheckedPresetValue::Int(*value),
            PresetValueExport::Float(value) => CheckedPresetValue::Float(value.parse().ok()?),
            PresetValueExport::Bool(value) => CheckedPresetValue::Bool(*value),
            PresetValueExport::String(value) => CheckedPresetValue::String(value.clone()),
            PresetValueExport::Bytes(value) => CheckedPresetValue::Bytes(value.clone()),
            PresetValueExport::None => CheckedPresetValue::None,
            PresetValueExport::List(values) => CheckedPresetValue::List(
                values
                    .iter()
                    .map(|item| Self::checked_preset_value_from_manifest(item, imported_type_aliases))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Set(values) => CheckedPresetValue::Set(
                values
                    .iter()
                    .map(|item| Self::checked_preset_value_from_manifest(item, imported_type_aliases))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Tuple(values) => CheckedPresetValue::Tuple(
                values
                    .iter()
                    .map(|item| Self::checked_preset_value_from_manifest(item, imported_type_aliases))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Dict(entries) => CheckedPresetValue::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        Some((
                            Self::checked_preset_value_from_manifest(&entry.key, imported_type_aliases)?,
                            Self::checked_preset_value_from_manifest(&entry.value, imported_type_aliases)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::ConstRef(path) => CheckedPresetValue::ConstRef(path.clone()),
            PresetValueExport::ModelLiteral { name, fields } => CheckedPresetValue::ModelLiteral {
                name: imported_type_aliases.get(name).cloned().unwrap_or_else(|| name.clone()),
                fields: fields
                    .iter()
                    .map(|field| {
                        Some((
                            field.name.clone(),
                            Self::checked_preset_value_from_manifest(&field.value, imported_type_aliases)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            },
            PresetValueExport::Unsupported => CheckedPresetValue::Unsupported,
        })
    }

    /// Convert manifest partial target kind metadata into frontend projection vocabulary.
    pub(super) fn partial_projection_target_kind_from_manifest(
        kind: PartialTargetKindExport,
    ) -> PartialProjectionTargetKind {
        match kind {
            PartialTargetKindExport::Function => PartialProjectionTargetKind::Function,
            PartialTargetKindExport::ModelConstructor => PartialProjectionTargetKind::ModelConstructor,
            PartialTargetKindExport::ClassConstructor => PartialProjectionTargetKind::ClassConstructor,
            PartialTargetKindExport::NewtypeConstructor => PartialProjectionTargetKind::NewtypeConstructor,
            PartialTargetKindExport::Partial | PartialTargetKindExport::Unknown => PartialProjectionTargetKind::Unknown,
        }
    }

    /// Rebuild a metadata-safe preset value as a synthetic AST expression.
    pub(super) fn manifest_preset_value_expr(
        value: &PresetValueExport,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let expr = match value {
            PresetValueExport::Int(value) => Expr::Literal(Literal::Int(IntLiteral::synthetic(*value))),
            PresetValueExport::Float(value) => Expr::Literal(Literal::Float(FloatLiteral {
                value: value.parse().ok()?,
                repr: value.clone(),
                suffix: None,
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
            PresetValueExport::Set(values) => Expr::Set(
                values
                    .iter()
                    .map(|item| Self::manifest_preset_value_expr(item, imported_type_aliases, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Tuple(values) => Expr::Tuple(
                values
                    .iter()
                    .map(|item| Self::manifest_preset_value_expr(item, imported_type_aliases, span))
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
                                Spanned::new(field.name.clone(), span),
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
    pub(super) fn manifest_const_ref_expr(path: &[String], span: Span) -> Option<Spanned<Expr>> {
        let (first, rest) = path.split_first()?;
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in rest {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        Some(expr)
    }

    /// Convert one manifest model export into semantic model metadata.
    pub(super) fn model_info_from_manifest(&self, export: &ModelExport) -> ModelInfo {
        let methods = self.methods_from_manifest(&export.methods);
        let method_overloads = self.method_overloads_from_manifest(&export.methods);
        ModelInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(&export.name, &export.fields, false),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: self.properties_from_manifest(&export.name, &export.properties),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Convert one manifest class export into semantic class metadata.
    pub(super) fn class_info_from_manifest(&self, export: &ClassExport) -> ClassInfo {
        let methods = self.methods_from_manifest(&export.methods);
        let method_overloads = self.method_overloads_from_manifest(&export.methods);
        ClassInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            extends: export.extends.clone(),
            traits: export.traits.clone(),
            trait_adoptions: Self::trait_adoptions_from_manifest(&export.traits, &export.trait_adoptions),
            derives: export.derives.clone(),
            fields: self.fields_from_manifest(
                &export.name,
                &export.fields,
                export.fields.iter().any(|field| {
                    matches!(
                        field.visibility,
                        crate::library_manifest::FieldVisibilityExport::Private
                    )
                }),
            ),
            field_defaults: Box::new(
                export
                    .fields
                    .iter()
                    .filter_map(|field| {
                        field.default.as_ref().and_then(|default| {
                            Self::manifest_param_default_expr(default, Span::default())
                                .map(|expr| (field.name.clone(), expr))
                        })
                    })
                    .collect(),
            ),
            field_default_metadata: Box::new(
                export
                    .fields
                    .iter()
                    .filter_map(|field| {
                        field.default.as_ref().map_or_else(
                            || {
                                field
                                    .has_default
                                    .then(|| (field.name.clone(), CheckedParamDefault::Unsupported))
                            },
                            |default| Some((field.name.clone(), self.checked_param_default_from_manifest(default))),
                        )
                    })
                    .collect(),
            ),
            field_provider_libraries: Box::new(HashMap::new()),
            field_order: export.fields.iter().map(|field| field.name.clone()).collect(),
            properties: self.properties_from_manifest(&export.name, &export.properties),
            method_overloads,
            methods,
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
        }
    }

    /// Preserve a compiled-library field default in the checked export vocabulary without reparsing or rebasing it.
    pub(super) fn checked_param_default_from_manifest(&self, default: &ParamDefaultExport) -> CheckedParamDefault {
        match default {
            ParamDefaultExport::Int(value) => CheckedParamDefault::Int(*value),
            ParamDefaultExport::Float(value) => value
                .parse::<f64>()
                .map(CheckedParamDefault::Float)
                .unwrap_or(CheckedParamDefault::Unsupported),
            ParamDefaultExport::Bool(value) => CheckedParamDefault::Bool(*value),
            ParamDefaultExport::String(value) => CheckedParamDefault::String(value.clone()),
            ParamDefaultExport::Bytes(value) => CheckedParamDefault::Bytes(value.clone()),
            ParamDefaultExport::None => CheckedParamDefault::None,
            ParamDefaultExport::List(values) => CheckedParamDefault::List(
                values
                    .iter()
                    .map(|value| self.checked_param_default_from_manifest(value))
                    .collect(),
            ),
            ParamDefaultExport::Dict(entries) => CheckedParamDefault::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        (
                            self.checked_param_default_from_manifest(&entry.key),
                            self.checked_param_default_from_manifest(&entry.value),
                        )
                    })
                    .collect(),
            ),
            ParamDefaultExport::ConstRef(path) => CheckedParamDefault::ConstRef(path.clone()),
            ParamDefaultExport::Call { path, args, signature } => CheckedParamDefault::Call {
                path: path.clone(),
                args: args
                    .iter()
                    .map(|arg| CheckedParamDefaultArg {
                        name: arg.name.clone(),
                        value: self.checked_param_default_from_manifest(&arg.value),
                    })
                    .collect(),
                signature: signature.as_ref().map(|signature| CheckedParamDefaultCallSignature {
                    params: self.params_from_manifest(&signature.params),
                    return_type: resolved_type_from_manifest_type_ref(&signature.return_type),
                }),
            },
            ParamDefaultExport::Unsupported => CheckedParamDefault::Unsupported,
        }
    }

    /// Rebuild a manifest-safe field default as synthetic source metadata for inherited constructor exports.
    pub(super) fn manifest_param_default_expr(default: &ParamDefaultExport, span: Span) -> Option<Spanned<Expr>> {
        let expr = match default {
            ParamDefaultExport::Int(value) => Expr::Literal(Literal::Int(IntLiteral::synthetic(*value))),
            ParamDefaultExport::Float(value) => Expr::Literal(Literal::Float(FloatLiteral {
                value: value.parse().ok()?,
                repr: value.clone(),
                suffix: None,
            })),
            ParamDefaultExport::Bool(value) => Expr::Literal(Literal::Bool(*value)),
            ParamDefaultExport::String(value) => Expr::Literal(Literal::String(value.clone())),
            ParamDefaultExport::Bytes(value) => Expr::Literal(Literal::Bytes(value.clone())),
            ParamDefaultExport::None => Expr::Literal(Literal::None),
            ParamDefaultExport::List(values) => Expr::List(
                values
                    .iter()
                    .map(|value| Self::manifest_param_default_expr(value, span).map(ListEntry::Element))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::Dict(entries) => Expr::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        Some(DictEntry::Pair(
                            Self::manifest_param_default_expr(&entry.key, span)?,
                            Self::manifest_param_default_expr(&entry.value, span)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::ConstRef(path) => Self::manifest_const_ref_expr(path, span)?.node,
            ParamDefaultExport::Call { path, args, .. } => Expr::Call(
                Box::new(Self::manifest_const_ref_expr(path, span)?),
                Vec::new(),
                args.iter()
                    .map(|arg| {
                        let value = Self::manifest_param_default_expr(&arg.value, span)?;
                        Some(match &arg.name {
                            Some(name) => CallArg::Named(Spanned::new(name.clone(), span), value),
                            None => CallArg::Positional(value),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            ParamDefaultExport::Unsupported => return None,
        };
        Some(Spanned::new(expr, span))
    }

    /// Convert one manifest trait export into semantic trait metadata.
    pub(super) fn trait_info_from_manifest(&self, export: &TraitExport) -> TraitInfo {
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
    pub(super) fn trait_adoptions_from_manifest(
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
                    implementation_type_params: Vec::new(),
                    inferred: false,
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
                implementation_type_params: Self::implementation_type_params_from_manifest(
                    &bound.implementation_type_params,
                ),
                inferred: bound.inferred,
            })
            .collect()
    }

    /// Decode one manifest implementation header into frontend semantic types.
    pub(super) fn implementation_type_params_from_manifest(
        type_params: &[ImplementationTypeParamExport],
    ) -> Vec<ImplementationTypeParamInfo> {
        type_params
            .iter()
            .map(|type_param| ImplementationTypeParamInfo {
                name: type_param.name.clone(),
                bounds: type_param
                    .bounds
                    .iter()
                    .map(|bound| ImplementationTraitBoundInfo {
                        trait_path: bound.trait_path.clone(),
                        type_args: bound
                            .type_args
                            .iter()
                            .map(resolved_type_from_manifest_type_ref)
                            .collect(),
                        associated_types: bound
                            .associated_types
                            .iter()
                            .map(|associated| {
                                (
                                    associated.name.clone(),
                                    resolved_type_from_manifest_type_ref(&associated.ty),
                                )
                            })
                            .collect(),
                        origin: match bound.origin {
                            ImplementationTraitBoundOriginExport::Standard => {
                                ImplementationTraitBoundOriginInfo::Standard
                            }
                            ImplementationTraitBoundOriginExport::RustCapability => {
                                ImplementationTraitBoundOriginInfo::RustCapability
                            }
                            ImplementationTraitBoundOriginExport::SourceCallable => {
                                ImplementationTraitBoundOriginInfo::SourceCallable
                            }
                        },
                    })
                    .collect(),
            })
            .collect()
    }

    /// Convert a manifest enum export into local enum symbol metadata.
    pub(super) fn enum_info_from_manifest(&self, export: &EnumExport) -> EnumInfo {
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
            variant_identities: export
                .variants
                .iter()
                .filter_map(|variant| Some((variant.name.clone(), variant.canonical.as_ref()?.hydrate()?)))
                .chain(export.variant_aliases.iter().filter_map(|alias| {
                    let identity = export
                        .variants
                        .iter()
                        .find(|variant| variant.name == alias.target)?
                        .canonical
                        .as_ref()?
                        .hydrate()?;
                    Some((alias.name.clone(), identity))
                }))
                .collect(),
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
    pub(super) fn newtype_info_from_manifest(&self, export: &NewtypeExport) -> NewtypeInfo {
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
            derives: export.derives.clone(),
            method_aliases: Self::method_aliases_from_manifest(&export.methods),
            methods: self.methods_from_manifest(&export.methods),
            method_overloads: self.method_overloads_from_manifest(&export.methods),
        }
    }

    pub(super) fn type_param_bounds_from_manifest(
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
    pub(super) fn type_param_bound_details_from_manifest(
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
                            implementation_type_params: Self::implementation_type_params_from_manifest(
                                &bound.implementation_type_params,
                            ),
                            inferred: bound.inferred,
                        })
                        .collect(),
                )
            })
            .collect()
    }

    /// Convert exported manifest fields into semantic field metadata for imported-library typechecking.
    pub(super) fn fields_from_manifest(
        &self,
        owner: &str,
        fields: &[FieldExport],
        uses_provider_constructor_bridge: bool,
    ) -> std::collections::HashMap<String, FieldInfo> {
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    FieldInfo {
                        identity: field.canonical.as_ref().and_then(|identity| identity.hydrate()),
                        ty: resolved_type_from_manifest_type_ref(&field.ty),
                        surface_type_name: field.surface_type_name.clone(),
                        visibility: match field.visibility {
                            crate::library_manifest::FieldVisibilityExport::Private => crate::ast::Visibility::Private,
                            crate::library_manifest::FieldVisibilityExport::Public => crate::ast::Visibility::Public,
                        },
                        is_type_private: matches!(
                            field.visibility,
                            crate::library_manifest::FieldVisibilityExport::Private
                        ),
                        owner: Some(owner.to_string()),
                        has_default: if uses_provider_constructor_bridge {
                            field.has_default
                        } else {
                            field
                                .default
                                .as_ref()
                                .is_some_and(ParamDefaultExport::is_materializable)
                        },
                        alias: field.alias.clone(),
                        description: field.description.clone(),
                    },
                )
            })
            .collect()
    }

    /// Convert manifest computed properties into public semantic member metadata.
    pub(super) fn properties_from_manifest(
        &self,
        owner: &str,
        properties: &[PropertyExport],
    ) -> std::collections::HashMap<String, PropertyInfo> {
        properties
            .iter()
            .map(|property| {
                (
                    property.name.clone(),
                    PropertyInfo {
                        identity: property.canonical.as_ref().and_then(|identity| identity.hydrate()),
                        return_type: resolved_type_from_manifest_type_ref(&property.return_type),
                        visibility: crate::ast::Visibility::Public,
                        owner: Some(owner.to_string()),
                        has_body: true,
                    },
                )
            })
            .collect()
    }

    /// Convert manifest methods into the legacy single-method-per-name lookup map.
    pub(super) fn methods_from_manifest(
        &self,
        methods: &[MethodExport],
    ) -> std::collections::HashMap<String, MethodInfo> {
        methods
            .iter()
            .map(|method| (method.name.clone(), self.method_info_from_manifest(method)))
            .collect()
    }

    /// Preserve same-type aliases so lowering can emit the canonical Rust method from a compiled dependency.
    pub(super) fn method_aliases_from_manifest(methods: &[MethodExport]) -> HashMap<String, String> {
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
    pub(super) fn method_overloads_from_manifest(
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
    pub(super) fn method_info_from_manifest(&self, method: &MethodExport) -> MethodInfo {
        MethodInfo {
            identity: method.canonical.as_ref().and_then(|identity| identity.hydrate()),
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
                                implementation_type_params: Self::implementation_type_params_from_manifest(
                                    &bound.implementation_type_params,
                                ),
                                inferred: bound.inferred,
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
    pub(super) fn params_from_manifest(&self, params: &[ParamExport]) -> Vec<CallableParam> {
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
                .with_mut(param.is_mut)
            })
            .collect()
    }

    pub(super) fn receiver_from_manifest(&self, receiver: Option<&ReceiverExport>) -> Option<Receiver> {
        match receiver {
            Some(ReceiverExport::Immutable) => Some(Receiver::Immutable),
            Some(ReceiverExport::Mutable) => Some(Receiver::Mutable),
            None => None,
        }
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

/// Collect every nominal type origin a manifest references, keyed by binding key.
///
/// Used both to seed the closure and to extend it when a newly opened artifact contributes more origins, so the
/// two callers cannot drift on what counts as an origin.
fn collect_nominal_type_origins(
    manifest: &LibraryManifest,
) -> BTreeMap<String, crate::library_manifest::NominalTypeOriginExport> {
    use crate::library_manifest::VisitTypeRefs;
    let mut origins = BTreeMap::new();
    let mut projection = manifest.clone();
    projection.visit_type_refs(&mut |ty| {
        if let Some(origin) = TypeChecker::nominal_type_origin(ty) {
            origins.insert(origin.binding_key(), origin.clone());
        }
    });
    origins
}

/// Render an export's public name without its leading package segment.
fn export_public_name(export: &crate::library_manifest::ExportIdentity) -> String {
    export
        .public_path
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join("::")
}
