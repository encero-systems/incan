//! Immutable dependency-interface snapshots and the deterministic legacy comparison oracle.
//!
//! Each source module is collected once into a sealed interface. Downstream checkers install those immutable
//! interfaces directly; the deterministic snapshot remains the comparison oracle for the retained legacy fallback.

use super::*;

mod checked_namespace;
mod stable_encoding;

pub use checked_namespace::CheckedNamespaceInterface;
use serde::Serialize;
use stable_encoding::stable_encode;

/// Schema version for the complete dependency-interface snapshot compared by the shadow oracle.
pub const MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION: u32 = 4;

/// Build-scoped immutable preparations shared by the fresh checkers of one collected module graph.
///
/// The cache contains only manifest-derived dependency facts. It never stores local declarations, lexical imports,
/// diagnostics produced by a source module, or a merged source dependency closure.
#[derive(Clone, Debug, Default)]
pub struct DependencyInterfacePreparationCache {
    pub(super) foreign_remappings: Arc<std::sync::Mutex<HashMap<String, Arc<PreparedForeignRemapping>>>>,
}

/// One compiled Loaf's prepared foreign nominal remapping and the effects its installation replays.
#[derive(Clone, Debug)]
pub(super) struct PreparedForeignRemapping {
    pub(super) remapping: Arc<HashMap<String, String>>,
    pub(super) replacing_identities: Arc<HashMap<String, PublicLibraryTypeIdentity>>,
    pub(super) preserving_identities: Arc<HashMap<String, PublicLibraryTypeIdentity>>,
    pub(super) appending_types: Arc<HashMap<String, Vec<TypeInfo>>>,
    pub(super) preserving_types: Arc<HashMap<String, Vec<TypeInfo>>>,
    pub(super) errors: Arc<[crate::diagnostics::CompileError]>,
}

/// One provider's sealed dependency interface, built once from its successful local checker.
///
/// The interface is retained behind [`Arc`] by orchestration. This stage keeps legacy dependency collection as the
/// lookup authority; the provider snapshot is the immutable candidate that tests compare with the legacy oracle.
#[derive(Clone, Debug)]
pub struct ModuleDependencyInterface {
    /// Canonical collected-module key selecting this provider.
    pub module_name: String,
    /// True module identity segments, kept distinct from flattened cache keys.
    pub module_path: Vec<String>,
    /// Whether the sealed surface is the public projection or a standard package's complete sibling projection.
    pub public_only: bool,
    /// Public visibility projection shared since stage 1.
    pub exports: Arc<[ExportedSymbol]>,
    /// Exact member symbols after facade re-export refresh.
    member_symbols: Arc<HashMap<String, SymbolKind>>,
    /// Transparent type-alias targets after facade re-export refresh.
    member_type_aliases: Arc<HashMap<String, TypeAliasTarget>>,
    /// Checked registry definition owned by this provider, when present.
    registry_definitions: Option<DependencyRegistryDefinitions>,
    /// Provider-owned structural registry constants.
    registry_consts: Arc<HashMap<CanonicalSymbolId, Result<incan_semantics_core::SemanticRegistryValue, String>>>,
    /// Partial projections after facade re-export refresh.
    member_partial_projections: Arc<HashMap<String, PartialProjectionInfo>>,
    /// Direct provider declarations before facade projection.
    direct_member_symbols: Arc<HashMap<String, SymbolKind>>,
    /// Direct provider alias targets before facade projection.
    direct_member_type_aliases: Arc<HashMap<String, TypeAliasTarget>>,
    /// Direct provider canonical identities before facade projection.
    direct_member_identities: Arc<HashMap<String, CanonicalSymbolId>>,
    /// Provider-owned declaration scopes used to interpret nominal signatures.
    declaration_nominal_scopes:
        Arc<HashMap<CanonicalSymbolId, Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>>>,
    /// Carrier spellings contributed by the retained provider declaration scopes.
    retained_nominal_carriers: Arc<std::collections::BTreeMap<CanonicalSymbolId, String>>,
    /// Original provider callable templates.
    declaration_callables: Arc<HashMap<CanonicalSymbolId, DeclarationCallableTypes>>,
    /// Source re-export links published by the provider.
    member_reexports: Arc<HashMap<String, DependencyMemberReexport>>,
    /// Public-library re-export links needed only by Body IR checking.
    body_ir_pub_reexports: Arc<HashMap<String, (ImportPath, String)>>,
    /// Direct provider partial projections before facade projection.
    direct_member_partial_projections: Arc<HashMap<String, PartialProjectionInfo>>,
    /// Provider-qualified derivable-module entries.
    derivable_modules: Arc<HashMap<String, Vec<String>>>,
    /// Provider-qualified canonical derive-trait identities.
    derivable_trait_identities: Arc<HashMap<String, CanonicalSymbolId>>,
    /// Provider-qualified trait metadata.
    module_traits: Arc<HashMap<String, TraitInfo>>,
    /// Provider-qualified native derive paths.
    trait_rust_derive_paths: Arc<HashMap<String, Vec<String>>>,
    /// Provider-owned nominal layouts retained for transitive signature lookup.
    transitive_public_types: Arc<HashMap<String, Vec<TypeInfo>>>,
    /// Provider-owned trait layouts retained for transitive signature lookup.
    transitive_public_traits: Arc<HashMap<String, Vec<TraitInfo>>>,
    /// Compiled libraries whose semantic interfaces provider signatures reference.
    compiled_libraries: Arc<[String]>,
    /// Checked identities for compiled-library types referenced by provider signatures.
    public_library_type_identities: Arc<HashMap<String, PublicLibraryTypeIdentity>>,
    /// Historical provider symbols retained in declaration order for checker-wide semantic scans.
    symbol_rows: Arc<[(Symbol, Option<CanonicalSymbolId>)]>,
    /// Deterministic semantic receipt for all provider-owned tables.
    pub snapshot: ModuleDependencyInterfaceSnapshot,
}

/// A deterministic representation of all dependency-owned state in one fresh checker.
///
/// Equality is structural and derived. Each table sorts map entries by its rendered key while retaining sequence
/// indexes for ordered values, so a mismatch distinguishes changed contents from changed ordering without bespoke
/// semantic comparison code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDependencyInterfaceSnapshot {
    /// Snapshot schema version, changed whenever the represented interface surface changes.
    pub version: u32,
    /// Complete legacy tables in stable table-name order.
    pub tables: Vec<DependencyInterfaceTableSnapshot>,
}

impl ModuleDependencyInterfaceSnapshot {
    /// Render a stable, line-oriented receipt suitable for a test failure or checked snapshot corpus.
    pub fn render(&self) -> String {
        let mut rendered = format!("module-dependency-interface-v{}\n", self.version);
        for table in &self.tables {
            rendered.push_str(&format!("[{}] {}\n", table.name, table.entries.len()));
            for entry in &table.entries {
                rendered.push_str(&hex::encode(entry));
                rendered.push('\n');
            }
        }
        rendered
    }
}

/// One legacy dependency-interface table rendered into deterministic entry order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyInterfaceTableSnapshot {
    /// Stable table name used by mismatch diagnostics.
    pub name: &'static str,
    /// Complete key/value records, sorted unless the source table is intrinsically ordered.
    pub entries: Vec<Vec<u8>>,
}

/// Exact state contributed while one provider was collected inside its component builder.
struct ProviderCollectionReceipt {
    symbol_range: std::ops::Range<usize>,
    compiled_libraries: Vec<String>,
    public_library_type_identities: HashMap<String, PublicLibraryTypeIdentity>,
}

impl TypeChecker {
    /// Share one build's immutable compiled-dependency preparations with this fresh checker.
    pub fn set_dependency_interface_preparation_cache(&mut self, cache: DependencyInterfacePreparationCache) {
        self.dependency_interface_preparation_cache = cache;
    }

    /// Select the immutable provider handles that form the next module's exact dependency closure.
    pub fn select_dependency_interfaces(&mut self, interfaces: Vec<Arc<ModuleDependencyInterface>>) {
        self.selected_dependency_interfaces = interfaces;
    }

    /// Consume selected handles only when they match the complete dependency list in stable closure order.
    pub(super) fn take_exact_dependency_interfaces(
        &mut self,
        dependencies: &[(&str, &Program)],
    ) -> Option<Vec<Arc<ModuleDependencyInterface>>> {
        let exact = self.selected_dependency_interfaces.len() == dependencies.len()
            && self
                .selected_dependency_interfaces
                .iter()
                .zip(dependencies)
                .all(|(interface, (name, _))| interface.module_name == *name);
        exact.then(|| std::mem::take(&mut self.selected_dependency_interfaces))
    }

    /// Capture every dependency-owned table and historical symbol row from the current checker.
    ///
    /// This method does not mutate or authorize lookup. It is the shadow oracle for migration to shared immutable
    /// interfaces: tests compare snapshots produced by legacy collection with snapshots produced by the candidate
    /// builder before any candidate value may answer a checker lookup.
    pub fn dependency_interface_snapshot(&self) -> ModuleDependencyInterfaceSnapshot {
        let tables = vec![
            ordered_nested_table("public_exports", &self.dependency_exports),
            nested_table("member_symbols", &self.dependency_member_symbols),
            nested_table("member_type_aliases", &self.dependency_member_type_aliases),
            map_table("registry_definitions", &self.dependency_registry_definitions),
            map_table("registry_consts", &self.dependency_registry_consts),
            nested_table(
                "member_partial_projections",
                &self.dependency_member_partial_projections,
            ),
            nested_table("direct_member_symbols", &self.dependency_direct_member_symbols),
            nested_table(
                "direct_member_type_aliases",
                &self.dependency_direct_member_type_aliases,
            ),
            nested_table("direct_member_identities", &self.dependency_direct_member_identities),
            shared_map_table("declaration_nominal_scopes", &self.declaration_nominal_scopes),
            map_table("retained_nominal_carriers", &self.retained_nominal_carriers),
            map_table("declaration_callables", &self.declaration_callables),
            nested_table("member_reexports", &self.dependency_member_reexports),
            nested_table("body_ir_pub_reexports", &self.body_ir_dependency_pub_reexports),
            map_table("module_paths", &self.dependency_module_path_segments),
            nested_table(
                "direct_member_partial_projections",
                &self.dependency_direct_member_partial_projections,
            ),
            map_table("derivable_modules", &self.dependency_derivable_modules),
            map_table(
                "derivable_trait_identities",
                &self.dependency_derivable_trait_identities,
            ),
            map_table("module_traits", &self.dependency_module_traits),
            map_table("trait_rust_derive_paths", &self.dependency_trait_rust_derive_paths),
            map_table("transitive_public_types", &self.transitive_pub_types),
            map_table("transitive_public_traits", &self.transitive_pub_traits),
            map_table("public_library_type_identities", &*self.public_library_type_identities),
            DependencyInterfaceTableSnapshot {
                name: "symbol_rows",
                entries: self
                    .symbols
                    .dependency_interface_symbols()
                    .map(|(id, symbol, identity)| stable_encode(&(id, symbol, identity)))
                    .collect(),
            },
        ];
        ModuleDependencyInterfaceSnapshot {
            version: MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION,
            tables,
        }
    }

    /// Seal the successfully checked current module as one immutable provider interface.
    ///
    /// The current module's declarations are already complete and its exact dependency closure is still installed.
    /// Reusing the legacy cache extractors here captures provider-owned signatures, aliases, registries, traits,
    /// derives, re-exports and nominal declaration scopes without checking a body again or merging dependency
    /// closures. The resulting value does not answer lookup until the later routing stage.
    pub fn seal_current_module_dependency_interface(
        &mut self,
        module_name: &str,
        module_ast: &Program,
        exports: Arc<[ExportedSymbol]>,
        public_only: bool,
    ) -> Arc<ModuleDependencyInterface> {
        self.seal_module_dependency_interface(module_name, module_ast, exports, public_only, None, true)
    }

    /// Collect and seal every member of one dependency component exactly once.
    ///
    /// Already-installed interfaces provide dependencies outside the component. Component members are predeclared
    /// together, collected under their own registered module paths and sealed with the exact historical symbol rows
    /// each collection produced. This is the only declaration-collection route retained for shared interfaces.
    pub fn build_dependency_interface_component(
        &mut self,
        modules: &[(&str, &Program, Arc<[ExportedSymbol]>)],
        declaration_context: &[(&str, &Program)],
        public_only: bool,
    ) -> Vec<Arc<ModuleDependencyInterface>> {
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::build_dependency_interface_component",
            modules = modules.len(),
            public_only,
        )
        .entered();
        let Some((_, first, _)) = modules.first() else {
            return Vec::new();
        };
        self.record_nominal_declaring_modules(first, declaration_context);
        let programs = modules
            .iter()
            .map(|(name, program, _)| (*name, *program))
            .collect::<Vec<_>>();
        for (name, _, exports) in modules {
            self.dependency_exports.insert((*name).to_string(), Arc::clone(exports));
        }
        self.predeclare_dependency_interfaces(&programs, public_only);
        let mut receipts = Vec::with_capacity(modules.len());
        {
            let _trace = tracing::trace_span!(
                target: "incan::invocation",
                "incan_frontend::import_dependency_interfaces",
                dependencies = modules.len(),
                public_only,
            )
            .entered();
            for (name, program, _) in modules {
                let start = self.symbols.all_symbols().len();
                let (source_types, source_traits) = self.collected_source_semantics();
                self.transitive_pub_types = source_types;
                self.transitive_pub_traits = source_traits;
                self.public_library_type_identities.clear();
                self.cached_pub_libraries.clear();
                self.foreign_pub_type_remappings.clear();
                self.pub_library_declaration_scopes.clear();
                if public_only {
                    self.import_module(program, name);
                } else {
                    self.import_module_all(program, name);
                }
                let mut compiled_libraries = self.cached_pub_libraries.iter().cloned().collect::<Vec<_>>();
                compiled_libraries.sort();
                receipts.push(ProviderCollectionReceipt {
                    symbol_range: start..self.symbols.all_symbols().len(),
                    compiled_libraries,
                    public_library_type_identities: self
                        .public_library_type_identities
                        .iter()
                        .map(|(name, identity)| (name.clone(), identity.clone()))
                        .collect(),
                });
            }
        }
        self.refresh_dependency_member_symbols(&programs, public_only);
        modules
            .iter()
            .zip(receipts)
            .map(|((name, program, exports), receipt)| {
                self.seal_module_dependency_interface(
                    name,
                    program,
                    Arc::clone(exports),
                    public_only,
                    Some(receipt),
                    false,
                )
            })
            .collect()
    }

    /// Reconstruct only source-provider nominal layouts before collecting one component member's compiled imports.
    ///
    /// Compiled-library caches publish bare, canonical and carrier keys, so key spelling cannot separate them from
    /// source layouts. The module-keyed direct source tables are the authoritative source-only boundary.
    fn collected_source_semantics(&self) -> (HashMap<String, Vec<TypeInfo>>, HashMap<String, Vec<TraitInfo>>) {
        let mut types = HashMap::new();
        let mut traits = HashMap::new();
        for (module_name, symbols) in &self.dependency_direct_member_symbols {
            let identities = self
                .dependency_direct_member_identities
                .get(module_name)
                .cloned()
                .unwrap_or_default();
            let (module_types, module_traits) = provider_transitive_semantics(symbols, &identities);
            extend_semantic_cache(&mut types, &module_types);
            extend_semantic_cache(&mut traits, &module_traits);
        }
        (types, traits)
    }

    /// Seal one provider from the current cache state, optionally retaining an exact collection-owned symbol range.
    fn seal_module_dependency_interface(
        &mut self,
        module_name: &str,
        module_ast: &Program,
        exports: Arc<[ExportedSymbol]>,
        public_only: bool,
        collection: Option<ProviderCollectionReceipt>,
        rebuild_provider_cache: bool,
    ) -> Arc<ModuleDependencyInterface> {
        self.dependency_exports
            .insert(module_name.to_string(), Arc::clone(&exports));
        if rebuild_provider_cache {
            self.register_dependency_derivable_metadata(module_name, module_ast);
            self.cache_dependency_direct_member_symbols(module_name, module_ast, public_only);
            self.cache_dependency_registry_definitions(module_name, module_ast, public_only);
            self.cache_dependency_registry_consts(module_ast);
            self.cache_dependency_member_symbols(module_name, module_ast, public_only);
            self.cache_dependency_adoption_traits();
        }
        let module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .or_else(|| self.current_module_path.clone())
            .unwrap_or_else(|| vec![module_name.to_string()]);
        let direct_member_identities = self
            .dependency_direct_member_identities
            .get(module_name)
            .cloned()
            .unwrap_or_default();
        let direct_member_symbols = self
            .dependency_direct_member_symbols
            .get(module_name)
            .cloned()
            .unwrap_or_default();
        let (transitive_public_types, transitive_public_traits) =
            provider_transitive_semantics(&direct_member_symbols, &direct_member_identities);
        let public_library_type_identities = if let Some(receipt) = &collection {
            receipt.public_library_type_identities.clone()
        } else {
            self.public_library_type_identities
                .iter()
                .map(|(name, identity)| (name.clone(), identity.clone()))
                .collect()
        };
        let compiled_libraries = collection
            .as_ref()
            .map(|receipt| receipt.compiled_libraries.clone())
            .unwrap_or_else(|| {
                let mut libraries = self.cached_pub_libraries.iter().cloned().collect::<Vec<_>>();
                libraries.sort();
                libraries
            });
        let owned_identity = |identity: &CanonicalSymbolId| identity_owned_by(identity, &module_path);
        let qualified = |key: &String| key_owned_by(key, &module_path);
        let symbol_rows = self
            .symbols
            .dependency_interface_symbols()
            .filter(|(id, _, identity)| {
                collection
                    .as_ref()
                    .is_some_and(|receipt| receipt.symbol_range.contains(id))
                    || identity.is_some_and(&owned_identity)
            })
            .map(|(_, symbol, identity)| (symbol.clone(), identity.cloned()))
            .collect::<Vec<_>>();
        Arc::new(ModuleDependencyInterface {
            module_name: module_name.to_string(),
            module_path: module_path.clone(),
            public_only,
            exports,
            member_symbols: self
                .dependency_member_symbols
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            member_type_aliases: self
                .dependency_member_type_aliases
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            registry_definitions: self.dependency_registry_definitions.get(module_name).cloned(),
            registry_consts: Arc::new(
                self.dependency_registry_consts
                    .iter()
                    .filter(|(identity, _)| owned_identity(identity))
                    .map(|(identity, value)| (identity.clone(), value.clone()))
                    .collect(),
            ),
            member_partial_projections: self
                .dependency_member_partial_projections
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            direct_member_symbols,
            direct_member_type_aliases: self
                .dependency_direct_member_type_aliases
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            direct_member_identities,
            declaration_nominal_scopes: Arc::new(
                self.declaration_nominal_scopes
                    .iter()
                    .filter(|(identity, _)| owned_identity(identity))
                    .map(|(identity, scope)| (identity.clone(), Arc::clone(scope)))
                    .collect(),
            ),
            retained_nominal_carriers: Arc::new(
                self.retained_nominal_carriers
                    .iter()
                    .filter(|(identity, _)| owned_identity(identity))
                    .map(|(identity, carrier)| (identity.clone(), carrier.clone()))
                    .collect(),
            ),
            declaration_callables: Arc::new(
                self.declaration_callables
                    .iter()
                    .filter(|(identity, _)| owned_identity(identity))
                    .map(|(identity, callable)| (identity.clone(), callable.clone()))
                    .collect(),
            ),
            member_reexports: self
                .dependency_member_reexports
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            body_ir_pub_reexports: self
                .body_ir_dependency_pub_reexports
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            direct_member_partial_projections: self
                .dependency_direct_member_partial_projections
                .get(module_name)
                .cloned()
                .unwrap_or_default(),
            derivable_modules: Arc::new(
                self.dependency_derivable_modules
                    .iter()
                    .filter(|(key, _)| qualified(key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            derivable_trait_identities: Arc::new(
                self.dependency_derivable_trait_identities
                    .iter()
                    .filter(|(key, _)| qualified(key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            module_traits: Arc::new(
                self.dependency_module_traits
                    .iter()
                    .filter(|(key, _)| qualified(key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            trait_rust_derive_paths: Arc::new(
                self.dependency_trait_rust_derive_paths
                    .iter()
                    .filter(|(key, _)| qualified(key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            transitive_public_types: Arc::new(transitive_public_types),
            transitive_public_traits: Arc::new(transitive_public_traits),
            compiled_libraries: Arc::from(compiled_libraries),
            public_library_type_identities: Arc::new(public_library_type_identities.clone()),
            symbol_rows: Arc::from(symbol_rows.clone()),
            snapshot: self.dependency_interface_snapshot_for(
                module_name,
                Some(&symbol_rows),
                &public_library_type_identities,
            ),
        })
    }

    /// Replace dependency lookup caches with the exact selected provider interfaces.
    ///
    /// The caller supplies handles in collected-module closure order. Local declarations, imports, diagnostics and
    /// body facts are not touched; only the dependency layer under those local facts is replaced.
    pub fn install_dependency_interfaces(&mut self, interfaces: &[Arc<ModuleDependencyInterface>]) {
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::install_dependency_interface_tables",
            interfaces = interfaces.len(),
        )
        .entered();
        self.clear_dependency_interface_tables();
        for interface in interfaces {
            self.install_dependency_interface(interface);
        }
    }

    /// Clear only the dependency-owned tables replaced by an exact interface selection.
    fn clear_dependency_interface_tables(&mut self) {
        self.dependency_exports.clear();
        self.dependency_member_symbols.clear();
        self.dependency_member_type_aliases.clear();
        self.dependency_registry_definitions.clear();
        self.dependency_registry_consts.clear();
        self.dependency_member_partial_projections.clear();
        self.dependency_direct_member_symbols.clear();
        self.dependency_direct_member_type_aliases.clear();
        self.dependency_direct_member_identities.clear();
        self.declaration_nominal_scopes.clear();
        self.retained_nominal_carriers.clear();
        self.declaration_callables.clear();
        self.dependency_member_reexports.clear();
        self.body_ir_dependency_pub_reexports.clear();
        self.dependency_direct_member_partial_projections.clear();
        self.dependency_derivable_modules.clear();
        self.dependency_derivable_trait_identities.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.transitive_pub_types.clear();
        self.transitive_pub_traits.clear();
        self.public_library_type_identities.clear();
        self.cached_pub_libraries.clear();
    }

    /// Install one sealed provider while preserving the selected closure's stable order.
    fn install_dependency_interface(&mut self, interface: &ModuleDependencyInterface) {
        let name = interface.module_name.clone();
        self.dependency_exports
            .insert(name.clone(), Arc::clone(&interface.exports));
        self.dependency_member_symbols
            .insert(name.clone(), Arc::clone(&interface.member_symbols));
        self.dependency_member_type_aliases
            .insert(name.clone(), Arc::clone(&interface.member_type_aliases));
        if let Some(definitions) = &interface.registry_definitions {
            self.dependency_registry_definitions
                .insert(name.clone(), definitions.clone());
        }
        self.dependency_registry_consts.extend(
            interface
                .registry_consts
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.dependency_member_partial_projections
            .insert(name.clone(), Arc::clone(&interface.member_partial_projections));
        self.dependency_direct_member_symbols
            .insert(name.clone(), Arc::clone(&interface.direct_member_symbols));
        self.dependency_direct_member_type_aliases
            .insert(name.clone(), Arc::clone(&interface.direct_member_type_aliases));
        self.dependency_direct_member_identities
            .insert(name.clone(), Arc::clone(&interface.direct_member_identities));
        self.declaration_nominal_scopes.extend(
            interface
                .declaration_nominal_scopes
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value))),
        );
        self.retained_nominal_carriers.extend(
            interface
                .retained_nominal_carriers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.declaration_callables.extend(
            interface
                .declaration_callables
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.dependency_member_reexports
            .insert(name.clone(), Arc::clone(&interface.member_reexports));
        self.body_ir_dependency_pub_reexports
            .insert(name.clone(), Arc::clone(&interface.body_ir_pub_reexports));
        self.dependency_module_path_segments
            .insert(name.clone(), interface.module_path.clone());
        self.dependency_direct_member_partial_projections
            .insert(name, Arc::clone(&interface.direct_member_partial_projections));
        self.dependency_derivable_modules.extend(
            interface
                .derivable_modules
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.dependency_derivable_trait_identities.extend(
            interface
                .derivable_trait_identities
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.dependency_module_traits.extend(
            interface
                .module_traits
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.dependency_trait_rust_derive_paths.extend(
            interface
                .trait_rust_derive_paths
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        for (name, values) in interface.transitive_public_types.iter() {
            self.transitive_pub_types
                .entry(name.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
        for (name, values) in interface.transitive_public_traits.iter() {
            self.transitive_pub_traits
                .entry(name.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
        for library in interface.compiled_libraries.iter() {
            self.install_compiled_library_semantics(library);
        }
        self.public_library_type_identities.extend(
            interface
                .public_library_type_identities
                .iter()
                .map(|(name, identity)| (name.clone(), identity.clone())),
        );
    }

    /// Rebuild one compiled Loaf's checker-local semantic cache from its already-indexed immutable manifest.
    ///
    /// Source dependency declarations are never recollected here. The library key list is retained by the shared
    /// provider interface, and the checker's existing cache guard admits each compiled library at most once.
    fn install_compiled_library_semantics(&mut self, library: &str) {
        let loaves = Arc::clone(&self.loaves);
        let Some(manifest) = loaves.manifest(library) else {
            return;
        };
        self.cache_transitive_pub_export_semantics(library, manifest);
    }

    /// Install selected interfaces without collecting any provider declaration from source again.
    ///
    /// Dependency symbol rows are replayed in provider and declaration order because checker-wide semantic scans use
    /// the historical arena even though ordinary lookup resolves through immutable module-keyed tables.
    pub fn install_dependency_interfaces_directly(&mut self, interfaces: &[Arc<ModuleDependencyInterface>]) {
        self.install_dependency_interfaces(interfaces);
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::install_dependency_interface_symbols",
            interfaces = interfaces.len(),
        )
        .entered();
        for interface in interfaces {
            self.symbols.begin_dependency_interface_bindings();
            for (symbol, identity) in interface.symbol_rows.iter() {
                self.symbols.define_import_binding(symbol.clone(), identity.clone());
            }
            self.symbols.finish_dependency_interface_bindings();
        }
    }

    /// Capture only the tables owned by `module_name`, excluding every other provider in the checker's closure.
    fn dependency_interface_snapshot_for(
        &self,
        module_name: &str,
        retained_symbol_rows: Option<&[(Symbol, Option<CanonicalSymbolId>)]>,
        public_library_type_identities: &HashMap<String, PublicLibraryTypeIdentity>,
    ) -> ModuleDependencyInterfaceSnapshot {
        let module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
        let direct_member_symbols = self
            .dependency_direct_member_symbols
            .get(module_name)
            .cloned()
            .unwrap_or_default();
        let direct_member_identities = self
            .dependency_direct_member_identities
            .get(module_name)
            .cloned()
            .unwrap_or_default();
        let (transitive_public_types, transitive_public_traits) =
            provider_transitive_semantics(&direct_member_symbols, &direct_member_identities);
        let tables = vec![
            ordered_module_table("public_exports", module_name, &self.dependency_exports),
            nested_module_table("member_symbols", module_name, &self.dependency_member_symbols),
            nested_module_table("member_type_aliases", module_name, &self.dependency_member_type_aliases),
            module_table(
                "registry_definitions",
                module_name,
                &self.dependency_registry_definitions,
            ),
            filtered_map_table("registry_consts", &self.dependency_registry_consts, |identity| {
                identity_owned_by(identity, &module_path)
            }),
            nested_module_table(
                "member_partial_projections",
                module_name,
                &self.dependency_member_partial_projections,
            ),
            nested_module_table(
                "direct_member_symbols",
                module_name,
                &self.dependency_direct_member_symbols,
            ),
            nested_module_table(
                "direct_member_type_aliases",
                module_name,
                &self.dependency_direct_member_type_aliases,
            ),
            nested_module_table(
                "direct_member_identities",
                module_name,
                &self.dependency_direct_member_identities,
            ),
            filtered_shared_map_table(
                "declaration_nominal_scopes",
                &self.declaration_nominal_scopes,
                |identity| identity_owned_by(identity, &module_path),
            ),
            filtered_map_table(
                "retained_nominal_carriers",
                &self.retained_nominal_carriers,
                |identity| identity_owned_by(identity, &module_path),
            ),
            filtered_map_table("declaration_callables", &self.declaration_callables, |identity| {
                identity_owned_by(identity, &module_path)
            }),
            nested_module_table("member_reexports", module_name, &self.dependency_member_reexports),
            nested_module_table(
                "body_ir_pub_reexports",
                module_name,
                &self.body_ir_dependency_pub_reexports,
            ),
            module_table("module_paths", module_name, &self.dependency_module_path_segments),
            nested_module_table(
                "direct_member_partial_projections",
                module_name,
                &self.dependency_direct_member_partial_projections,
            ),
            prefixed_map_table("derivable_modules", &module_path, &self.dependency_derivable_modules),
            prefixed_map_table(
                "derivable_trait_identities",
                &module_path,
                &self.dependency_derivable_trait_identities,
            ),
            prefixed_map_table("module_traits", &module_path, &self.dependency_module_traits),
            prefixed_map_table(
                "trait_rust_derive_paths",
                &module_path,
                &self.dependency_trait_rust_derive_paths,
            ),
            map_table("transitive_public_types", &transitive_public_types),
            map_table("transitive_public_traits", &transitive_public_traits),
            map_table("public_library_type_identities", public_library_type_identities),
            DependencyInterfaceTableSnapshot {
                name: "symbol_rows",
                entries: retained_symbol_rows
                    .into_iter()
                    .flat_map(|rows| rows.iter())
                    .enumerate()
                    .map(|(index, (symbol, identity))| stable_encode(&(index, symbol, identity)))
                    .collect(),
            },
        ];
        ModuleDependencyInterfaceSnapshot {
            version: MODULE_DEPENDENCY_INTERFACE_SNAPSHOT_VERSION,
            tables,
        }
    }
}

/// Reconstruct one provider's retained transitive type and trait contributions from its direct checked members.
fn provider_transitive_semantics(
    symbols: &HashMap<String, SymbolKind>,
    identities: &HashMap<String, CanonicalSymbolId>,
) -> (HashMap<String, Vec<TypeInfo>>, HashMap<String, Vec<TraitInfo>>) {
    let mut types = HashMap::<String, Vec<TypeInfo>>::new();
    let mut traits = HashMap::<String, Vec<TraitInfo>>::new();
    for (name, symbol) in symbols {
        match symbol {
            SymbolKind::Type(info) if !matches!(info, TypeInfo::TypeAlias | TypeInfo::Builtin) => {
                types.entry(name.clone()).or_default().push(info.clone());
                if let Some(carrier) = identities
                    .get(name)
                    .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                {
                    types.entry(carrier).or_insert_with(|| vec![info.clone()]);
                }
            }
            SymbolKind::Trait(info) => traits.entry(name.clone()).or_default().push(info.clone()),
            _ => {}
        }
    }
    (types, traits)
}

/// Merge retained semantic entries while preserving their collection order.
fn extend_semantic_cache<T: Clone>(target: &mut HashMap<String, Vec<T>>, source: &HashMap<String, Vec<T>>) {
    for (name, entries) in source {
        target.entry(name.clone()).or_default().extend(entries.iter().cloned());
    }
}

/// Whether a canonical declaration belongs to the exact source module being sealed.
fn identity_owned_by(identity: &CanonicalSymbolId, module_path: &[String]) -> bool {
    match &identity.origin {
        SymbolOrigin::Module(path) => path == module_path,
        SymbolOrigin::Package { module_path: path, .. } => path == module_path,
        SymbolOrigin::Builtin | SymbolOrigin::RustCrate(_) => false,
    }
}

/// Whether a module-qualified metadata key belongs to the exact provider path.
fn key_owned_by(key: &str, module_path: &[String]) -> bool {
    let exact = module_path.join(".");
    key == exact || key.strip_prefix(&exact).is_some_and(|suffix| suffix.starts_with('.'))
}

/// Render one module's value from a module-keyed map.
fn module_table<V: Serialize>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, V>,
) -> DependencyInterfaceTableSnapshot {
    let entries = values
        .get(module_name)
        .map(|value| vec![stable_encode(&(module_name, value))])
        .unwrap_or_default();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render one module's inner member map in deterministic order.
fn nested_module_table<V: Serialize>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, Arc<HashMap<String, V>>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .get(module_name)
        .into_iter()
        .flat_map(|members| members.iter())
        .map(|(member, value)| stable_encode(&(member, value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render one module's immutable ordered projection.
fn ordered_module_table<V: Serialize>(
    name: &'static str,
    module_name: &str,
    values: &HashMap<String, Arc<[V]>>,
) -> DependencyInterfaceTableSnapshot {
    let entries = values
        .get(module_name)
        .into_iter()
        .flat_map(|members| members.iter().enumerate())
        .map(|(index, value)| stable_encode(&(index, value)))
        .collect();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render entries selected by provider-owned canonical identity.
fn filtered_map_table<K: Serialize, V: Serialize>(
    name: &'static str,
    values: &impl MapEntries<K, V>,
    include: impl Fn(&K) -> bool,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .entries()
        .filter(|(key, _)| include(key))
        .map(|(key, value)| stable_encode(&(key, value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Encode selected map values through their shared inner value rather than pointer identity.
fn filtered_shared_map_table<K: Serialize, V: Serialize>(
    name: &'static str,
    values: &HashMap<K, Arc<V>>,
    include: impl Fn(&K) -> bool,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .filter(|(key, _)| include(key))
        .map(|(key, value)| stable_encode(&(key, value.as_ref())))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Encode a complete map through each shared value's structural contents rather than pointer identity.
fn shared_map_table<K: Serialize, V: Serialize>(
    name: &'static str,
    values: &HashMap<K, Arc<V>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .map(|(key, value)| stable_encode(&(key, value.as_ref())))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render module-qualified metadata whose stable key starts with the provider's canonical source path.
fn prefixed_map_table<V: Serialize>(
    name: &'static str,
    module_path: &[String],
    values: &HashMap<String, V>,
) -> DependencyInterfaceTableSnapshot {
    let prefix = format!("{}.", module_path.join("."));
    let exact = module_path.join(".");
    let mut entries = values
        .iter()
        .filter(|(key, _)| *key == &exact || key.starts_with(&prefix))
        .map(|(key, value)| stable_encode(&(key, value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render a map into key-sorted records without relying on its randomized iteration order.
fn map_table<K: Serialize, V: Serialize>(
    name: &'static str,
    values: &impl MapEntries<K, V>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .entries()
        .map(|(key, value)| stable_encode(&(key, value)))
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render a module-keyed map whose inner maps also need deterministic ordering.
fn nested_table<V: Serialize>(
    name: &'static str,
    values: &HashMap<String, Arc<HashMap<String, V>>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .flat_map(|(module, members)| {
            members
                .iter()
                .map(move |(member, value)| stable_encode(&(module, member, value)))
        })
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Render module-keyed immutable slices while preserving each projection's declared order.
fn ordered_nested_table<V: Serialize>(
    name: &'static str,
    values: &HashMap<String, Arc<[V]>>,
) -> DependencyInterfaceTableSnapshot {
    let mut entries = values
        .iter()
        .flat_map(|(module, members)| {
            members
                .iter()
                .enumerate()
                .map(move |(index, value)| stable_encode(&(module, index, value)))
        })
        .collect::<Vec<_>>();
    entries.sort();
    DependencyInterfaceTableSnapshot { name, entries }
}

/// Common read-only entry view for hash and ordered maps used by interface state.
trait MapEntries<K, V> {
    /// Borrow every key/value pair without imposing iteration order.
    fn entries<'a>(&'a self) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a;
}

impl<K, V> MapEntries<K, V> for HashMap<K, V> {
    fn entries<'a>(&'a self) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        self.iter()
    }
}

impl<K, V> MapEntries<K, V> for std::collections::BTreeMap<K, V> {
    fn entries<'a>(&'a self) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        self.iter()
    }
}
