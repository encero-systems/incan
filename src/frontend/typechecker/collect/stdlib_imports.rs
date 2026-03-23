//! Stdlib-aware import collection and namespace validation.
//!
//! This keeps stdlib import enforcement (RFC 022) separate from general declaration collection while preserving the
//! existing behavior.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::library_manifest_index::{LibraryManifestFailureKind, LibraryManifestIndexEntry};
use crate::frontend::module::ExportedSymbol;
use crate::frontend::symbols::*;
use crate::frontend::testing_markers::load_testing_marker_semantics;
use crate::frontend::typechecker::TypeChecker;
use crate::library_manifest::{
    ClassExport, ConstExport, EnumExport, FieldExport, FunctionExport, LibraryManifest, MethodExport, ModelExport,
    NewtypeExport, ParamExport, ReceiverExport, TraitExport, TypeParamExport, resolved_type_from_manifest_type_ref,
};
use incan_core::lang::stdlib;
use incan_core::lang::surface::types as surface_types;
use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

enum ManifestExportRef<'a> {
    Model(&'a ModelExport),
    Class(&'a ClassExport),
    Function(&'a FunctionExport),
    Trait(&'a TraitExport),
    Enum(&'a EnumExport),
    EnumVariant {
        enum_name: &'a str,
        fields: &'a [crate::library_manifest::TypeRef],
    },
    TypeAlias,
    Newtype(&'a NewtypeExport),
    Const(&'a ConstExport),
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
                // Reject `import std.f64.consts` — unknown stdlib module; suggest `import rust::std::f64::consts`.
                if stdlib::is_any_stdlib_path(&path.segments)
                    && !stdlib::is_known_stdlib_module(&path.segments)
                {
                    self.errors
                        .push(errors::unknown_stdlib_module(&path.segments.join("."), span));
                }
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.segments.last().cloned().unwrap_or_else(|| "module".to_string()));
                // Allow `import std.web as std` (alias matches source root), but reject `import std.web as rust` (alias is a different reserved root).
                let same_root = path.segments.first().map(|s| s.as_str()) == Some(&name);
                if !same_root {
                    self.validate_root_namespace(&name, span);
                }
                self.define_import_symbol(name, path.segments.clone(), false, span);
            }
            ImportKind::From { module, items } => {
                // Reject unknown stdlib module, e.g. `from std.f64.consts import PI`;
                // suggest a correction, e.g.`from rust::std::f64::consts import PI`.
                if module.parent_levels == 0
                    && !module.is_absolute
                    && stdlib::is_any_stdlib_path(&module.segments)
                    && !stdlib::is_known_stdlib_module(&module.segments)
                {
                    self.errors
                        .push(errors::unknown_stdlib_module(&module.segments.join("."), span));
                }

                let is_std_web = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() >= 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == stdlib::STDLIB_WEB;
                let is_std_async = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() >= 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "async";
                let is_std_reflection = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() == 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "reflection";
                let is_std_testing = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() == 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "testing";
                let is_known_stdlib_with_stub = module.parent_levels == 0
                    && !module.is_absolute
                    && stdlib::is_known_stdlib_module(&module.segments)
                    && stdlib::stdlib_stub_path(&module.segments).is_some();
                let module_path_str = module.segments.join(".");
                let testing_semantics = if is_std_testing {
                    match load_testing_marker_semantics() {
                        Ok(semantics) => Some(semantics),
                        Err(err) => {
                            self.errors
                                .push(errors::invalid_std_testing_marker_metadata(&err.to_string(), span));
                            None
                        }
                    }
                } else {
                    None
                };

                // For each item in `from module import item1, item2, ...`
                // create a symbol as if it were `import module::item`
                for item in items {
                    // Stdlib-scoped surface types: define them as builtin types only when imported from their owning
                    // module.
                    if let Some(id) = surface_types::from_str(item.name.as_str())
                        && let Some(expected_module_path) = surface_types::stdlib_module_path(id) {
                            let allow = match expected_module_path {
                                "std.web" => is_std_web,
                                "std.reflection" => is_std_reflection,
                                _ if expected_module_path.starts_with("std.async.") => {
                                    let async_root_or_prelude =
                                        module_path_str == "std.async" || module_path_str == "std.async.prelude";
                                    is_std_async
                                        && (async_root_or_prelude || module_path_str == expected_module_path)
                                }
                                _ => false,
                            };
                            if allow {
                                let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                                self.validate_root_namespace(&local_name, span);
                                self.symbols.define(Symbol {
                                    name: local_name,
                                    kind: SymbolKind::Type(TypeInfo::Builtin),
                                    span,
                                    scope: 0,
                                });
                                continue;
                            }
                        }

                    // RFC 023: for known stdlib modules with `.incn` stubs, prefer AST-derived signatures.
                    if is_known_stdlib_with_stub {
                        // Try function lookup first.
                        let ast_info = self.stdlib_cache.lookup_function(&module.segments, &item.name);
                        if let Some(info) = ast_info {
                            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            let mut resolved_marker_path = module.segments.clone();
                            resolved_marker_path.push(item.name.clone());
                            let module_feature = self.surface_context.decorator_feature_for_path(&resolved_marker_path);
                            let marker_feature =
                                testing_semantics
                                    .as_ref()
                                    .and_then(|semantics| semantics.marker_kind(&item.name))
                                    .map(|_| SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker));
                            if is_std_testing
                                && module_feature
                                    == Some(SurfaceFeatureKey::Decorator(
                                        DecoratorFeature::StdlibDecoratorFunction,
                                    ))
                                && marker_feature
                                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker))
                            {
                                self.testing_marker_import_bindings.insert(local_name.clone());
                            }
                            self.validate_root_namespace(&local_name, span);
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Function(info),
                                span,
                                scope: 0,
                            });
                            continue;
                        }

                        // Phase 6: try trait lookup (e.g., `from std.derives.comparison import Eq`).
                        let trait_info = self.stdlib_cache.lookup_trait(&module.segments, &item.name);
                        if let Some(info) = trait_info {
                            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            self.validate_root_namespace(&local_name, span);
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Trait(info),
                                span,
                                scope: 0,
                            });
                            continue;
                        }

                        // Top-level stdlib const bindings (e.g. `from std.math import PI`).
                        let const_info = self.stdlib_cache.lookup_constant(&module.segments, &item.name);
                        if let Some(info) = const_info {
                            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            self.validate_root_namespace(&local_name, span);
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Variable(info),
                                span,
                                scope: 0,
                            });
                            continue;
                        }
                    }

                    let aliased_type = item.alias.as_ref().and_then(|alias| {
                        if self.symbols.lookup(alias).is_some() {
                            return None;
                        }
                        let id = self.symbols.lookup(&item.name)?;
                        let sym = self.symbols.get(id)?;
                        let SymbolKind::Type(info) = &sym.kind else {
                            return None;
                        };
                        Some((alias.clone(), info.clone()))
                    });

                    if let Some((alias, info)) = aliased_type {
                        self.symbols.define(Symbol {
                            name: alias,
                            kind: SymbolKind::Type(info),
                            span,
                            scope: 0,
                        });
                        continue;
                    }
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    self.validate_root_namespace(&name, span);
                    let mut path = module.segments.clone();
                    path.push(item.name.clone());
                    self.define_import_symbol(name, path, false, span);
                }
            }
            ImportKind::PubLibrary { library } => {
                let name = import.alias.clone().unwrap_or_else(|| library.clone());
                self.validate_root_namespace(&name, span);
                self.validate_pub_library_entry(library, span);
                self.define_import_symbol(name, vec!["pub".to_string(), library.clone()], false, span);
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
                if self.reject_unsupported_rust_core_alloc(crate_name, span) {
                    return;
                }

                // Rust crate import: `import rust::serde_json`` or `import rust::serde_json::Value`
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.clone()));
                let full_path = self.rust_import_full_path(crate_name, path, None);
                self.define_rust_import_binding(name, crate_name, full_path, span);
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..  // version, features: not used here
            } => {
                if self.reject_unsupported_rust_core_alloc(crate_name, span) {
                    return;
                }

                // from rust::time import Instant, Duration
                for item in items {
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    let full_path = self.rust_import_full_path(crate_name, path, Some(&item.name));
                    self.define_rust_import_binding(name, crate_name, full_path, span);
                }
            }
        }
    }

    fn validate_pub_library_entry(&mut self, library: &str, span: Span) {
        let known_libraries = self.library_manifests.known_libraries();
        let Some(entry) = self.library_manifests.get(library).cloned() else {
            self.errors
                .push(errors::unknown_pub_library(library, &known_libraries, span));
            return;
        };
        if let LibraryManifestIndexEntry::Failed(failure) = entry {
            self.push_pub_library_failure(library, &failure, span);
        }
    }

    fn collect_pub_imports(&mut self, library: &str, items: &[ImportItem], span: Span) {
        let known_libraries = self.library_manifests.known_libraries();
        let Some(entry) = self.library_manifests.get(library).cloned() else {
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
            let Some(export) = Self::find_manifest_export(&manifest, &item.name) else {
                self.errors.push(errors::pub_library_symbol_not_exported(
                    &item.name,
                    library,
                    &available_exports,
                    span,
                ));
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

            self.define_pub_import_symbol(local_name, export, &imported_type_aliases, span);
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

    fn manifest_export_names(manifest: &LibraryManifest) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(manifest.exports.models.iter().map(|item| item.name.clone()));
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
        names.sort();
        names.dedup();
        names
    }

    fn find_manifest_export<'a>(manifest: &'a LibraryManifest, name: &str) -> Option<ManifestExportRef<'a>> {
        if let Some(item) = manifest.exports.models.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Model(item));
        }
        if let Some(item) = manifest.exports.classes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Class(item));
        }
        if let Some(item) = manifest.exports.functions.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Function(item));
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
        }
        if manifest.exports.type_aliases.iter().any(|item| item.name == name) {
            return Some(ManifestExportRef::TypeAlias);
        }
        if let Some(item) = manifest.exports.newtypes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Newtype(item));
        }
        if let Some(item) = manifest.exports.consts.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Const(item));
        }
        None
    }

    fn manifest_export_is_type(export: &ManifestExportRef<'_>) -> bool {
        matches!(
            export,
            ManifestExportRef::Model(_)
                | ManifestExportRef::Class(_)
                | ManifestExportRef::Trait(_)
                | ManifestExportRef::Enum(_)
                | ManifestExportRef::TypeAlias
                | ManifestExportRef::Newtype(_)
        )
    }

    fn existing_local_symbol_kind(&self, name: &str) -> Option<&'static str> {
        let symbol_id = self.symbols.lookup_local(name)?;
        let symbol = self.symbols.get(symbol_id)?;
        let kind = match &symbol.kind {
            SymbolKind::Variable(_) => "const/variable",
            SymbolKind::Function(_) => "function",
            SymbolKind::Type(_) => "type",
            SymbolKind::Trait(_) => "trait",
            SymbolKind::Module(_) => "imported module",
            SymbolKind::Variant(_) => "enum variant",
            SymbolKind::Field(_) => "field",
            SymbolKind::RustModule { .. } => "rust import",
        };
        Some(kind)
    }

    fn define_pub_import_symbol(
        &mut self,
        local_name: String,
        export: ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let mut kind = match export {
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
            ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::EnumVariant { enum_name, fields } => SymbolKind::Variant(VariantInfo {
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            }),
            ManifestExportRef::TypeAlias => SymbolKind::Type(TypeInfo::TypeAlias),
            ManifestExportRef::Newtype(export) => {
                SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
            }
            ManifestExportRef::Const(export) => SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            }),
        };
        self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);

        self.symbols.define(Symbol {
            name: local_name,
            kind,
            span,
            scope: 0,
        });
    }

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
            SymbolKind::Function(info) => {
                for (_, ty) in &mut info.params {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
                Self::remap_resolved_type_with_import_aliases(&mut info.return_type, imported_type_aliases);
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
                    for method in info.methods.values_mut() {
                        for (_, ty) in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Model(info) => {
                    for field in info.fields.values_mut() {
                        Self::remap_resolved_type_with_import_aliases(&mut field.ty, imported_type_aliases);
                    }
                    for method in info.methods.values_mut() {
                        for (_, ty) in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Newtype(info) => {
                    Self::remap_resolved_type_with_import_aliases(&mut info.underlying, imported_type_aliases);
                    for method in info.methods.values_mut() {
                        for (_, ty) in &mut method.params {
                            Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                        }
                        Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                    }
                }
                TypeInfo::Enum(_) | TypeInfo::TypeAlias | TypeInfo::Builtin => {}
            },
            SymbolKind::Trait(info) => {
                for method in info.methods.values_mut() {
                    for (_, ty) in &mut method.params {
                        Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                    }
                    Self::remap_resolved_type_with_import_aliases(&mut method.return_type, imported_type_aliases);
                }
                for (_, ty) in &mut info.requires {
                    Self::remap_resolved_type_with_import_aliases(ty, imported_type_aliases);
                }
            }
            SymbolKind::Module(_) | SymbolKind::Variant(_) | SymbolKind::Field(_) | SymbolKind::RustModule { .. } => {}
        }
    }

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
                    Self::remap_resolved_type_with_import_aliases(param, imported_type_aliases);
                }
                Self::remap_resolved_type_with_import_aliases(return_type, imported_type_aliases);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    Self::remap_resolved_type_with_import_aliases(item, imported_type_aliases);
                }
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) | ResolvedType::Ref(inner) => {
                Self::remap_resolved_type_with_import_aliases(inner, imported_type_aliases);
            }
            ResolvedType::FrozenDict(key, value) => {
                Self::remap_resolved_type_with_import_aliases(key, imported_type_aliases);
                Self::remap_resolved_type_with_import_aliases(value, imported_type_aliases);
            }
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::Unknown => {}
        }
    }

    fn function_info_from_manifest(&self, export: &FunctionExport) -> FunctionInfo {
        FunctionInfo {
            params: self.params_from_manifest(&export.params),
            return_type: resolved_type_from_manifest_type_ref(&export.return_type),
            is_async: export.is_async,
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            type_param_bounds: self.type_param_bounds_from_manifest(&export.type_params),
        }
    }

    fn model_info_from_manifest(&self, export: &ModelExport) -> ModelInfo {
        ModelInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            traits: export.traits.clone(),
            derives: Vec::new(),
            fields: self.fields_from_manifest(&export.fields),
            methods: self.methods_from_manifest(&export.methods),
        }
    }

    fn class_info_from_manifest(&self, export: &ClassExport) -> ClassInfo {
        ClassInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            extends: export.extends.clone(),
            traits: export.traits.clone(),
            derives: Vec::new(),
            fields: self.fields_from_manifest(&export.fields),
            methods: self.methods_from_manifest(&export.methods),
        }
    }

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

    fn enum_info_from_manifest(&self, export: &EnumExport) -> EnumInfo {
        EnumInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            variants: export.variants.iter().map(|variant| variant.name.clone()).collect(),
        }
    }

    fn newtype_info_from_manifest(&self, export: &NewtypeExport) -> NewtypeInfo {
        NewtypeInfo {
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            underlying: resolved_type_from_manifest_type_ref(&export.underlying),
            methods: self.methods_from_manifest(&export.methods),
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

    fn fields_from_manifest(&self, fields: &[FieldExport]) -> std::collections::HashMap<String, FieldInfo> {
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    FieldInfo {
                        ty: resolved_type_from_manifest_type_ref(&field.ty),
                        has_default: field.has_default,
                        alias: field.alias.clone(),
                        description: field.description.clone(),
                    },
                )
            })
            .collect()
    }

    fn methods_from_manifest(&self, methods: &[MethodExport]) -> std::collections::HashMap<String, MethodInfo> {
        methods
            .iter()
            .map(|method| {
                (
                    method.name.clone(),
                    MethodInfo {
                        receiver: self.receiver_from_manifest(method.receiver.as_ref()),
                        params: self.params_from_manifest(&method.params),
                        return_type: resolved_type_from_manifest_type_ref(&method.return_type),
                        is_async: method.is_async,
                        has_body: method.has_body,
                    },
                )
            })
            .collect()
    }

    fn params_from_manifest(&self, params: &[ParamExport]) -> Vec<(String, ResolvedType)> {
        params
            .iter()
            .map(|param| (param.name.clone(), resolved_type_from_manifest_type_ref(&param.ty)))
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
        let module_name = module.segments.join("_");
        let Some(exports) = self.dependency_exports.get(&module_name) else {
            return;
        };

        let mut exported_names: HashSet<String> = HashSet::new();
        for sym in exports {
            match sym {
                ExportedSymbol::Const(name)
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
            if !exported_names.contains(&item.name) {
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

    /// Validate and register a Rust import placeholder symbol for codegen.
    fn define_rust_import_binding(&mut self, name: Ident, crate_name: &str, full_path: Vec<Ident>, span: Span) {
        self.validate_root_namespace(&name, span);
        self.define_rust_import_symbol(name, crate_name.to_string(), full_path, span);
    }

    /// Define a symbol for a Rust crate import, skipping if a real definition exists.
    fn define_rust_import_symbol(&mut self, name: Ident, crate_name: String, path: Vec<Ident>, span: Span) {
        if self.has_real_definition(&name) {
            return;
        }
        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::RustModule {
                crate_name,
                path: path.join("::"),
            },
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

    /// Returns `true` if `name` already resolves to a "real" definition (type, function, trait, or variant) that
    /// should not be overwritten by a module/rust-module placeholder.
    fn has_real_definition(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|sym| {
            matches!(
                sym.kind,
                SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_)
            )
        })
    }
}
