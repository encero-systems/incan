//! How a consumer reads the type names in a compiled library's published declarations.
//!
//! A published field, parameter or return type names a type the way its declaring module spells it: through the
//! module's own declarations and its imports, private ones included (`from crate.search.models import Nomination` in
//! `search/exact.incn`). The consumer resolves most of those spellings through the package root's public names, which
//! is right exactly when the root publishes the same declaration under the same name. The declaration scopes here
//! cover every other spelling: a type only a submodule or facade publishes, and a root name that names a different
//! declaration. Each such spelling maps to the declaration's provider-qualified key, whose identity and members come
//! from the declaration itself.

use std::collections::{HashMap, HashSet};

use crate::api_metadata::{
    ApiDeclaration, CheckedApiMetadataPackage, is_package_root_module, resolve_api_alias_target,
};
use crate::library_manifest::LibraryManifest;
use crate::symbols::{SymbolKind, TypeInfo};
use crate::typechecker::{TypeChecker, canonical_public_library_type_name};
use incan_semantics_core::CanonicalSymbolId;

impl TypeChecker {
    /// The provider-qualified key a consumer uses for the nominal declaration `name` of `module_path` in `library`.
    ///
    /// A root-module declaration is keyed by its root public name, as the root's own exports are. Any other declaration
    /// is keyed by its module path, the key a module-namespace import of it already uses.
    fn declaration_type_key(library: &str, module_path: &[String], name: &str) -> String {
        if is_package_root_module(module_path) {
            return canonical_public_library_type_name(library, name);
        }
        let path = module_path
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect::<Vec<_>>()
            .join("::");
        canonical_public_library_type_name(library, &path)
    }

    /// Follow the spelling `name` in the checked API module `module_path` to the model, class, enum or newtype it
    /// declares, returning the declaring module and the declared name.
    ///
    /// An alias, public or private, is followed to its target as the module wrote it, through as many modules as the
    /// chain crosses. A spelling that reaches no nominal declaration of this package, such as an import from another
    /// package, a function or a cycle, yields `None`.
    fn api_nominal_declaration_path(
        api: &CheckedApiMetadataPackage,
        module_path: &[String],
        name: &str,
    ) -> Option<(Vec<String>, String)> {
        let mut module_path = module_path.to_vec();
        let mut name = name.to_string();
        let mut visited = HashSet::new();
        loop {
            if !visited.insert((module_path.clone(), name.clone())) {
                return None;
            }
            let module = api.modules.iter().find(|module| module.module_path == module_path)?;
            let declaration = module
                .declarations
                .iter()
                .filter(|declaration| Self::api_declaration_name(declaration) == name)
                .min_by_key(|declaration| matches!(declaration, ApiDeclaration::Alias(_)))?;
            match declaration {
                ApiDeclaration::Model(_)
                | ApiDeclaration::Class(_)
                | ApiDeclaration::Enum(_)
                | ApiDeclaration::Newtype(_) => return Some((module_path, name)),
                ApiDeclaration::Alias(alias) => {
                    let target = resolve_api_alias_target(&module_path, &alias.target_path);
                    let (target_name, target_module) = target.split_last()?;
                    name = target_name.clone();
                    module_path = target_module.to_vec();
                }
                _ => return None,
            }
        }
    }

    /// Build, for every checked API module of `manifest`, the spellings whose declaration the package root's public
    /// names do not carry, each mapped to its declaration's key.
    ///
    /// A spelling is left out when the root publishes the same declaration under it, so the root's existing mapping,
    /// including a consumer's own import of that name, keeps spelling it. A manifest without canonical identities
    /// gives no evidence that two paths name one declaration and yields no entries.
    fn build_pub_library_declaration_scopes(
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<Vec<String>, HashMap<String, String>> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return HashMap::new();
        };
        let graph = &manifest.contract_metadata.identity_graph;
        let package_path = |segments: &[String], name: &str| {
            std::iter::once(manifest.name.clone())
                .chain(segments.iter().cloned())
                .chain(std::iter::once(name.to_string()))
                .collect::<Vec<_>>()
        };
        let mut scopes = HashMap::new();
        for module in &api.modules {
            let mut scope = HashMap::new();
            for declaration in &module.declarations {
                let name = Self::api_declaration_name(declaration);
                let Some((declaring_module, declared_name)) =
                    Self::api_nominal_declaration_path(api, &module.module_path, name)
                else {
                    continue;
                };
                if is_package_root_module(&declaring_module) {
                    continue;
                }
                let Some(declared) = graph.canonical_for_public_path(&package_path(&declaring_module, &declared_name))
                else {
                    continue;
                };
                if graph.canonical_for_public_path(&package_path(&[], name)).as_ref() == Some(&declared) {
                    continue;
                }
                scope.insert(
                    name.to_string(),
                    Self::declaration_type_key(library, &declaring_module, &declared_name),
                );
            }
            if !scope.is_empty() {
                scopes.insert(module.module_path.clone(), scope);
            }
        }
        scopes
    }

    /// Return the declaration scope of one checked API module of `library`, building the library's scopes once.
    fn pub_library_declaration_scope(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        module_path: &[String],
    ) -> HashMap<String, String> {
        if !self.pub_library_declaration_scopes.contains_key(library) {
            let scopes = Self::build_pub_library_declaration_scopes(library, manifest);
            self.pub_library_declaration_scopes.insert(library.to_string(), scopes);
        }
        self.pub_library_declaration_scopes
            .get(library)
            .and_then(|scopes| scopes.get(module_path))
            .cloned()
            .unwrap_or_default()
    }

    /// Rewrite the type names in `kind`, a symbol built from the declaration `canonical` identifies, through its
    /// declaring module's scope.
    ///
    /// The identity graph's direct entry for the declaration names its declaring module. Names the scope does not
    /// cover are left for the caller's root and import remapping.
    pub(super) fn remap_symbol_kind_through_declaring_module(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        canonical: Option<&CanonicalSymbolId>,
        kind: &mut SymbolKind,
    ) {
        let Some(declaring_module) = canonical
            .and_then(|canonical| {
                manifest
                    .contract_metadata
                    .identity_graph
                    .declaration_source_path(canonical)
            })
            .and_then(|source_path| source_path.split_last())
            .map(|(_, module_path)| module_path.to_vec())
        else {
            return;
        };
        let scope = self.pub_library_declaration_scope(library, manifest, &declaring_module);
        self.remap_symbol_kind_with_import_aliases(kind, &scope);
    }

    /// Register every nominal declaration a submodule of `library` publishes under its declaration key: its identity,
    /// and its members with their type names read in the declaring module's scope, then through `root_remapping`.
    ///
    /// These keys are what the declaration scopes map spellings to, so a field whose type only a submodule publishes
    /// resolves to that declaration's fields and methods. A key another path registered first keeps its entry.
    pub(super) fn register_pub_library_declarations(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        root_remapping: &HashMap<String, String>,
    ) {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return;
        };
        let graph = &manifest.contract_metadata.identity_graph;
        for module in &api.modules {
            if is_package_root_module(&module.module_path) {
                continue;
            }
            let scope = self.pub_library_declaration_scope(library, manifest, &module.module_path);
            for declaration in &module.declarations {
                if !matches!(
                    declaration,
                    ApiDeclaration::Model(_)
                        | ApiDeclaration::Class(_)
                        | ApiDeclaration::Enum(_)
                        | ApiDeclaration::Newtype(_)
                ) {
                    continue;
                }
                let name = Self::api_declaration_name(declaration);
                let key = Self::declaration_type_key(library, &module.module_path, name);
                let mut source_path = module.module_path.clone();
                source_path.push(name.to_string());
                let public_path = std::iter::once(manifest.name.clone())
                    .chain(source_path.iter().cloned())
                    .collect::<Vec<_>>();
                let identity = self.admitted_public_type_identity(
                    library,
                    &source_path,
                    graph.canonical_for_public_path(&public_path),
                );
                self.public_library_type_identities
                    .entry(key.clone())
                    .or_insert(identity);
                if self.transitive_pub_types.contains_key(&key) {
                    continue;
                }
                let Some(mut kind) = self.symbol_kind_from_api_declaration(Some(api), declaration) else {
                    continue;
                };
                self.remap_symbol_kind_with_import_aliases(&mut kind, &scope);
                self.remap_symbol_kind_with_import_aliases(&mut kind, root_remapping);
                Self::mark_compiled_class_field_provider(&mut kind, library);
                if let SymbolKind::Type(info) = kind
                    && !matches!(info, TypeInfo::TypeAlias | TypeInfo::Builtin)
                {
                    self.transitive_pub_types.entry(key).or_default().push(info);
                }
            }
        }
    }
}
